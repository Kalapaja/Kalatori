use kalatori_client::types::ChainType;
use rust_decimal::Decimal;
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
use crate::dao::{
    DAO,
    DaoInterface,
};
use crate::etherscan_client::EtherscanClient;
use crate::types::{
    IncomingTransaction,
    InvoiceWithReceivedAmount,
};

#[derive(Debug)]
pub enum BalanceCheckerError {
    InvoiceNotFound { invoice_id: Uuid },
    FetchBalanceFailed,
    FetchTransfersFailed,
}

#[derive(Clone)]
pub struct BalanceChecker<
    D: DaoInterface + 'static = DAO,
    AH: BlockChainClient<AssetHubChainConfig> + 'static = AssetHubClient,
    PG: BlockChainClient<PolygonChainConfig> + 'static = PolygonClient,
> {
    registry: InvoiceRegistry,
    asset_hub_client: AH,
    polygon_client: PG,
    etherscan_client: EtherscanClient,
    transactions_recorder: TransactionsRecorder<D>,
}

impl<
    D: DaoInterface + 'static,
    AH: BlockChainClient<AssetHubChainConfig> + 'static,
    PG: BlockChainClient<PolygonChainConfig> + 'static,
> BalanceChecker<D, AH, PG>
{
    pub fn new(
        registry: InvoiceRegistry,
        asset_hub_client: AH,
        polygon_client: PG,
        etherscan_client: EtherscanClient,
        transactions_recorder: TransactionsRecorder<D>,
    ) -> Self {
        Self {
            registry,
            asset_hub_client,
            polygon_client,
            etherscan_client,
            transactions_recorder,
        }
    }

    #[tracing::instrument(skip(self))]
    async fn get_account_balance(
        &self,
        chain: ChainType,
        asset_id: &str,
        address: &str,
    ) -> Result<Decimal, BalanceCheckerError> {
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

            BalanceCheckerError::FetchBalanceFailed
        })
    }

    #[tracing::instrument(skip(self))]
    async fn get_incoming_transactions(
        &self,
        chain: ChainType,
        asset_id: &str,
        address: &str,
        invoice_id: Uuid,
    ) -> Result<Vec<IncomingTransaction>, BalanceCheckerError> {
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

                    BalanceCheckerError::FetchTransfersFailed
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
        invoice: &mut InvoiceWithReceivedAmount,
        balance: Decimal,
    ) -> Result<(), BalanceCheckerError> {
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

                BalanceCheckerError::FetchTransfersFailed
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
                    .process_invoice_transaction(invoice, transaction)
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

    pub async fn check_invoice_balance(
        &self,
        invoice_id: Uuid,
    ) -> Result<InvoiceWithReceivedAmount, BalanceCheckerError> {
        let Some(mut invoice) = self
            .registry
            .get_invoice(&invoice_id)
            .await
        else {
            // TODO: in that case we probably should try to fetch invoice with amounts from
            // database
            return Err(BalanceCheckerError::InvoiceNotFound {
                invoice_id,
            })
        };

        let received_amount = invoice.total_received_amount;
        let chain = invoice.invoice.chain;
        let asset_id = &invoice.invoice.asset_id;
        let address = &invoice.invoice.payment_address;

        let balance = self
            .get_account_balance(chain, asset_id, address)
            .await?;

        if received_amount != balance {
            self.get_and_store_transactions(&mut invoice, balance)
                .await?;
        }

        Ok(invoice)
    }
}
