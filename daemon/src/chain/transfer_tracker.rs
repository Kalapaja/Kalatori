use std::collections::{
    HashMap,
    HashSet,
};
use std::str::FromStr;
use std::sync::Arc;

use futures::StreamExt;
use rust_decimal::Decimal;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::chain::TransactionsRecorderError;
use crate::chain_client::{
    BlockChainClient,
    ChainConfig,
    ChainTransfer,
    GeneralChainTransfer,
    SubscriptionError,
    TransfersStream,
};
use crate::dao::DaoInterface;
use crate::types::{
    ChainType,
    IncomingTransaction,
    InvoiceWithReceivedAmount,
};

use super::TransactionsRecorder;

// TODO: move it somewhere else
#[derive(Clone)]
pub struct InvoiceRegistry {
    invoices: Arc<RwLock<HashMap<Uuid, InvoiceWithReceivedAmount>>>,
}

impl InvoiceRegistry {
    pub fn new() -> Self {
        InvoiceRegistry {
            invoices: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_invoice(
        &self,
        record: InvoiceWithReceivedAmount,
    ) {
        let mut invoices = self.invoices.write().await;
        invoices.insert(record.invoice.id, record);
    }

    pub async fn add_invoices(
        &self,
        records: Vec<InvoiceWithReceivedAmount>,
    ) {
        let mut invoices_map = self.invoices.write().await;

        for record in records {
            invoices_map.insert(record.invoice.id, record);
        }
    }

    pub async fn remove_invoice(
        &self,
        invoice_id: &Uuid,
    ) -> Option<InvoiceWithReceivedAmount> {
        let mut invoices = self.invoices.write().await;
        invoices.remove(invoice_id)
    }

    #[expect(dead_code)]
    pub async fn remove_invoices(
        &self,
        invoices_ids: &[Uuid],
    ) -> Vec<InvoiceWithReceivedAmount> {
        let mut invoices = self.invoices.write().await;
        let mut removed_invoices = Vec::with_capacity(invoices_ids.len());

        for invoice_id in invoices_ids {
            if let Some(invoice) = invoices.remove(invoice_id) {
                removed_invoices.push(invoice);
            }
        }

        removed_invoices
    }

    pub async fn get_invoice(
        &self,
        invoice_id: &Uuid,
    ) -> Option<InvoiceWithReceivedAmount> {
        let invoices = self.invoices.read().await;
        invoices.get(invoice_id).cloned()
    }

    pub async fn find_invoice_by_address(
        &self,
        address: &str,
        chain: ChainType,
        asset_id: &str,
    ) -> Option<InvoiceWithReceivedAmount> {
        // TODO: if we'll have large amount of invoices and incoming transfers
        // it might be a problem and should be optimized
        let invoices = self.invoices.read().await;

        invoices
            .values()
            .find(|inv| {
                inv.invoice.chain == chain
                    && inv.invoice.payment_address == address
                    && inv.invoice.asset_id == asset_id
            })
            .cloned()
    }

    pub async fn update_filled_amount(
        &self,
        invoice_id: &Uuid,
        new_filled_amount: Decimal,
    ) {
        let mut invoices = self.invoices.write().await;

        if let Some(record) = invoices.get_mut(invoice_id) {
            record.total_received_amount = new_filled_amount;
        }
    }

    pub async fn used_asset_ids(&self) -> HashMap<ChainType, HashSet<String>> {
        let invoices = self.invoices.read().await;
        let mut asset_ids_map: HashMap<_, HashSet<_>> = HashMap::new();

        for record in invoices.values() {
            asset_ids_map
                .entry(record.invoice.chain)
                .or_default()
                .insert(record.invoice.asset_id.clone());
        }

        asset_ids_map
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub async fn invoices_count(&self) -> usize {
        let invoices = self.invoices.read().await;
        invoices.len()
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
