use std::time::Duration;

use kalatori_client::types::{
    ChainType,
    KalatoriEventExt,
};
use rust_decimal::Decimal;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::chain::{
    InvoiceRegistry,
    TransactionsRecorder,
    TransactionsRecorderError,
};
use crate::chain_client::{
    AssetHubChainConfig,
    AssetHubClient,
    BlockChainClient,
    PolygonChainConfig,
    PolygonClient,
};
use crate::configs::PaymentsConfig;
use crate::dao::{
    DAO,
    DaoInterface,
    DaoTransactionInterface,
};
use crate::etherscan_client::EtherscanClient;
use crate::types::{
    IncomingTransaction,
    Invoice,
    InvoiceEventType,
    InvoiceStatus,
    InvoiceWithReceivedAmount,
};

const EXPIRATION_CHECK_INTERVAL_MILLIS: u64 = 10_000;

#[derive(Debug)]
enum ExpirationDetectorError {
    FetchBalanceFailed,
    FetchTransfersFailed,
    DatabaseError,
}

pub struct ExpirationDetector<
    D: DaoInterface + 'static = DAO,
    AH: BlockChainClient<AssetHubChainConfig> + 'static = AssetHubClient,
    PG: BlockChainClient<PolygonChainConfig> + 'static = PolygonClient,
> {
    dao: D,
    registry: InvoiceRegistry,
    asset_hub_client: AH,
    polygon_client: PG,
    etherscan_client: EtherscanClient,
    config: PaymentsConfig,
    transactions_recorder: TransactionsRecorder<D>,
}

impl<
    D: DaoInterface + 'static,
    AH: BlockChainClient<AssetHubChainConfig> + 'static,
    PG: BlockChainClient<PolygonChainConfig> + 'static,
> ExpirationDetector<D, AH, PG>
{
    pub fn new(
        dao: D,
        registry: InvoiceRegistry,
        asset_hub_client: AH,
        polygon_client: PG,
        etherscan_client: EtherscanClient,
        config: PaymentsConfig,
        transactions_recorder: TransactionsRecorder<D>,
    ) -> Self {
        ExpirationDetector {
            dao,
            registry,
            asset_hub_client,
            polygon_client,
            etherscan_client,
            config,
            transactions_recorder,
        }
    }

    async fn fetch_expired_invoices(&self) -> Vec<Invoice> {
        // TODO: fetch partially paid expired invoices as well and return them together

        self.dao
            .get_expired_invoices()
            .await
            .inspect_err(|_| {
                tracing::warn!("Failed to fetch expired invoices from database");
            })
            .unwrap_or_default()
    }

    #[tracing::instrument(skip(self))]
    async fn get_account_balance(
        &self,
        chain: ChainType,
        asset_id: &str,
        address: &str,
    ) -> Result<Decimal, ExpirationDetectorError> {
        match chain {
            // We don't expect parsing errors here, unwraps should be safe
            ChainType::PolkadotAssetHub => {
                self.asset_hub_client
                    .fetch_asset_balance(
                        &asset_id.parse().unwrap(),
                        &address.parse().unwrap(),
                    )
                    .await
            },
            ChainType::Polygon => {
                self.polygon_client
                    .fetch_asset_balance(
                        &asset_id.parse().unwrap(),
                        &address.parse().unwrap(),
                    )
                    .await
            },
        }
        .map_err(|e| {
            tracing::warn!(
                error.source = ?e,
                "Failed to get account balance in order to compare with received amount"
            );

            ExpirationDetectorError::FetchBalanceFailed
        })
    }

    #[tracing::instrument(skip(self))]
    async fn get_incoming_transactions(
        &self,
        chain: ChainType,
        asset_id: &str,
        address: &str,
        invoice_id: Uuid,
    ) -> Result<Vec<IncomingTransaction>, ExpirationDetectorError> {
        match chain {
            ChainType::PolkadotAssetHub => {
                // TODO: it's better to return some kind of error instead
                Ok(vec![])
            },
            ChainType::Polygon => self
                .etherscan_client
                .get_account_incoming_transfers(chain, asset_id, address, invoice_id)
                .await
                .map_err(|e| {
                    tracing::warn!(
                        error = ?e,
                        "Failed to get account incoming transfers using etherscan client"
                    );

                    ExpirationDetectorError::FetchTransfersFailed
                }),
        }
    }

    #[tracing::instrument(
        skip(self, invoice),
        fields(
            invoice_id = %invoice.invoice.id,
            received_amount = %invoice.total_received_amount,
        )
    )]
    async fn get_and_store_transactions(
        &self,
        mut invoice: InvoiceWithReceivedAmount,
        balance: Decimal,
    ) -> Result<(), ExpirationDetectorError> {
        let received_amount = invoice.total_received_amount;
        let invoice_id = invoice.invoice.id;
        let chain = invoice.invoice.chain;
        let asset_id = &invoice.invoice.asset_id;
        let address = &invoice.invoice.payment_address;

        tracing::warn!("Detected inconsistency in recorded received amount and account balance");

        let incoming_transactions = self
            .get_incoming_transactions(
                chain,
                asset_id,
                address,
                invoice_id
            )
            .await
            .map_err(|e| {
                tracing::warn!(
                    error = ?e,
                    "Error while trying to get incoming transactions from indexers, invoice will not be marked as expired yet"
                );

                ExpirationDetectorError::FetchTransfersFailed
            })?;

        let total_amount: Decimal = incoming_transactions
            .iter()
            .map(|trans| trans.transfer_info.amount)
            .sum();

        if total_amount != balance {
            // TODO: build event and send it as a webhook. It'll be a way to
            // notify admin that something goes wrong and require manual intervention
            tracing::error!(
                transactions_amount_sum = ?total_amount,
                "Account balance amount is not equal to sum of its incoming transactions"
            );
        }

        if received_amount != total_amount {
            tracing::warn!(
                transactions_amount_sum = ?total_amount,
                "Recorded received amount (sum of incoming transactions amounts stored in database) is not equal to sum of incoming transactions fetched from indexer. Probably some transactions have been missing, store them now"
            );

            for transaction in incoming_transactions {
                // TODO: On transaction update, it can become partially paid or paid
                // If it's partially paid, it still remains expired (we don't extend valid till
                // period) so we probably need to handle that case and initiate
                // refund. Perhaps it will happen on the next iteration
                // automatically? Need to check it out when refunds will be implemented
                match self
                    .transactions_recorder
                    .process_invoice_transaction(&mut invoice, transaction)
                    .await
                {
                    Ok(()) => tracing::info!("Missing transaction has been recorded in database"),
                    Err(TransactionsRecorderError::TransactionDuplication {
                        ..
                    }) => tracing::debug!("Transaction is already presented in the database"),
                    Err(_) => tracing::warn!(
                        "Database error occurred while trying to record potentially missing transaction"
                    ),
                };
            }
        }

        Ok(())
    }

    async fn check_invoice_balance(
        &self,
        invoice: InvoiceWithReceivedAmount,
    ) -> Result<(), ExpirationDetectorError> {
        let received_amount = invoice.total_received_amount;
        let invoice_id = invoice.invoice.id;
        let chain = invoice.invoice.chain;
        let asset_id = &invoice.invoice.asset_id;
        let address = &invoice.invoice.payment_address;

        let balance = self
            .get_account_balance(chain, asset_id, address)
            .await?;

        if received_amount != balance {
            self.get_and_store_transactions(invoice, balance)
                .await?;
        } else {
            let event = invoice
                .into_public_invoice(&self.config.payment_url_base)
                .build_event(InvoiceEventType::Expired)
                .into();

            let dao_transaction = self
                .dao
                .begin_transaction()
                .await
                .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

            dao_transaction
                .create_webhook_event(event)
                .await
                .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

            // TODO: we currently handle only unpaid expired invoices, will need also handle
            // partially paid expired
            dao_transaction
                .update_invoice_status(invoice_id, InvoiceStatus::UnpaidExpired)
                .await
                .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

            dao_transaction
                .commit()
                .await
                .map_err(|_e| ExpirationDetectorError::DatabaseError)?;

            self.registry
                .remove_invoice(&invoice_id)
                .await;

            tracing::info!(
                %invoice_id,
                "Invoice has been marked as expired"
            );
        }

        Ok(())
    }

    // 1. Update statuses in the database for expired and partially paid expired
    //    invoices
    // 2. Notify tracker, it should remove them from tracking
    // 3. Check balances one last time, if it doesn't match with recorded received
    //    amount, fetch transactions from indexer and update status
    // 4. Schedule webhooks for expired invoices
    async fn handle_expirations(&self) {
        let expired_invoices = self.fetch_expired_invoices().await;

        let expired_invoices_ids: Vec<_> = expired_invoices
            .iter()
            .map(|inv| inv.id)
            .collect();

        if expired_invoices_ids.is_empty() {
            tracing::trace!("There are no expired invoices, do nothing");
        } else {
            tracing::info!(
                expired_invoices_ids = ?expired_invoices_ids,
                "There are {} expired invoices, trying to process them", expired_invoices_ids.len()
            );
        }

        for invoice in expired_invoices {
            let Some(invoice_with_amount) = self
                .registry
                .get_invoice(&invoice.id)
                .await
            else {
                tracing::error!(
                    invoice_id = %invoice.id,
                    "An invoice which should be marked as expired is not found in registry"
                );
                // TODO: in that case we probably should try to fetch invoice with amounts from
                // database

                continue
            };

            match self
                .check_invoice_balance(invoice_with_amount)
                .await
            {
                Ok(()) => tracing::info!(
                    invoice_id = %invoice.id,
                    "Expired invoice has been processed successfully"
                ),
                Err(e) => tracing::warn!(
                    invoice_id = %invoice.id,
                    invoice_status = %invoice.status,
                    error = ?e,
                    "Error while trying process expired invoice. It remains with previous status"
                ),
            };
        }
    }

    #[tracing::instrument(skip_all, fields(category = "expiration_detector"))]
    async fn perform(
        self,
        token: CancellationToken,
    ) {
        let mut interval = interval(Duration::from_millis(
            EXPIRATION_CHECK_INTERVAL_MILLIS,
        ));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.handle_expirations().await;
                }
                () = token.cancelled() => {
                    tracing::info!(
                        "Expiration detector received shutdown signal, finishing pending tasks before shutting down"
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
