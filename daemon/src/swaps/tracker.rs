use std::collections::HashMap;
use std::time::Duration;

use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::balance_checker::BalanceChecker;
use crate::clients::{
    ExecutorSwapStatus,
    SwapsClientError,
};
use crate::dao::{
    DaoInterface,
    DaoSwapError,
    DaoTransactionInterface,
};
use crate::types::{
    PayoutStatus,
    RefundStatus,
    Swap,
    SwapStatus,
    TransactionOriginVariant,
};
use crate::utils::logging::{
    category,
    operation,
};

use super::SwapsClients;

const SWAPS_EXECUTOR_API_POLLING_INTERVAL_MILLIS: u64 = 3000;
const SWAPS_EXECUTOR_DATABASE_POLLING_INTERVAL_MILLIS: u64 = 100;
const SWAPS_EXECUTOR_PENDING_RELOAD_RETRY_MILLIS: u64 = 5000;
const SWAPS_EXECUTOR_PENDING_RELOAD_MAX_ATTEMPTS: u32 = 12;

struct TrackedSwaps {
    swaps: HashMap<Uuid, Swap>,
}

impl TrackedSwaps {
    pub fn new() -> Self {
        Self {
            swaps: HashMap::new(),
        }
    }

    pub fn has_any_swaps(&self) -> bool {
        !self.swaps.is_empty()
    }

    pub fn add_swaps(
        &mut self,
        swaps: Vec<Swap>,
    ) {
        for swap in swaps {
            self.swaps.insert(swap.id, swap);
        }
    }

    pub fn get_all_swaps(&self) -> Vec<Swap> {
        self.swaps.values().cloned().collect()
    }

    pub fn remove_swap(
        &mut self,
        swap_id: Uuid,
    ) {
        self.swaps.remove(&swap_id);
    }
}

/// Apply the result of re-reading a hashless tracked swap.
///
/// Kept separate from the database call so the tracker-store behavior can be
/// tested without constructing its unrelated provider and chain clients.
fn apply_hashless_swap_reload(
    store: &mut TrackedSwaps,
    swap: &Swap,
    reload_result: Result<Option<Swap>, DaoSwapError>,
) -> Option<Swap> {
    let reloaded = match reload_result {
        Ok(Some(reloaded)) => reloaded,
        Ok(None) => {
            tracing::warn!(
                swap_id = %swap.id,
                invoice_id = %swap.request.invoice_id,
                "Tracked swap has disappeared from the database, dropping it from the tracker"
            );
            store.remove_swap(swap.id);
            return None;
        },
        Err(e) => {
            tracing::warn!(
                swap_id = %swap.id,
                invoice_id = %swap.request.invoice_id,
                error = ?e,
                "Failed to re-read a swap with no transaction hash, will retry next round"
            );
            return None;
        },
    };

    if reloaded
        .swap_details
        .transaction_hash
        .is_none()
    {
        if matches!(
            reloaded.status,
            SwapStatus::Completed | SwapStatus::Failed | SwapStatus::Abandoned
        ) {
            // Loud on purpose: the swap was marked `Submitted` before the
            // external call, so funds may have been in flight with nothing
            // recorded to track them by. Remove it after this signal because a
            // terminal row can never acquire a hash on a later refresh.
            tracing::warn!(
                swap_id = %swap.id,
                invoice_id = %swap.request.invoice_id,
                swap_status = %reloaded.status,
                "Tracked swap reached a terminal status without a transaction hash; manual reconciliation may be required, dropping it from the tracker"
            );
            store.remove_swap(swap.id);
            return None;
        }

        // Loud on purpose: the swap was marked `Submitted` before the external
        // call, so funds may be in flight with nothing recorded to track them
        // by.
        tracing::warn!(
            swap_id = %swap.id,
            invoice_id = %swap.request.invoice_id,
            swap_status = %reloaded.status,
            "Tracked swap still has no transaction hash and cannot be polled — if this persists, its submission needs manual reconciliation"
        );
        return None;
    }

    store.add_swaps(vec![reloaded.clone()]);

    Some(reloaded)
}

pub struct SwapsTracker<D: DaoInterface + 'static> {
    dao: D,
    store: TrackedSwaps,
    clients: SwapsClients,
    balance_checker: BalanceChecker,
}

#[expect(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum SwapsTrackerError {
    ApiError,
    DatabaseError,
    BalanceCheckerError,
}

impl From<SwapsClientError> for SwapsTrackerError {
    fn from(_value: SwapsClientError) -> Self {
        SwapsTrackerError::ApiError
    }
}

impl<D: DaoInterface + 'static> SwapsTracker<D> {
    pub fn new(
        dao: D,
        clients: SwapsClients,
        balance_checker: BalanceChecker,
    ) -> Self {
        Self {
            dao,
            clients,
            balance_checker,
            store: TrackedSwaps::new(),
        }
    }

    #[tracing::instrument(skip_all)]
    async fn handle_swap_executed(
        &mut self,
        swap: &Swap,
    ) -> Result<(), SwapsTrackerError> {
        // TODO: check error, if it's Invoice not found, skip monitoring (shouldn't
        // happen though)
        let invoice = self
            .balance_checker
            .check_invoice_balance(swap.request.invoice_id)
            .await
            .map_err(|e| {
                tracing::warn!(
                    error = ?e,
                    "Error while check balance after swap has been executed"
                );

                SwapsTrackerError::BalanceCheckerError
            })?;

        tracing::debug!(
            invoice_with_amount = ?invoice,
            "Invoice has been checked after swap successful execution"
        );

        if invoice.total_received_amount.is_zero() {
            tracing::warn!(
                "Swap has executed status but received amount after check is still zero. Will recheck balance later"
            );
            return Err(SwapsTrackerError::BalanceCheckerError)
        }

        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_| SwapsTrackerError::DatabaseError)?;

        dao_transaction
            .update_swap_completed(swap.id)
            .await
            .map_err(|_| SwapsTrackerError::DatabaseError)?;

        match swap.request.origin.variant() {
            TransactionOriginVariant::Payout(payout_id) => {
                dao_transaction
                    .update_payout_status(payout_id, PayoutStatus::Completed)
                    .await
                    .map_err(|_| SwapsTrackerError::DatabaseError)?;
            },
            TransactionOriginVariant::Refund(refund_id) => {
                dao_transaction
                    .update_refund_status(refund_id, RefundStatus::Completed)
                    .await
                    .map_err(|_| SwapsTrackerError::DatabaseError)?;
            },
            #[expect(
                clippy::unreachable,
                reason = "pre-existing panic site, grandfathered when the panic gate landed; see the panic-gate backlog in docs/conventions.md"
            )]
            TransactionOriginVariant::InternalTransfer(_) => unreachable!(),
            TransactionOriginVariant::None => {},
        }

        dao_transaction
            .commit()
            .await
            .map_err(|_| SwapsTrackerError::DatabaseError)?;

        self.store.remove_swap(swap.id);
        tracing::info!("Swap has been filled and marked as completed in the database");

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn handle_swap_failed(
        &mut self,
        swap: &Swap,
    ) -> Result<(), SwapsTrackerError> {
        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_| SwapsTrackerError::DatabaseError)?;

        dao_transaction
            .update_swap_failed(
                swap.id,
                "Swap has been failed and refunded".to_string(),
            )
            .await
            .map_err(|_| SwapsTrackerError::DatabaseError)?;

        match swap.request.origin.variant() {
            TransactionOriginVariant::Payout(payout_id) => {
                if let Some(payout) = dao_transaction
                    .get_payout_by_id(payout_id)
                    .await
                    .map_err(|_| SwapsTrackerError::DatabaseError)?
                {
                    let mut retry_meta = payout.retry_meta;
                    retry_meta.increment_retry("Swap has failed".to_string());

                    dao_transaction
                        .update_payout_retry(payout_id, retry_meta, true)
                        .await
                        .map_err(|_| SwapsTrackerError::DatabaseError)?;
                } else {
                    // TODO: add logs but it shouldn't really happen
                }
            },
            TransactionOriginVariant::Refund(refund_id) => {
                if let Some(refund) = dao_transaction
                    .get_refund_by_id(refund_id)
                    .await
                    .map_err(|_| SwapsTrackerError::DatabaseError)?
                {
                    let mut retry_meta = refund.retry_meta;
                    retry_meta.increment_retry("Swap has failed".to_string());

                    dao_transaction
                        .update_refund_retry(refund_id, retry_meta, true)
                        .await
                        .map_err(|_| SwapsTrackerError::DatabaseError)?;
                } else {
                    // TODO: add logs but it shouldn't really happen
                }
            },
            #[expect(
                clippy::unreachable,
                reason = "pre-existing panic site, grandfathered when the panic gate landed; see the panic-gate backlog in docs/conventions.md"
            )]
            TransactionOriginVariant::InternalTransfer(_) => unreachable!(),
            TransactionOriginVariant::None => {},
        }

        dao_transaction
            .commit()
            .await
            .map_err(|_| SwapsTrackerError::DatabaseError)?;

        self.store.remove_swap(swap.id);
        // it's expected and "normal" behaviour, so just `info` record
        // TODO: update message?;
        tracing::info!("Swap has failed while executing and has been refunded");

        Ok(())
    }

    #[tracing::instrument(skip_all, fields(swap_id = %swap.id, invoice_id = %swap.request.invoice_id))]
    async fn check_swap(
        &mut self,
        swap: &Swap,
    ) -> Result<(), SwapsTrackerError> {
        // TODO: match over errors, for some of them we should mark swap as failed
        // immediately like transaction hash is not set
        let status = self
            .clients
            .get_transaction_status(
                swap.request.swap_executor,
                &swap.swap_details,
            )
            .await?;

        match status {
            ExecutorSwapStatus::Pending => {
                tracing::trace!("Swap still has pending status, keep watching")
            },
            ExecutorSwapStatus::Executed => {
                self.handle_swap_executed(swap).await?;
            },
            ExecutorSwapStatus::Failed => {
                self.handle_swap_failed(swap).await?;
            },
        }

        Ok(())
    }

    /// Re-read a tracked swap that has no transaction hash yet and refresh the
    /// stored copy.
    ///
    /// The database poll picks swaps up every 100 ms, which can be well before
    /// `submit_with_signature` has attached the hash of a submission it is
    /// still waiting on. Without this refresh the tracked clone stays hashless
    /// for the lifetime of the process and every poll below fails with
    /// `TransactionHashIsNotSet`.
    ///
    /// Returns `None` when the swap still can't be polled this round.
    async fn refresh_hashless_swap(
        &mut self,
        swap: &Swap,
    ) -> Option<Swap> {
        let reload_result = self.dao.get_swap_by_id(swap.id).await;
        apply_hashless_swap_reload(&mut self.store, swap, reload_result)
    }

    async fn check_swaps(&mut self) {
        let swaps = self.store.get_all_swaps();

        for swap in swaps {
            let swap = if swap
                .swap_details
                .transaction_hash
                .is_none()
            {
                match self.refresh_hashless_swap(&swap).await {
                    Some(refreshed) => refreshed,
                    None => continue,
                }
            } else {
                swap
            };

            let result = self.check_swap(&swap).await;

            if let Err(e) = result {
                tracing::debug!(swap_id = %swap.id, invoice_id = %swap.request.invoice_id, error = ?e, "Got an error while checking swap");
            }
        }
    }

    async fn get_submitted_swaps(&mut self) {
        match self.dao.get_submitted_swaps().await {
            Ok(swaps) => {
                if !swaps.is_empty() {
                    let swaps_count = swaps.len();
                    self.store.add_swaps(swaps);
                    tracing::info!(%swaps_count, "Added submitted swaps for tracking");
                }
            },
            Err(e) => tracing::warn!(
                error = ?e,
                "Error while fetching submitted swaps for monitoring"
            ),
        };
    }

    async fn get_outdated_swaps(&mut self) {
        match self.dao.get_outdated_swaps().await {
            Ok(swaps) => {
                if !swaps.is_empty() {
                    let swaps_count = swaps.len();
                    tracing::info!(%swaps_count, "Marked swaps as abandoned");
                }
            },
            Err(e) => tracing::warn!(
                error = ?e,
                "Error while markind swaps abandoned"
            ),
        }
    }

    async fn perform(
        mut self,
        token: CancellationToken,
    ) {
        tracing::info!("Starting swaps tracker");

        // Pending swaps left over from a service reload must be reloaded before
        // anything else: without them the daemon accepts new swaps while
        // silently never monitoring the ones already submitted.
        //
        // A transient database error should not take the daemon down, so retry.
        // But the retry must be bounded: the previous `unwrap()` here reached
        // the global panic hook installed in `main`, which cancels the shutdown
        // token and stops the daemon, so an unbounded retry would *weaken* the
        // failure handling — a permanently undecodable row would leave the API
        // serving forever with no tracker and no operator signal. On exhaustion,
        // fall back to that same shutdown path.
        let mut pending_swaps = None;

        for attempt in 1..=SWAPS_EXECUTOR_PENDING_RELOAD_MAX_ATTEMPTS {
            // Race the query itself, not just the backoff: a shutdown requested
            // while the call is stalled on a connection must not wait it out.
            let result = tokio::select! {
                result = self.dao.get_pending_swaps() => result,
                () = token.cancelled() => return,
            };

            match result {
                Ok(swaps) => {
                    pending_swaps = Some(swaps);
                    break
                },
                Err(e) => {
                    tracing::warn!(
                        error.category = category::SWAPS_TRACKER,
                        error.operation = operation::RELOAD_PENDING_SWAPS,
                        error.source = ?e,
                        %attempt,
                        max_attempts = SWAPS_EXECUTOR_PENDING_RELOAD_MAX_ATTEMPTS,
                        "Failed to load pending swaps on startup, retrying"
                    );

                    tokio::select! {
                        () = token.cancelled() => return,
                        () = tokio::time::sleep(Duration::from_millis(
                            SWAPS_EXECUTOR_PENDING_RELOAD_RETRY_MILLIS,
                        )) => {}
                    }
                },
            }
        }

        let Some(pending_swaps) = pending_swaps else {
            tracing::error!(
                error.category = category::SWAPS_TRACKER,
                error.operation = operation::RELOAD_PENDING_SWAPS,
                max_attempts = SWAPS_EXECUTOR_PENDING_RELOAD_MAX_ATTEMPTS,
                "Could not load pending swaps, shutting down rather than \
                 serving without a swaps tracker"
            );

            token.cancel();

            return
        };

        self.store.add_swaps(pending_swaps);

        // Created after the reload on purpose: tokio's default missed-tick
        // behaviour is `Burst`, so intervals built before a slow retry would
        // fire their accumulated backlog back-to-back and hammer both SQLite
        // and the provider APIs on the way out of startup.
        let mut api_polling_interval = interval(Duration::from_millis(
            SWAPS_EXECUTOR_API_POLLING_INTERVAL_MILLIS,
        ));

        let mut database_polling_interval = interval(Duration::from_millis(
            SWAPS_EXECUTOR_DATABASE_POLLING_INTERVAL_MILLIS,
        ));

        loop {
            tokio::select! {
                _ = api_polling_interval.tick(), if self.store.has_any_swaps() => {
                    self.check_swaps().await;
                },
                _ = database_polling_interval.tick() => {
                    // TODO: also fetch swaps which has valid_till < now and are still active
                    self.get_submitted_swaps().await;
                    self.get_outdated_swaps().await;
                },
                () = token.cancelled() => {
                    tracing::info!(
                        "Swaps executor received shutdown signal, shutting down immediately"
                    );

                    break
                }
            }
        }
    }

    pub fn ignite(
        self,
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.perform(token).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::dao::DaoSwapError;
    use crate::types::{
        SwapStatus,
        default_swap,
    };

    use super::*;

    #[test]
    fn terminal_hashless_swap_is_dropped_after_reload() {
        let mut swap = default_swap(Uuid::new_v4());
        swap.status = SwapStatus::Submitted;

        let mut failed = swap.clone();
        failed.status = SwapStatus::Failed;

        let mut store = TrackedSwaps::new();
        store.add_swaps(vec![swap.clone()]);

        assert!(apply_hashless_swap_reload(&mut store, &swap, Ok(Some(failed))).is_none());
        assert!(!store.swaps.contains_key(&swap.id));
    }

    #[test]
    fn transient_hashless_swap_states_remain_tracked_for_retry() {
        let mut swap = default_swap(Uuid::new_v4());
        swap.status = SwapStatus::Submitted;

        let mut store = TrackedSwaps::new();
        store.add_swaps(vec![swap.clone()]);

        assert!(
            apply_hashless_swap_reload(
                &mut store,
                &swap,
                Ok(Some(swap.clone()))
            )
            .is_none()
        );
        assert!(store.swaps.contains_key(&swap.id));

        assert!(
            apply_hashless_swap_reload(
                &mut store,
                &swap,
                Err(DaoSwapError::DatabaseError)
            )
            .is_none()
        );
        assert!(store.swaps.contains_key(&swap.id));
    }
}
