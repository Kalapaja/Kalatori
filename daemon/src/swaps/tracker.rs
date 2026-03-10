use std::collections::HashMap;
use std::time::Duration;

use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::clients::{
    AcrossClient,
    AcrossSwapStatus,
    BungeeClient,
    BungeeSwapStatus,
};
use crate::dao::DaoInterface;
use crate::types::{
    InternalSwapDetails,
    Swap,
    SwapExecutorType,
};

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
    across_client: AcrossClient,
    bungee_client: BungeeClient,
}

#[derive(Debug)]
pub enum SwapsTrackerError {
    ApiError,
    DatabaseError,
}

impl<D: DaoInterface + 'static> SwapsTracker<D> {
    pub fn new(dao: D) -> Self {
        // TODO: create in main, share with Executor
        let across_client = AcrossClient::new();
        let bungee_client = BungeeClient::new();

        Self {
            dao,
            across_client,
            bungee_client,
            store: TrackedSwaps::new(),
        }
    }

    #[tracing::instrument(skip_all, fields(swap_id = %swap.id, invoice_id = %swap.request.invoice_id))]
    async fn check_across_swap(
        &mut self,
        swap: &Swap,
    ) -> Result<(), SwapsTrackerError> {
        tracing::trace!("Check across swap");

        let InternalSwapDetails::Across(details) = &swap.swap_details else {
            tracing::error!("Unexpected internal swap details. Expected Across");
            return Ok(())
        };

        if let Some(tx_hash) = details.transaction_hash.as_ref() {
            let result = self
                .across_client
                .get_swap_status(tx_hash.as_str().into())
                .await
                // TODO: check errors specifically?
                .map_err(|_| SwapsTrackerError::ApiError)?;

            match result.status {
                AcrossSwapStatus::Expired => {
                    // shouldn't really happen as long as it already has been sent but just in case
                    self.dao
                        .update_swap_failed(
                            swap.id,
                            "Got bungee status code expired or cancelled".to_string(),
                        )
                        .await
                        .map_err(|_| SwapsTrackerError::DatabaseError)?;

                    self.store.remove_swap(swap.id);

                    tracing::info!(
                        "Got across status code expired. Swap has been marked as failed and will no longer be tracked"
                    );
                },
                AcrossSwapStatus::Pending => {
                    tracing::trace!("Swap still has pending status, keep watching")
                },
                AcrossSwapStatus::Filled => {
                    self.dao
                        .update_swap_completed(swap.id)
                        .await
                        .map_err(|_| SwapsTrackerError::DatabaseError)?;

                    self.store.remove_swap(swap.id);
                    tracing::info!("Swap has been filled and marked as completed in the database")
                },
                AcrossSwapStatus::Refunded => {
                    self.dao
                        .update_swap_failed(
                            swap.id,
                            "Swap has been failed and refunded".to_string(),
                        )
                        .await
                        .map_err(|_| SwapsTrackerError::DatabaseError)?;

                    self.store.remove_swap(swap.id);
                    // it's expected and "normal" behaviour, so just `info` record
                    tracing::info!("Swap has failed while executing and has been refunded")
                },
            }
        } else {
            self.dao
                .update_swap_failed(
                    swap.id,
                    "No transaction hash saved".to_string(),
                )
                .await
                .map_err(|_| SwapsTrackerError::DatabaseError)?;

            tracing::error!(
                "Across swap has been marked as sent but transaction hash is empty.
                It has been marked as failed and will no longer be tracked"
            );
        }

        Ok(())
    }

    #[tracing::instrument(skip_all, fields(swap_id = %swap.id, invoice_id = %swap.request.invoice_id))]
    async fn check_bungee_swap(
        &mut self,
        swap: &Swap,
    ) -> Result<(), SwapsTrackerError> {
        tracing::trace!("Check bungee swap");

        let InternalSwapDetails::Bungee(details) = &swap.swap_details else {
            tracing::error!("Unexpected internal swap details. Expected Bungee");
            return Ok(())
        };

        if let Some(tx_hash) = details.transaction_hash.as_ref() {
            let result = self
                .bungee_client
                .get_swap_status(tx_hash.as_str().into())
                .await
                // TODO: check errors specifically?
                .map_err(|_| SwapsTrackerError::ApiError)?;

            let Some(trans) = result.first() else {
                // TODO: perhaps mark as failed
                tracing::warn!("Bungee API returned empty swap status");
                return Err(SwapsTrackerError::ApiError)
            };

            match trans.bungee_status_code {
                BungeeSwapStatus::Expired | BungeeSwapStatus::Cancelled => {
                    // shouldn't really happen as long as it already has been sent but just in case
                    self.dao
                        .update_swap_failed(
                            swap.id,
                            "Got bungee status code expired or cancelled".to_string(),
                        )
                        .await
                        .map_err(|_| SwapsTrackerError::DatabaseError)?;

                    self.store.remove_swap(swap.id);

                    tracing::info!(
                        status_code = ?trans.bungee_status_code,
                        "Got bungee status code expired or cancelled. Swap has been marked as failed and will no longer be tracked"
                    );
                },
                BungeeSwapStatus::Pending
                | BungeeSwapStatus::Assigned
                | BungeeSwapStatus::Extracted => {
                    tracing::trace!("Swap still has pending status, keep watching")
                },
                // According to the docs, both settled and fulfilled statuses are final
                BungeeSwapStatus::Settled | BungeeSwapStatus::Fulfilled => {
                    self.dao
                        .update_swap_completed(swap.id)
                        .await
                        .map_err(|_| SwapsTrackerError::DatabaseError)?;

                    self.store.remove_swap(swap.id);
                    tracing::info!("Swap has been filled and marked as completed in the database")
                },
                BungeeSwapStatus::Refunded => {
                    self.dao
                        .update_swap_failed(
                            swap.id,
                            "Swap has been failed and refunded".to_string(),
                        )
                        .await
                        .map_err(|_| SwapsTrackerError::DatabaseError)?;

                    self.store.remove_swap(swap.id);
                    // it's expected and "normal" behaviour, so just `info` record
                    tracing::info!("Swap has failed while executing and has been refunded")
                },
            }
        } else {
            self.dao
                .update_swap_failed(
                    swap.id,
                    "No transaction hash saved".to_string(),
                )
                .await
                .map_err(|_| SwapsTrackerError::DatabaseError)?;

            tracing::error!(
                "Bungee swap has been marked as sent but transaction hash is empty.
                It has been marked as failed and will no longer be tracked"
            );
        }

        Ok(())
    }

    async fn check_swaps(&mut self) {
        let swaps = self.store.get_all_swaps();

        for swap in swaps {
            let result = match swap.request.swap_executor {
                SwapExecutorType::Across => self.check_across_swap(&swap).await,
                SwapExecutorType::Bungee => self.check_bungee_swap(&swap).await,
            };

            if let Err(e) = result {
                tracing::debug!(swap_id = %swap.id, invoice_id = %swap.request.invoice_id, error = ?e, "Got an error while checking swap");
            }
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

        // First of all need to fetch pending swaps which has left after service realod.
        // Need to either handle an error and retry loading or just panic and restart
        // the daemon, we can't just leave those pending swaps in this state
        // forever.
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
