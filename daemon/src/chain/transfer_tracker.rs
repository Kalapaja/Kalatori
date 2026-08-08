use std::str::FromStr;
use std::time::Duration;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::chain_client::{
    BackfillError,
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

/// How often the catch-up sweep re-reads what the subscription may have missed
/// (issue #333). Every tick costs one `eth_getLogs` per chain, so this is also
/// the knob that decides whether a free public endpoint can carry a daemon.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Widest range a single sweep asks for. Providers cap `eth_getLogs` spans, and
/// after a long outage the gap can be arbitrarily large; the cursor makes the
/// remainder the next tick's problem.
const MAX_SWEEP_RANGE: u64 = 1_000;

/// Whether the catch-up sweep is running for this chain.
struct SweepState {
    enabled: bool,
}

impl SweepState {
    fn new() -> Self {
        Self {
            enabled: true,
        }
    }

    /// Stop sweeping this chain, and say so once. A chain whose client cannot
    /// re-read past blocks keeps the gap described in #333, and a silent skip
    /// would leave a clean log reading as "nothing to recover here".
    fn disable(
        &mut self,
        chain: crate::types::ChainType,
    ) {
        if !self.enabled {
            return;
        }

        tracing::warn!(
            %chain,
            "Chain client cannot re-read past blocks, so transfers missed while the subscription is down are recovered only by the balance check at invoice expiry"
        );
        self.enabled = false;
    }
}

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
    /// Owned separately from the recorder: the sweep cursor is the tracker's
    /// own progress, not a property of any transaction it records.
    dao: D,
    phantom: std::marker::PhantomData<T>,
}

impl<T: ChainConfig, C: BlockChainClient<T> + 'static, D: DaoInterface + 'static>
    TransfersTracker<T, C, D>
{
    pub fn new(
        client: C,
        registry: InvoiceRegistry,
        transactions_recorder: TransactionsRecorder<D>,
        dao: D,
    ) -> Self {
        TransfersTracker {
            client,
            registry,
            transactions_recorder,
            dao,
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

    /// Record a transfer against its invoice, if any invoice matches.
    ///
    /// A transfer nobody is waiting for, and a transfer already in the
    /// database, are both `Ok`: nothing is left to do. Only a failed write is
    /// an error, and only the sweep acts on it — it must not move its cursor
    /// past a transfer that did not land.
    #[tracing::instrument(skip(self))]
    async fn process_transfer(
        &self,
        transfer: GeneralChainTransfer,
    ) -> Result<(), TransactionsRecorderError> {
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
                Err(e) => {
                    tracing::warn!(
                        %invoice_id,
                        error = ?e,
                        "Error while trying to store transfer in database, invoice hasn't been updated"
                    );

                    return Err(e);
                },
            };
        }

        Ok(())
    }

    async fn handle_subscription_event(
        &self,
        event: Option<Result<Vec<ChainTransfer<T>>, SubscriptionError>>,
    ) -> Result<(), SubscriptionError> {
        match event {
            Some(Ok(transfers)) => {
                for transfer in transfers {
                    // The live path has nowhere to retry to: the event is gone
                    // once handled. The sweep is what recovers a failed write.
                    let _result = self
                        .process_transfer(transfer.into())
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

    /// Re-read the blocks between the stored cursor and the chain head, and
    /// record whatever the live subscription did not deliver (issue #333).
    ///
    /// Runs off the subscription entirely: alloy reconnects and resubscribes
    /// underneath us without the stream ever ending, so there is no moment we
    /// could hook "the subscription just came back" onto. Polling is the only
    /// signal that does not depend on noticing the gap.
    async fn sweep(
        &self,
        assets: &[T::AssetId],
        state: &mut SweepState,
    ) {
        let chain = T::CHAIN_TYPE;

        let head = match self
            .client
            .latest_confirmed_block()
            .await
        {
            Ok(head) => head,
            Err(BackfillError::Unsupported) => {
                state.disable(chain);
                return;
            },
            Err(BackfillError::RequestFailed) => {
                tracing::debug!(
                    %chain,
                    "Could not read the chain head for the catch-up sweep, retrying next tick"
                );
                return;
            },
        };

        let cursor = match self
            .dao
            .get_chain_sync_cursor(chain)
            .await
        {
            Ok(cursor) => cursor,
            Err(e) => {
                tracing::warn!(
                    %chain,
                    error = ?e,
                    "Could not read the sweep cursor, skipping this catch-up sweep"
                );
                return;
            },
        };

        let Some(cursor) = cursor else {
            // First run against this database. Starting from the head rather
            // than from genesis: the sweep exists to close gaps in this
            // daemon's own tracking, and invoices older than it are covered by
            // the balance check.
            self.store_cursor(chain, head).await;
            tracing::info!(
                %chain,
                block_number = head,
                "Catch-up sweep starting from the current chain head"
            );
            return;
        };

        let last_processed_block = cursor.last_processed_block;

        // `head` below the cursor is not a rollback: public RPC pools answer
        // from whichever node picked up the request, and some of them lag.
        if head <= last_processed_block {
            return;
        }

        let from_block = last_processed_block.saturating_add(1);
        let to_block = head.min(
            from_block
                .saturating_add(MAX_SWEEP_RANGE)
                .saturating_sub(1),
        );

        let transfers = match self
            .client
            .fetch_transfers_in_range(assets, from_block, to_block)
            .await
        {
            Ok(transfers) => transfers,
            Err(BackfillError::Unsupported) => {
                state.disable(chain);
                return;
            },
            Err(BackfillError::RequestFailed) => {
                tracing::debug!(
                    %chain,
                    from_block,
                    to_block,
                    "Catch-up sweep could not read the block range, retrying next tick"
                );
                return;
            },
        };

        let fetched = transfers.len();
        let mut failed = 0_usize;

        for transfer in transfers {
            if self
                .process_transfer(transfer.into())
                .await
                .is_err()
            {
                failed = failed.saturating_add(1);
            }
        }

        if failed > 0 {
            // Leaving the cursor where it is re-reads this range next tick.
            // Re-delivering what did land is free: transfers are deduplicated
            // by `(chain, tx_hash)` in the database.
            tracing::warn!(
                %chain,
                from_block,
                to_block,
                failed,
                "Catch-up sweep could not record every transfer, holding the cursor to retry the range"
            );
            return;
        }

        self.store_cursor(chain, to_block).await;

        if fetched > 0 {
            tracing::info!(
                %chain,
                from_block,
                to_block,
                transfers = fetched,
                "Catch-up sweep recovered transfers the subscription did not deliver"
            );
        }
    }

    async fn store_cursor(
        &self,
        chain: crate::types::ChainType,
        block_number: u64,
    ) {
        if let Err(e) = self
            .dao
            .advance_chain_sync_cursor(chain, block_number)
            .await
        {
            // The range was processed, so nothing is lost — the next sweep
            // re-reads it against the older cursor.
            tracing::warn!(
                %chain,
                block_number,
                error = ?e,
                "Could not persist the sweep cursor, the next sweep will re-read this range"
            );
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
        let mut sweep_state = SweepState::new();
        // The first tick fires immediately, which is what establishes the
        // cursor on a fresh database before any gap can open.
        let mut sweep_ticker = tokio::time::interval(SWEEP_INTERVAL);
        sweep_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

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

                // A dead subscription is exactly when the sweep earns its keep,
                // so it keeps ticking here. The wait runs to a fixed deadline
                // rather than a plain sleep, so a sweep in the middle of it
                // cannot cut the backoff short.
                let retry_deadline = tokio::time::Instant::now() + retry_state.record_failure();
                loop {
                    tokio::select! {
                        () = tokio::time::sleep_until(retry_deadline) => break,
                        _instant = sweep_ticker.tick(), if sweep_state.enabled => {
                            self.sweep(&assets, &mut sweep_state).await;
                        },
                        () = token.cancelled() => {
                            tracing::info!(
                                "Transfers tracker received cancellation signal, shutting down"
                            );
                            return;
                        },
                    }
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
                _instant = sweep_ticker.tick(), if sweep_state.enabled => {
                    self.sweep(&assets, &mut sweep_state).await;
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
    use std::sync::{
        Arc,
        Mutex,
    };

    use futures::stream;
    use mockall::predicate::eq;
    use rust_decimal::Decimal;

    use crate::chain_client::{
        AssetHubChainConfig,
        ClientError,
        MockBlockChainClient,
        PolygonChainConfig,
        default_general_chain_transfer,
    };
    use crate::dao::MockDaoInterface;
    use crate::types::{
        ChainSyncCursor,
        ChainType,
        Invoice,
        default_invoice,
    };

    use super::*;

    fn pending_transfers_stream() -> TransfersStream<PolygonChainConfig> {
        Box::pin(stream::pending())
    }

    #[tokio::test(start_paused = true)]
    async fn perform_applies_backoff_between_failed_subscription_cycles() {
        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        let attempt_times = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        chain_client
            .expect_chain_name()
            .return_const("test-chain");
        chain_client
            .expect_latest_confirmed_block()
            .returning(|| Err(BackfillError::Unsupported));
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
        let recorder = TransactionsRecorder::<MockDaoInterface>::default();
        let tracker = TransfersTracker::new(
            chain_client,
            registry,
            recorder,
            MockDaoInterface::default(),
        );

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

    #[tokio::test(start_paused = true)]
    async fn successful_client_recreation_still_waits_before_resubscribing() {
        let subscription_attempts = Arc::new(Mutex::new(Vec::new()));
        let mut replacement_client = MockBlockChainClient::<PolygonChainConfig>::default();
        replacement_client
            .expect_chain_name()
            .return_const("replacement-chain");
        replacement_client
            .expect_latest_confirmed_block()
            .returning(|| Err(BackfillError::Unsupported));
        let replacement_attempts = Arc::clone(&subscription_attempts);
        replacement_client
            .expect_subscribe_transfers()
            .once()
            .returning(move |_| {
                replacement_attempts
                    .lock()
                    .unwrap()
                    .push(tokio::time::Instant::now());
                Ok(pending_transfers_stream())
            });

        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        chain_client
            .expect_chain_name()
            .return_const("initial-chain");
        chain_client
            .expect_latest_confirmed_block()
            .returning(|| Err(BackfillError::Unsupported));
        let initial_attempts = Arc::clone(&subscription_attempts);
        chain_client
            .expect_subscribe_transfers()
            .once()
            .returning(move |_| {
                initial_attempts
                    .lock()
                    .unwrap()
                    .push(tokio::time::Instant::now());
                Err(SubscriptionError::SubscriptionFailed)
            });
        chain_client
            .expect_recreate()
            .once()
            .return_once(move || Ok(replacement_client));

        let tracker = TransfersTracker::new(
            chain_client,
            InvoiceRegistry::new(),
            TransactionsRecorder::<MockDaoInterface>::default(),
            MockDaoInterface::default(),
        );
        let token = CancellationToken::new();
        let tracker_task = tokio::spawn(tracker.perform(vec![], token.clone()));

        tokio::task::yield_now().await;
        assert_eq!(
            subscription_attempts
                .lock()
                .unwrap()
                .len(),
            1
        );
        tokio::time::advance(INITIAL_RETRY_DELAY - Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            subscription_attempts
                .lock()
                .unwrap()
                .len(),
            1
        );

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        {
            let attempt_times = subscription_attempts.lock().unwrap();
            assert_eq!(attempt_times.len(), 2);
            assert_eq!(
                attempt_times[1].duration_since(attempt_times[0]),
                INITIAL_RETRY_DELAY
            );
        }

        token.cancel();
        tracker_task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn failed_stream_event_drops_subscription_and_backs_off() {
        assert_stream_failure_drops_subscription_and_backs_off(Box::pin(stream::iter([Err(
            SubscriptionError::SubscriptionFailed,
        )])))
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn closed_stream_drops_subscription_and_backs_off() {
        assert_stream_failure_drops_subscription_and_backs_off(Box::pin(stream::empty())).await;
    }

    async fn assert_stream_failure_drops_subscription_and_backs_off(
        first_stream: TransfersStream<PolygonChainConfig>
    ) {
        let subscription_attempts = Arc::new(Mutex::new(Vec::new()));
        let first_stream = Arc::new(Mutex::new(Some(first_stream)));
        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        chain_client
            .expect_chain_name()
            .return_const("test-chain");
        chain_client
            .expect_latest_confirmed_block()
            .returning(|| Err(BackfillError::Unsupported));
        let recorded_attempts = Arc::clone(&subscription_attempts);
        chain_client
            .expect_subscribe_transfers()
            .times(2)
            .returning(move |_| {
                recorded_attempts
                    .lock()
                    .unwrap()
                    .push(tokio::time::Instant::now());
                Ok(first_stream
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap_or_else(pending_transfers_stream))
            });

        let tracker = TransfersTracker::new(
            chain_client,
            InvoiceRegistry::new(),
            TransactionsRecorder::<MockDaoInterface>::default(),
            MockDaoInterface::default(),
        );
        let token = CancellationToken::new();
        let tracker_task = tokio::spawn(tracker.perform(vec![], token.clone()));

        tokio::task::yield_now().await;
        assert_eq!(
            subscription_attempts
                .lock()
                .unwrap()
                .len(),
            1
        );
        tokio::time::advance(INITIAL_RETRY_DELAY - Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            subscription_attempts
                .lock()
                .unwrap()
                .len(),
            1
        );

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        {
            let attempt_times = subscription_attempts.lock().unwrap();
            assert_eq!(attempt_times.len(), 2);
            assert_eq!(
                attempt_times[1].duration_since(attempt_times[0]),
                INITIAL_RETRY_DELAY
            );
        }

        token.cancel();
        tracker_task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_while_awaiting_stream_event_stops_without_retrying() {
        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        chain_client
            .expect_chain_name()
            .return_const("test-chain");
        chain_client
            .expect_latest_confirmed_block()
            .returning(|| Err(BackfillError::Unsupported));
        chain_client
            .expect_subscribe_transfers()
            .once()
            .returning(|_| Ok(pending_transfers_stream()));
        chain_client.expect_recreate().never();

        let tracker = TransfersTracker::new(
            chain_client,
            InvoiceRegistry::new(),
            TransactionsRecorder::<MockDaoInterface>::default(),
            MockDaoInterface::default(),
        );
        let token = CancellationToken::new();
        let started_at = tokio::time::Instant::now();
        let tracker_task = tokio::spawn(tracker.perform(vec![], token.clone()));

        tokio::task::yield_now().await;
        token.cancel();
        tracker_task.await.unwrap();

        assert_eq!(tokio::time::Instant::now(), started_at);
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

    #[test]
    #[tracing_test::traced_test]
    fn health_record_without_failures_preserves_initial_state() {
        let started_at = tokio::time::Instant::now();
        let mut retry_state = RetryState::new();

        retry_state.record_health_at(started_at);

        assert_eq!(retry_state.delay, INITIAL_RETRY_DELAY);
        assert_eq!(retry_state.attempts, 0);
        assert_eq!(retry_state.degraded_since, None);
        assert_eq!(retry_state.last_warning, None);
        assert!(!logs_contain(
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
        let recorder = TransactionsRecorder::<MockDaoInterface>::default();
        let mut tracker = TransfersTracker::new(
            chain_client,
            registry.clone(),
            recorder,
            MockDaoInterface::default(),
        );

        // Test case 1:
        // - No invoices with related address
        // - Expectations:
        //   - No recorder calls
        let transfer = default_general_chain_transfer();

        let _result = tracker.process_transfer(transfer).await;
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

        let _result = tracker
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

        let _result = tracker
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

        let _result = tracker.process_transfer(transfer).await;
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
        let recorder = TransactionsRecorder::<MockDaoInterface>::default();
        let mut tracker = TransfersTracker::new(
            chain_client,
            registry.clone(),
            recorder,
            MockDaoInterface::default(),
        );

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

    // ========================================================================
    // Catch-up sweep (issue #333)
    // ========================================================================

    fn sweep_tracker(
        chain_client: MockBlockChainClient<PolygonChainConfig>,
        dao: MockDaoInterface,
        registry: InvoiceRegistry,
    ) -> TransfersTracker<
        PolygonChainConfig,
        MockBlockChainClient<PolygonChainConfig>,
        MockDaoInterface,
    > {
        TransfersTracker::new(
            chain_client,
            registry,
            TransactionsRecorder::<MockDaoInterface>::default(),
            dao,
        )
    }

    fn cursor_at(block_number: u64) -> ChainSyncCursor {
        ChainSyncCursor {
            chain: ChainType::Polygon,
            last_processed_block: block_number,
            updated_at: chrono::Utc::now(),
        }
    }

    fn polygon_transfer_to(recipient: &str) -> ChainTransfer<PolygonChainConfig> {
        ChainTransfer {
            asset_id: alloy::primitives::Address::from_str(
                "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
            )
            .expect("asset address fixture parses"),
            asset_name: "USDC".to_string(),
            amount: Decimal::new(10, 0),
            sender: alloy::primitives::Address::ZERO,
            recipient: alloy::primitives::Address::from_str(recipient)
                .expect("recipient address fixture parses"),
            transaction_id: "0x1234567890abcdef".to_string(),
            timestamp: 0,
        }
    }

    #[tokio::test]
    async fn sweep_starts_from_the_head_when_no_cursor_exists() {
        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        chain_client
            .expect_latest_confirmed_block()
            .once()
            .returning(|| Ok(500));
        // Nothing to recover yet, so nothing is read.
        chain_client
            .expect_fetch_transfers_in_range()
            .never();

        let mut dao = MockDaoInterface::default();
        dao.expect_get_chain_sync_cursor()
            .once()
            .returning(|_chain| Ok(None));
        dao.expect_advance_chain_sync_cursor()
            .withf(|chain, block_number| *chain == ChainType::Polygon && *block_number == 500)
            .once()
            .returning(|_chain, block_number| Ok(cursor_at(block_number)));

        let tracker = sweep_tracker(
            chain_client,
            dao,
            InvoiceRegistry::new(),
        );
        let mut state = SweepState::new();

        tracker.sweep(&[], &mut state).await;

        assert!(state.enabled);
    }

    #[tokio::test]
    async fn sweep_reads_the_gap_and_advances_the_cursor() {
        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        chain_client
            .expect_latest_confirmed_block()
            .once()
            .returning(|| Ok(120));
        chain_client
            .expect_fetch_transfers_in_range()
            .withf(|_assets, from_block, to_block| *from_block == 101 && *to_block == 120)
            .once()
            .returning(|_assets, _from, _to| Ok(vec![]));

        let mut dao = MockDaoInterface::default();
        dao.expect_get_chain_sync_cursor()
            .once()
            .returning(|_chain| Ok(Some(cursor_at(100))));
        dao.expect_advance_chain_sync_cursor()
            .withf(|_chain, block_number| *block_number == 120)
            .once()
            .returning(|_chain, block_number| Ok(cursor_at(block_number)));

        let tracker = sweep_tracker(
            chain_client,
            dao,
            InvoiceRegistry::new(),
        );

        tracker
            .sweep(&[], &mut SweepState::new())
            .await;
    }

    #[tokio::test]
    async fn sweep_clamps_a_long_gap_to_the_range_limit() {
        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        chain_client
            .expect_latest_confirmed_block()
            .once()
            .returning(|| Ok(5_000));
        chain_client
            .expect_fetch_transfers_in_range()
            .withf(|_assets, from_block, to_block| *from_block == 1 && *to_block == MAX_SWEEP_RANGE)
            .once()
            .returning(|_assets, _from, _to| Ok(vec![]));

        let mut dao = MockDaoInterface::default();
        dao.expect_get_chain_sync_cursor()
            .once()
            .returning(|_chain| Ok(Some(cursor_at(0))));
        // The rest of the gap is the next tick's problem, so the cursor stops
        // at the end of the range actually read.
        dao.expect_advance_chain_sync_cursor()
            .withf(|_chain, block_number| *block_number == MAX_SWEEP_RANGE)
            .once()
            .returning(|_chain, block_number| Ok(cursor_at(block_number)));

        let tracker = sweep_tracker(
            chain_client,
            dao,
            InvoiceRegistry::new(),
        );

        tracker
            .sweep(&[], &mut SweepState::new())
            .await;
    }

    #[tokio::test]
    async fn sweep_ignores_a_head_behind_the_cursor() {
        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        // A lagging node in a public RPC pool answers with an older head.
        chain_client
            .expect_latest_confirmed_block()
            .once()
            .returning(|| Ok(150));
        chain_client
            .expect_fetch_transfers_in_range()
            .never();

        let mut dao = MockDaoInterface::default();
        dao.expect_get_chain_sync_cursor()
            .once()
            .returning(|_chain| Ok(Some(cursor_at(200))));
        dao.expect_advance_chain_sync_cursor()
            .never();

        let tracker = sweep_tracker(
            chain_client,
            dao,
            InvoiceRegistry::new(),
        );

        tracker
            .sweep(&[], &mut SweepState::new())
            .await;
    }

    #[tokio::test]
    async fn sweep_records_what_the_subscription_missed() {
        let invoice = default_invoice().with_amount(Decimal::ZERO);
        let registry = InvoiceRegistry::new();
        registry
            .add_invoice(invoice.clone())
            .await;
        let payment_address = invoice.invoice.payment_address.clone();

        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        chain_client
            .expect_latest_confirmed_block()
            .once()
            .returning(|| Ok(120));
        chain_client
            .expect_fetch_transfers_in_range()
            .once()
            .returning(move |_assets, _from, _to| {
                Ok(vec![polygon_transfer_to(
                    &payment_address,
                )])
            });

        let mut dao = MockDaoInterface::default();
        dao.expect_get_chain_sync_cursor()
            .once()
            .returning(|_chain| Ok(Some(cursor_at(100))));
        dao.expect_advance_chain_sync_cursor()
            .once()
            .returning(|_chain, block_number| Ok(cursor_at(block_number)));

        let mut tracker = sweep_tracker(chain_client, dao, registry);
        tracker
            .transactions_recorder
            .expect_process_invoice_transaction()
            .once()
            .returning(|_invoice, _transaction| Ok(()));

        tracker
            .sweep(&[], &mut SweepState::new())
            .await;

        tracker
            .transactions_recorder
            .checkpoint();
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn sweep_holds_the_cursor_when_a_transfer_cannot_be_recorded() {
        let invoice = default_invoice().with_amount(Decimal::ZERO);
        let registry = InvoiceRegistry::new();
        registry
            .add_invoice(invoice.clone())
            .await;
        let payment_address = invoice.invoice.payment_address.clone();

        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        chain_client
            .expect_latest_confirmed_block()
            .once()
            .returning(|| Ok(120));
        chain_client
            .expect_fetch_transfers_in_range()
            .once()
            .returning(move |_assets, _from, _to| {
                Ok(vec![polygon_transfer_to(
                    &payment_address,
                )])
            });

        let mut dao = MockDaoInterface::default();
        dao.expect_get_chain_sync_cursor()
            .once()
            .returning(|_chain| Ok(Some(cursor_at(100))));
        // Advancing here would lose the payment: the range would never be read
        // again, and nothing else re-reads it before invoice expiry.
        dao.expect_advance_chain_sync_cursor()
            .never();

        let mut tracker = sweep_tracker(chain_client, dao, registry);
        tracker
            .transactions_recorder
            .expect_process_invoice_transaction()
            .once()
            .returning(|_invoice, _transaction| {
                Err(TransactionsRecorderError::DaoTransactionError)
            });

        tracker
            .sweep(&[], &mut SweepState::new())
            .await;

        assert!(logs_contain(
            "holding the cursor to retry the range"
        ));
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn sweep_stops_and_says_so_when_the_chain_cannot_backfill() {
        let mut chain_client = MockBlockChainClient::<PolygonChainConfig>::default();
        // Called once for the first sweep, and never again once disabled.
        chain_client
            .expect_latest_confirmed_block()
            .once()
            .returning(|| Err(BackfillError::Unsupported));

        let mut dao = MockDaoInterface::default();
        dao.expect_get_chain_sync_cursor()
            .never();

        let tracker = sweep_tracker(
            chain_client,
            dao,
            InvoiceRegistry::new(),
        );
        let mut state = SweepState::new();

        tracker.sweep(&[], &mut state).await;

        assert!(!state.enabled);
        assert!(logs_contain(
            "Chain client cannot re-read past blocks"
        ));
    }
}
