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
    DaoTransactionInterface,
};
use crate::types::{
    PayoutStatus,
    RefundStatus,
    Swap,
    TransactionOriginVariant,
};

use super::SwapsClients;

const SWAPS_EXECUTOR_API_POLLING_INTERVAL_MILLIS: u64 = 3000;
const SWAPS_EXECUTOR_DATABASE_POLLING_INTERVAL_MILLIS: u64 = 100;

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

    async fn check_swaps(&mut self) {
        let swaps = self.store.get_all_swaps();

        for swap in swaps {
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

        let mut api_polling_interval = interval(Duration::from_millis(
            SWAPS_EXECUTOR_API_POLLING_INTERVAL_MILLIS,
        ));

        let mut database_polling_interval = interval(Duration::from_millis(
            SWAPS_EXECUTOR_DATABASE_POLLING_INTERVAL_MILLIS,
        ));

        // TODO: First of all need to fetch pending swaps which has left after service
        // reaload. Need to either handle an error and retry loading or just
        // panic and restart the daemon, we can't just leave those pending swaps
        // in this state forever.
        let pending_swaps = self
            .dao
            .get_pending_swaps()
            .await
            .unwrap();

        self.store.add_swaps(pending_swaps);

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
