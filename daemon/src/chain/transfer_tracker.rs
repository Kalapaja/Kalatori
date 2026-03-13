use std::str::FromStr;

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
                tracing::error!(
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
                tracing::error!(
                    error.category = "transfer_tracker",
                    error.operation = "handle_subscription_event",
                    error.source = ?e,
                    "Error receiving transfer event"
                );
                Err(e)
            },
            None => {
                tracing::warn!("Transfer event subscription ended");
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

        loop {
            subscription = self
                .get_or_create_subscription(subscription, &assets)
                .await;

            let Some(poll_subscription) = &mut subscription else {
                tracing::warn!(
                    "Failed poll chain subscription, probably it's down. Trying to recreate client and resubscribe..."
                );
                // If we couldn't create a subscription, try to recreate the client with another
                // RPC endpoint
                match self.client.recreate().await {
                    Ok(new_client) => {
                        self.client = new_client;

                        tracing::warn!(
                            "Recreated blockchain client for {} with new RPC endpoint",
                            self.client.chain_name()
                        );

                        // Retry subscription immediately with the new client
                        continue;
                    },
                    Err(e) => {
                        tracing::error!(
                            error.category = "transfer_tracker",
                            error.operation = "perform",
                            error.source = ?e,
                            "Error recreating blockchain client"
                        );
                    },
                }

                // All endpoints failed — wait before retrying, but respect cancellation
                tokio::select! {
                    () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
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
                    if self.handle_subscription_event(subscription_event).await.is_err() {
                        subscription = None;
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
