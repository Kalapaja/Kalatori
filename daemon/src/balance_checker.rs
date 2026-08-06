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
    DaoTransactionInterface,
};
use crate::etherscan_client::EtherscanClient;
use crate::types::{
    GeneralTransactionId,
    IncomingTransaction,
    InvoiceWithReceivedAmount,
    TransferInfo,
};
use crate::utils::logging::{
    category,
    operation,
};

#[derive(Debug)]
pub enum BalanceCheckerError {
    InvoiceNotFound { invoice_id: Uuid },
    FetchBalanceFailed,
    FetchTransfersFailed,
    DatabaseError,
}

#[derive(Clone)]
pub struct BalanceChecker<
    D: DaoInterface + 'static = DAO,
    AH: BlockChainClient<AssetHubChainConfig> + 'static = AssetHubClient,
    PG: BlockChainClient<PolygonChainConfig> + 'static = PolygonClient,
> {
    dao: D,
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
        dao: D,
        registry: InvoiceRegistry,
        asset_hub_client: AH,
        polygon_client: PG,
        etherscan_client: EtherscanClient,
        transactions_recorder: TransactionsRecorder<D>,
    ) -> Self {
        Self {
            dao,
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
        // The asset id and the address come from config and from stored
        // invoices. Both should always parse, but a single bad row must not be
        // able to take the daemon down — report it as a failed balance fetch.
        let unparsable = |field: &'static str| {
            tracing::warn!(
                error.category = category::BALANCE_CHECKER,
                error.operation = operation::FETCH_BALANCE,
                %chain,
                field,
                "Balance fetch skipped: value does not parse for this chain"
            );

            BalanceCheckerError::FetchBalanceFailed
        };

        match chain {
            ChainType::PolkadotAssetHub => {
                self.asset_hub_client
                    .fetch_asset_balance(
                        asset_id
                            .parse()
                            .map_err(|_| unparsable("asset_id"))?,
                        address
                            .parse()
                            .map_err(|_| unparsable("address"))?,
                    )
                    .await
            },
            ChainType::Polygon => {
                self.polygon_client
                    .fetch_asset_balance(
                        asset_id
                            .parse()
                            .map_err(|_| unparsable("asset_id"))?,
                        address
                            .parse()
                            .map_err(|_| unparsable("address"))?,
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
        asset_id: &str,
        address: &str,
        invoice_id: Uuid,
    ) -> Result<Vec<IncomingTransaction>, BalanceCheckerError> {
        self.etherscan_client
            .get_account_incoming_transfers(
                ChainType::Polygon,
                asset_id,
                address,
                invoice_id,
            )
            .await
            .map_err(|e| {
                tracing::warn!(
                    error = ?e,
                    "Failed to get account incoming transfers using etherscan client"
                );

                BalanceCheckerError::FetchTransfersFailed
            })
    }

    /// Asset Hub has no external indexer to replay missed history from, but
    /// every payment address is unique to its invoice, so the finalized
    /// on-chain balance is the ground truth of what the invoice received.
    /// When the balance is ahead of our records (subscription gap, restart),
    /// record the difference as a synthetic adjustment transaction so the
    /// invoice converges instead of expiring unpaid.
    #[expect(clippy::arithmetic_side_effects)]
    #[tracing::instrument(
        skip(self, invoice),
        fields(invoice_id = %invoice.invoice.id)
    )]
    async fn reconcile_asset_hub_balance(
        &self,
        invoice: &mut InvoiceWithReceivedAmount,
        balance: Decimal,
    ) -> Result<(), BalanceCheckerError> {
        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|_| BalanceCheckerError::DatabaseError)?;

        // This read and the adjustment write below deliberately share one
        // transaction. If the live subscription committed a transfer after
        // the balance fetch, this snapshot includes it; if it tries to commit
        // after this read, SQLite serializes the competing writes (or rejects
        // this stale writer for a later retry) instead of letting both commit.
        let persisted_invoice = dao_transaction
            .get_invoice_with_received_amount_by_id(invoice.invoice.id)
            .await
            .map_err(|_| BalanceCheckerError::DatabaseError)?
            .ok_or(BalanceCheckerError::InvoiceNotFound {
                invoice_id: invoice.invoice.id,
            })?;

        let received_amount = persisted_invoice.total_received_amount;
        let delta = balance - received_amount;

        if delta <= Decimal::ZERO {
            if delta < Decimal::ZERO {
                // We recorded more than the address holds. Never "un-record"
                // payments automatically — this needs a human (funds moved out
                // of the payment address, or a transfer was double-recorded).
                tracing::error!(
                    %balance,
                    %received_amount,
                    "Recorded received amount exceeds on-chain balance, manual intervention required"
                );
            } else {
                tracing::debug!(
                    %balance,
                    "Transaction-scoped received amount already matches the on-chain balance"
                );
            }

            dao_transaction
                .commit()
                .await
                .map_err(|_| BalanceCheckerError::DatabaseError)?;
            *invoice = persisted_invoice;
            return Ok(());
        }

        tracing::warn!(
            %balance,
            %received_amount,
            %delta,
            "On-chain balance is ahead of recorded transfers, recording a balance-adjustment transaction to recover missed transfers"
        );

        // Both the balance query (`at_latest` resolves to the latest finalized
        // block in subxt) and the transfer subscription observe finalized
        // blocks only, so the delta consists of finalized transfers the
        // subscription missed. All-NULL transaction coordinates keep this
        // record outside the per-chain uniqueness indexes.
        let transaction = IncomingTransaction {
            id: Uuid::new_v4(),
            invoice_id: persisted_invoice.invoice.id,
            transfer_info: TransferInfo {
                chain: ChainType::PolkadotAssetHub,
                asset_id: persisted_invoice
                    .invoice
                    .asset_id
                    .clone(),
                asset_name: persisted_invoice
                    .invoice
                    .asset_name
                    .clone(),
                amount: delta,
                source_address: "unknown".to_string(),
                destination_address: persisted_invoice
                    .invoice
                    .payment_address
                    .clone(),
            },
            transaction_id: GeneralTransactionId {
                block_number: None,
                position_in_block: None,
                tx_hash: None,
            },
        };

        let (updated_invoice, min_paid_amount) = match self
            .transactions_recorder
            .process_invoice_transaction_in(
                &dao_transaction,
                &persisted_invoice,
                transaction,
            )
            .await
        {
            Ok(update) => update,
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    "Database error occurred while trying to record balance-adjustment transaction"
                );
                return Err(BalanceCheckerError::DatabaseError)
            },
        };

        dao_transaction
            .commit()
            .await
            .map_err(|_| BalanceCheckerError::DatabaseError)?;

        self.transactions_recorder
            .apply_recorded_invoice_update(
                invoice,
                updated_invoice,
                min_paid_amount,
            )
            .await;

        tracing::info!(
            invoice_status = %invoice.invoice.status,
            total_received_amount = %invoice.total_received_amount,
            "Balance-adjustment transaction has been recorded, invoice has been updated"
        );

        Ok(())
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
        tracing::warn!("Detected inconsistency in recorded received amount and account balance");

        if invoice.invoice.chain == ChainType::PolkadotAssetHub {
            return self
                .reconcile_asset_hub_balance(invoice, balance)
                .await;
        }

        let received_amount = invoice.total_received_amount;
        let invoice_id = invoice.invoice.id;
        let asset_id = &invoice.invoice.asset_id;
        let address = &invoice.invoice.payment_address;

        let incoming_transactions = self
            .get_incoming_transactions(
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

    #[tracing::instrument(skip(self))]
    pub async fn check_invoice_balance(
        &self,
        invoice_id: Uuid,
    ) -> Result<InvoiceWithReceivedAmount, BalanceCheckerError> {
        let mut invoice = if let Some(invoice) = self
            .registry
            .get_invoice(&invoice_id)
            .await
        {
            tracing::trace!(
                ?invoice,
                "Invoice for balance checking is found in registry"
            );
            invoice
        } else {
            self.dao
                .get_invoice_with_received_amount_by_id(invoice_id)
                .await
                .map_err(|e| {
                    tracing::warn!(
                        error = ?e,
                        "Failed to get invoice with received amounts from database"
                    );

                    BalanceCheckerError::DatabaseError
                })?
                .inspect(|invoice| tracing::trace!(
                    ?invoice,
                    "Invoice for balance checking wasn't found in registry but is found in database"
                ))
                .ok_or(BalanceCheckerError::InvoiceNotFound {
                    invoice_id,
                })?
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
        } else {
            tracing::trace!(
                ?balance,
                "Invoice received amount is equal to payment address balance"
            )
        }

        Ok(invoice)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use crate::chain::TransactionsRecorder;
    use crate::chain_client::MockBlockChainClient;
    use crate::configs::EtherscanClientConfig;
    use crate::dao::{
        MockDaoInterface,
        MockDaoTransactionInterface,
    };
    use crate::types::{
        InvoiceStatus,
        default_invoice,
    };

    use super::*;

    fn balance_checker() -> BalanceChecker<
        MockDaoInterface,
        MockBlockChainClient<AssetHubChainConfig>,
        MockBlockChainClient<PolygonChainConfig>,
    > {
        BalanceChecker::new(
            MockDaoInterface::default(),
            InvoiceRegistry::new(),
            MockBlockChainClient::<AssetHubChainConfig>::default(),
            MockBlockChainClient::<PolygonChainConfig>::default(),
            EtherscanClient::new(EtherscanClientConfig {
                requests_per_second: NonZeroU32::MIN,
                api_key: String::new().into(),
            }),
            TransactionsRecorder::<MockDaoInterface>::default(),
        )
    }

    #[tokio::test]
    async fn malformed_chain_identifiers_fail_before_calling_rpc() {
        let checker = balance_checker();
        let asset_hub_address = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
        let polygon_address = "0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7";

        for (chain, asset_id, address) in [
            (
                ChainType::PolkadotAssetHub,
                "not-an-asset",
                asset_hub_address,
            ),
            (
                ChainType::PolkadotAssetHub,
                "1337",
                "not-an-address",
            ),
            (
                ChainType::Polygon,
                "not-an-asset",
                polygon_address,
            ),
            (
                ChainType::Polygon,
                polygon_address,
                "not-an-address",
            ),
        ] {
            assert!(matches!(
                checker
                    .get_account_balance(chain, asset_id, address)
                    .await,
                Err(BalanceCheckerError::FetchBalanceFailed)
            ));
        }
    }

    #[tokio::test]
    async fn asset_hub_reconciliation_observes_transfer_committed_after_balance_fetch() {
        let mut stale_invoice = default_invoice().with_amount(Decimal::ZERO);
        stale_invoice.invoice.chain = ChainType::PolkadotAssetHub;
        let invoice_id = stale_invoice.invoice.id;
        let balance = Decimal::TEN;

        // Inject the race deterministically: the balance checker still holds a
        // zero-total clone, while the transaction-scoped read represents the
        // live subscription having committed the full balance in between.
        let mut persisted_invoice = stale_invoice.clone();
        persisted_invoice.total_received_amount = balance;
        let mut dao_transaction = MockDaoTransactionInterface::default();
        dao_transaction
            .expect_get_invoice_with_received_amount_by_id()
            .once()
            .with(mockall::predicate::eq(invoice_id))
            .return_once(move |_| Ok(Some(persisted_invoice)));
        dao_transaction
            .expect_commit()
            .once()
            .return_once(|| Ok(()));

        let mut dao = MockDaoInterface::default();
        dao.expect_begin_transaction()
            .once()
            .return_once(move || Ok(dao_transaction));

        let mut recorder = TransactionsRecorder::<MockDaoInterface>::default();
        recorder
            .expect_process_invoice_transaction()
            .never();
        recorder
            .expect_process_invoice_transaction_in()
            .never();
        recorder
            .expect_apply_recorded_invoice_update()
            .never();

        let checker = BalanceChecker::new(
            dao,
            InvoiceRegistry::new(),
            MockBlockChainClient::<AssetHubChainConfig>::default(),
            MockBlockChainClient::<PolygonChainConfig>::default(),
            EtherscanClient::new(EtherscanClientConfig {
                requests_per_second: NonZeroU32::MIN,
                api_key: String::new().into(),
            }),
            recorder,
        );

        assert!(
            checker
                .reconcile_asset_hub_balance(&mut stale_invoice, balance)
                .await
                .is_ok()
        );
        assert_eq!(
            stale_invoice.total_received_amount,
            balance
        );
    }

    #[tokio::test]
    async fn asset_hub_reconciliation_records_genuine_shortfall_once() {
        let mut stale_invoice = default_invoice().with_amount(Decimal::ZERO);
        stale_invoice.invoice.chain = ChainType::PolkadotAssetHub;
        let invoice_id = stale_invoice.invoice.id;
        let recorded_amount = Decimal::new(3, 0);
        let balance = Decimal::TEN;
        let expected_delta = balance - recorded_amount;

        let mut persisted_invoice = stale_invoice.clone();
        persisted_invoice.total_received_amount = recorded_amount;
        let returned_persisted_invoice = persisted_invoice.clone();

        let mut dao_transaction = MockDaoTransactionInterface::default();
        dao_transaction
            .expect_get_invoice_with_received_amount_by_id()
            .once()
            .with(mockall::predicate::eq(invoice_id))
            .return_once(move |_| Ok(Some(returned_persisted_invoice)));
        dao_transaction
            .expect_commit()
            .once()
            .return_once(|| Ok(()));

        let mut dao = MockDaoInterface::default();
        dao.expect_begin_transaction()
            .once()
            .return_once(move || Ok(dao_transaction));

        let mut recorder = TransactionsRecorder::<MockDaoInterface>::default();
        recorder
            .expect_process_invoice_transaction()
            .never();
        recorder
            .expect_process_invoice_transaction_in()
            .once()
            .withf(move |_, invoice, transaction| {
                invoice.total_received_amount == recorded_amount
                    && transaction.invoice_id == invoice_id
                    && transaction.transfer_info.amount == expected_delta
                    && transaction
                        .transaction_id
                        .block_number
                        .is_none()
                    && transaction
                        .transaction_id
                        .position_in_block
                        .is_none()
                    && transaction
                        .transaction_id
                        .tx_hash
                        .is_none()
            })
            .return_once(|_, invoice, transaction| {
                let mut updated_invoice = invoice.clone();
                updated_invoice.total_received_amount += transaction.transfer_info.amount;
                updated_invoice.invoice.status = InvoiceStatus::PartiallyPaid;
                Ok((updated_invoice, Decimal::ZERO))
            });
        recorder
            .expect_apply_recorded_invoice_update()
            .once()
            .return_once(|invoice, updated_invoice, _| *invoice = updated_invoice);

        let checker = BalanceChecker::new(
            dao,
            InvoiceRegistry::new(),
            MockBlockChainClient::<AssetHubChainConfig>::default(),
            MockBlockChainClient::<PolygonChainConfig>::default(),
            EtherscanClient::new(EtherscanClientConfig {
                requests_per_second: NonZeroU32::MIN,
                api_key: String::new().into(),
            }),
            recorder,
        );

        assert!(
            checker
                .reconcile_asset_hub_balance(&mut stale_invoice, balance)
                .await
                .is_ok()
        );
        assert_eq!(
            stale_invoice.total_received_amount,
            balance
        );
    }
}
