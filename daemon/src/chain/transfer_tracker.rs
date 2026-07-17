use std::str::FromStr;
use std::time::Duration;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::chain_client::{
    BlockChainClient,
    ChainConfig,
    ChainTransfer,
    GeneralChainTransfer,
    SubscriptionError,
    TransfersStream,
};
use crate::dao::DaoInterface;
use crate::types::IncomingTransaction;

use super::{
    InvoiceRegistry,
    TransactionsRecorder,
    TransactionsRecorderError,
};

const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
const DEGRADED_WARNING_INTERVAL: Duration = Duration::from_secs(60);

struct RetryState {
    delay: Duration,
    degraded_since: Option<tokio::time::Instant>,
    last_warning: Option<tokio::time::Instant>,
    attempts: u64,
}

impl RetryState {
    fn new() -> Self {
        Self {
            delay: INITIAL_RETRY_DELAY,
            degraded_since: None,
            last_warning: None,
            attempts: 0,
        }
    }

    fn record_failure(&mut self) -> Duration {
        self.record_failure_at(tokio::time::Instant::now())
    }

    fn record_failure_at(
        &mut self,
        now: tokio::time::Instant,
    ) -> Duration {
        let degraded_since = *self.degraded_since.get_or_insert(now);
        self.attempts = self.attempts.saturating_add(1);

        let should_warn = self
            .last_warning
            .is_none_or(|last_warning| {
                now.duration_since(last_warning) >= DEGRADED_WARNING_INTERVAL
            });
        if should_warn {
            tracing::warn!(
                failed_attempts = self.attempts,
                degraded_for_seconds = now
                    .duration_since(degraded_since)
                    .as_secs(),
                next_retry_seconds = self.delay.as_secs(),
                "Transfer tracking is degraded; retrying with backoff"
            );
            self.last_warning = Some(now);
        }

        let delay = self.delay;
        self.delay = self
            .delay
            .saturating_mul(2)
            .min(MAX_RETRY_DELAY);
        delay
    }

    fn record_health(&mut self) {
        self.record_health_at(tokio::time::Instant::now());
    }

    fn record_health_at(
        &mut self,
        now: tokio::time::Instant,
    ) {
        let Some(degraded_since) = self.degraded_since.take() else {
            return;
        };

        tracing::info!(
            failed_attempts = self.attempts,
            outage_seconds = now
                .duration_since(degraded_since)
                .as_secs(),
            "Transfer tracking recovered"
        );
        self.delay = INITIAL_RETRY_DELAY;
        self.last_warning = None;
        self.attempts = 0;
    }
}

pub struct TransfersTracker<
    T: ChainConfig,
    C: BlockChainClient<T> + 'static,
    D: DaoInterface + 'static,
> {
    client: C,
    registry: InvoiceRegistry,
    transactions_recorder: TransactionsRecorder<D>,
    phantom: std::marker::PhantomData<T>,
}

impl<T: ChainConfig, C: BlockChainClient<T> + 'static, D: DaoInterface + 'static>
    TransfersTracker<T, C, D>
{
    pub fn new(
        client: C,
        registry: InvoiceRegistry,
        transactions_recorder: TransactionsRecorder<D>,
    ) -> Self {
        TransfersTracker {
            client,
            registry,
            transactions_recorder,
            phantom: std::marker::PhantomData,
        }
    }

    async fn get_or_create_subscription(
        &self,
        subscription: Option<TransfersStream<T>>,
        asset_ids: &[T::AssetId],
    ) -> Option<TransfersStream<T>> {
        if subscription.is_some() {
            return subscription;
        }

        self.client
            .subscribe_transfers(asset_ids)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = "transfer_tracker",
                    error.operation = "get_or_create_subscription",
                    error.source = ?e,
                    "Error subscribing to transfer events"
                );
            })
            .ok()
    }

    #[tracing::instrument(skip(self))]
    async fn process_transfer(
        &self,
        transfer: GeneralChainTransfer,
    ) {
        if let Some(mut invoice) = self
            .registry
            .find_invoice_by_address(
                &transfer.recipient,
                transfer.chain,
                &transfer.asset_id,
            )
            .await
        {
            let invoice_id = invoice.invoice.id;

            tracing::info!(
                %invoice_id,
                "Processing incoming transfer for invoice"
            );

            let transaction = IncomingTransaction::from_chain_transfer(invoice_id, transfer);

            match self
                .transactions_recorder
                .process_invoice_transaction(&mut invoice, transaction)
                .await
            {
                Ok(()) => tracing::info!(
                    %invoice_id,
                    invoice_status = %invoice.invoice.status,
                    total_received_amount = %invoice.total_received_amount,
                    "Transfer has been stored in database successfully, invoice has been updated"
                ),
                Err(TransactionsRecorderError::TransactionDuplication {
                    ..
                }) => tracing::info!(
                    %invoice_id,
                    "Transfer is already presented in database, invoice hasn't been updated"
                ),
                Err(e) => tracing::warn!(
                    %invoice_id,
                    error = ?e,
                    "Error while trying to store transfer in database, invoice hasn't been updated"
                ),
            };
        }
    }

    async fn handle_subscription_event(
        &self,
        event: Option<Result<Vec<ChainTransfer<T>>, SubscriptionError>>,
    ) -> Result<(), SubscriptionError> {
        match event {
            Some(Ok(transfers)) => {
                for transfer in transfers {
                    self.process_transfer(transfer.into())
                        .await;
                }

                Ok(())
            },
            Some(Err(e)) => {
                tracing::debug!(
                    error.category = "transfer_tracker",
                    error.operation = "handle_subscription_event",
                    error.source = ?e,
                    "Error receiving transfer event"
                );
                Err(e)
            },
            None => {
                tracing::debug!("Transfer event subscription ended");
                Err(SubscriptionError::StreamClosed)
            },
        }
    }

    #[tracing::instrument(skip(self, token), fields(chain = %T::CHAIN_TYPE))]
    async fn perform(
        mut self,
        assets: Vec<T::AssetId>,
        token: CancellationToken,
    ) {
        tracing::info!(
            "Starting transfers tracker for {}",
            self.client.chain_name()
        );

        let mut subscription = None;
        let mut retry_state = RetryState::new();

        loop {
            subscription = self
                .get_or_create_subscription(subscription, &assets)
                .await;

            let Some(poll_subscription) = &mut subscription else {
                tracing::debug!("Failed to establish transfer subscription; recreating client");
                // If we couldn't create a subscription, try to recreate the client with another
                // RPC endpoint
                match self.client.recreate().await {
                    Ok(new_client) => {
                        self.client = new_client;

                        tracing::debug!(
                            "Recreated blockchain client for {} with new RPC endpoint",
                            self.client.chain_name()
                        );
                    },
                    Err(e) => {
                        tracing::debug!(
                            error.category = "transfer_tracker",
                            error.operation = "perform",
                            error.source = ?e,
                            "Error recreating blockchain client"
                        );
                    },
                }

                let retry_delay = retry_state.record_failure();
                tokio::select! {
                    () = tokio::time::sleep(retry_delay) => {},
                    () = token.cancelled() => {
                        tracing::info!(
                            "Transfers tracker received cancellation signal, shutting down"
                        );
                        break;
                    },
                }

                continue;
            };

            tokio::select! {
                subscription_event = poll_subscription.next() => {
                    match subscription_event {
                        Some(Ok(transfers)) => {
                            retry_state.record_health();
                            let _result = self
                                .handle_subscription_event(Some(Ok(transfers)))
                                .await;
                        },
                        failed_event => {
                            let _result = self.handle_subscription_event(failed_event).await;
                            subscription = None;
                            let retry_delay = retry_state.record_failure();
                            tokio::select! {
                                () = tokio::time::sleep(retry_delay) => {},
                                () = token.cancelled() => {
                                    tracing::info!(
                                        "Transfers tracker received cancellation signal, shutting down"
                                    );
                                    break;
                                },
                            }
                        },
                    }
                },
                () = token.cancelled() => {
                    tracing::info!(
                        "Transfers tracker received cancellation signal, shutting down"
                    );
                    break;
                },
            }
        }
    }

    pub fn ignite(
        self,
        assets: &[String],
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        // TODO: handle invalid asset IDs, though they shouldn't happen in practice
        let assets = assets
            .iter()
            .filter_map(|asset_id| T::AssetId::from_str(asset_id)
                .inspect_err(|_e| {
                    tracing::error!(
                        // TODO: add error, it should implement either debug or display
                        chain = %T::CHAIN_TYPE,
                        %asset_id,
                        "Error while trying to parse asset id `{}` for {} chain tracker, it will be skipped",
                        asset_id,
                        T::CHAIN_TYPE
                    )
                })
                .ok()
            )
            .collect();

        tokio::spawn(async move {
            self.perform(assets, token).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use mockall::predicate::eq;
    use rust_decimal::Decimal;

    use crate::chain_client::{
        AssetHubChainConfig,
        ClientError,
        MockBlockChainClient,
        PolygonChainConfig,
        default_general_chain_transfer,
    };
    use crate::dao::DAO;
    use crate::types::{
        ChainType,
        Invoice,
        default_invoice,
    };

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn perform_applies_backoff_between_failed_subscription_cycles() {
        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        let attempt_times = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        chain_client
            .expect_chain_name()
            .return_const("test-chain");
        let recorded_times = std::sync::Arc::clone(&attempt_times);
        chain_client
            .expect_subscribe_transfers()
            .returning(move |_| {
                recorded_times
                    .lock()
                    .unwrap()
                    .push(tokio::time::Instant::now());
                Err(SubscriptionError::SubscriptionFailed)
            });
        chain_client
            .expect_recreate()
            .returning(|| Err(ClientError::AllEndpointsUnreachable));

        let registry = InvoiceRegistry::new();
        let recorder = TransactionsRecorder::<DAO>::default();
        let tracker = TransfersTracker::new(chain_client, registry, recorder);

        let token = CancellationToken::new();
        let tracker_task = tokio::spawn(tracker.perform(vec![], token.clone()));

        // Delays 1+2+4+8+16+32+60+60 = 183s of virtual time -> 9 attempts
        tokio::time::sleep(Duration::from_secs(200)).await;
        token.cancel();
        tracker_task.await.unwrap();

        let attempt_times = attempt_times.lock().unwrap();
        let gaps: Vec<u64> = attempt_times
            .windows(2)
            .map(|pair| {
                pair[1]
                    .duration_since(pair[0])
                    .as_secs()
            })
            .collect();
        assert!(
            gaps.starts_with(&[1, 2, 4, 8, 16, 32, 60, 60]),
            "expected exponential gaps up to the cap, got {gaps:?}"
        );
        assert!(
            gaps.iter().all(|gap| *gap >= 1),
            "no attempt may follow the previous one without delay: {gaps:?}"
        );
    }

    #[test]
    fn persistent_failures_back_off_exponentially_to_cap() {
        let started_at = tokio::time::Instant::now();
        let mut attempted_at = started_at;
        let mut retry_state = RetryState::new();
        let expected_delays = [1, 2, 4, 8, 16, 32, 60, 60];

        for expected_delay in expected_delays {
            let delay = retry_state.record_failure_at(attempted_at);
            assert_eq!(
                delay,
                Duration::from_secs(expected_delay)
            );
            attempted_at += delay;
        }

        assert_eq!(
            retry_state.attempts,
            expected_delays.len() as u64
        );
        assert_eq!(
            attempted_at.duration_since(started_at),
            Duration::from_secs(183)
        );
    }

    #[test]
    #[tracing_test::traced_test]
    fn degraded_warnings_are_rate_limited() {
        let started_at = tokio::time::Instant::now();
        let mut retry_state = RetryState::new();

        retry_state.record_failure_at(started_at);
        retry_state.record_failure_at(started_at + Duration::from_secs(59));
        retry_state.record_failure_at(started_at + Duration::from_secs(60));

        logs_assert(|logs| {
            let warning_count = logs
                .iter()
                .filter(|log| {
                    log.contains(" WARN ")
                        && log.contains("Transfer tracking is degraded; retrying with backoff")
                })
                .count();
            if warning_count == 2 {
                Ok(())
            } else {
                Err(format!(
                    "expected 2 degraded warnings, found {warning_count}"
                ))
            }
        });
    }

    #[test]
    #[tracing_test::traced_test]
    fn successful_event_resets_backoff_after_recovery() {
        let started_at = tokio::time::Instant::now();
        let mut retry_state = RetryState::new();

        assert_eq!(
            retry_state.record_failure_at(started_at),
            Duration::from_secs(1)
        );
        assert_eq!(
            retry_state.record_failure_at(started_at),
            Duration::from_secs(2)
        );
        retry_state.record_health_at(started_at + Duration::from_secs(10));

        assert_eq!(
            retry_state.record_failure_at(started_at + Duration::from_secs(10)),
            Duration::from_secs(1)
        );
        assert!(logs_contain(
            "Transfer tracking recovered"
        ));
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_process_transfer() {
        // As long as this function doesn't return any result,
        // we can check log records to ensure the code is following
        // expected flows
        let chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        let registry = InvoiceRegistry::new();
        let recorder = TransactionsRecorder::<DAO>::default();
        let mut tracker = TransfersTracker::new(chain_client, registry.clone(), recorder);

        // Test case 1:
        // - No invoices with related address
        // - Expectations:
        //   - No recorder calls
        let transfer = default_general_chain_transfer();

        tracker.process_transfer(transfer).await;
        tracker
            .transactions_recorder
            .checkpoint();
        assert!(!logs_contain(
            "Transfer has been stored in database successfully, invoice has been updated"
        ));
        assert!(!logs_contain(
            "Transfer is already presented in database, invoice hasn't been updated"
        ));
        assert!(!logs_contain(
            "Error while trying to store transfer in database, invoice hasn't been updated"
        ));

        // Test case 2:
        // - Successful flow
        // - Invoice with related address exists in registry
        // - Expectations:
        //   - Recorded called and respond success
        //   - Respective log record
        let invoice = default_invoice().with_amount(Decimal::ZERO);
        let invoice_id = invoice.invoice.id;
        registry
            .add_invoice(invoice.clone())
            .await;

        let transfer = GeneralChainTransfer {
            recipient: invoice.invoice.payment_address.clone(),
            ..default_general_chain_transfer()
        };

        let expected_transaction =
            IncomingTransaction::from_chain_transfer(invoice_id, transfer.clone());

        tracker
            .transactions_recorder
            .expect_process_invoice_transaction()
            .with(
                eq(invoice.clone()),
                eq(expected_transaction.clone()),
            )
            .once()
            .returning(|_, _| Ok(()));

        tracker
            .process_transfer(transfer.clone())
            .await;
        tracker
            .transactions_recorder
            .checkpoint();
        assert!(logs_contain(
            "Transfer has been stored in database successfully, invoice has been updated"
        ));
        assert!(!logs_contain(
            "Transfer is already presented in database, invoice hasn't been updated"
        ));
        assert!(!logs_contain(
            "Error while trying to store transfer in database, invoice hasn't been updated"
        ));

        // Test case 3:
        // - Duplicated transaction error
        // - Invoice with related address exists in registry
        // - Expectations:
        //   - Recorded called and respond duplication error
        //   - Respective log record
        tracker
            .transactions_recorder
            .expect_process_invoice_transaction()
            .with(
                eq(invoice.clone()),
                eq(expected_transaction.clone()),
            )
            .once()
            .returning(|_invoice, transaction| {
                Err(
                    TransactionsRecorderError::TransactionDuplication {
                        chain: transaction.transfer_info.chain,
                        general_transaction_id: transaction.transaction_id,
                    },
                )
            });

        tracker
            .process_transfer(transfer.clone())
            .await;
        tracker
            .transactions_recorder
            .checkpoint();
        assert!(logs_contain(
            "Transfer is already presented in database, invoice hasn't been updated"
        ));
        assert!(!logs_contain(
            "Error while trying to store transfer in database, invoice hasn't been updated"
        ));

        // Test case 4:
        // - Database error
        // - Invoice with related address exists in registry
        // - Expectations:
        //   - Recorded called and respond duplication error
        //   - Respective log record
        tracker
            .transactions_recorder
            .expect_process_invoice_transaction()
            .with(
                eq(invoice),
                eq(expected_transaction.clone()),
            )
            .once()
            .returning(|_, _| Err(TransactionsRecorderError::DaoTransactionError));

        tracker.process_transfer(transfer).await;
        tracker
            .transactions_recorder
            .checkpoint();
        assert!(logs_contain(
            "Error while trying to store transfer in database, invoice hasn't been updated"
        ));
    }

    #[tokio::test]
    async fn test_handle_subscription_event() {
        let chain_client = MockBlockChainClient::<AssetHubChainConfig>::default();
        let registry = InvoiceRegistry::new();
        let recorder = TransactionsRecorder::<DAO>::default();
        let mut tracker = TransfersTracker::new(chain_client, registry.clone(), recorder);

        // Test case 1:
        // - Successful case
        // - Vec with transactions input
        // - Expectations:
        //   - Transfers input
        //   - Ok result
        let transfer = ChainTransfer::<AssetHubChainConfig> {
            asset_id: 1984,
            asset_name: "USDt".to_string(),
            amount: Decimal::TEN,
            sender: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
                .parse()
                .unwrap(),
            recipient: "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"
                .parse()
                .unwrap(),
            transaction_id: (1000, 2),
            timestamp: 1000,
        };

        let transfers = vec![transfer.clone(), transfer.clone(), transfer.clone()];

        let invoice = Invoice {
            payment_address: "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string(),
            chain: ChainType::PolkadotAssetHub,
            asset_id: 1984.to_string(),
            ..default_invoice()
        }
        .with_amount(Decimal::ZERO);

        registry.add_invoice(invoice).await;

        tracker
            .transactions_recorder
            .expect_process_invoice_transaction()
            .times(transfers.len())
            .returning(|_, _| Ok(()));

        let result = tracker
            .handle_subscription_event(Some(Ok(transfers)))
            .await;
        assert_eq!(result, Ok(()));

        // Test case 2:
        // - Unsuccessful case
        // - None input
        // - Expectations:
        //   - Err result
        //   - StreamClosed error
        let result = tracker
            .handle_subscription_event(None)
            .await;
        assert_eq!(
            result,
            Err(SubscriptionError::StreamClosed)
        );

        // Test case 3:
        // - Unsuccessful case
        // - Error input
        // - Expectations:
        //   - Err result
        //   - Provided error returned
        let result = tracker
            .handle_subscription_event(Some(Err(
                SubscriptionError::SubscriptionFailed,
            )))
            .await;
        assert_eq!(
            result,
            Err(SubscriptionError::SubscriptionFailed)
        );
    }
}
