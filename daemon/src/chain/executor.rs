use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;

use chrono::Utc;
use futures::stream::{
    FuturesUnordered,
    StreamExt,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use thiserror::Error;
use tokio::time::{
    Duration,
    interval,
};
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use uuid::Uuid;

use crate::chain_client::{
    AssetHubChainConfig,
    AssetHubClient,
    BlockChainClient,
    ChainConfig,
    GeneralChainTransfer,
    KeyringClient,
    PolygonChainConfig,
    PolygonClient,
    SignedTransaction,
    SignedTransactionUtils,
    TransactionError,
};
use crate::dao::{
    DAO,
    DaoInterface,
    DaoTransactionInterface,
};
use crate::types::{
    ChainType,
    GeneralTransactionId,
    OutgoingTransaction,
    Payout,
    PayoutStatus,
    RetryMeta,
    Transaction,
    TransactionOrigin,
    TransactionOriginVariant,
    TransferInfo,
};

#[derive(Debug, Error)]
pub enum ChainExecutorError {
    // Database related error, we weren't able to fetch payouts/refunds to process
    #[error("Failed to fetch transfers from database")]
    FetchTransfers,
    // Error while building or signing transfer. Consider this error retriable for now,
    // later we might change this behavior based on the error details
    #[error("Failed to build or sign transfer: {reason}")]
    BuildTransfer { reason: String },
    // Database transaction error while storing processing results
    #[error("Database transaction error: {reason}")]
    DaoTransactionError { reason: String },
}

const MAX_CONCURRENT_TRANSFERS: u32 = 10;
const POLLING_INTERVAL_MILLIS: u64 = 100;

#[derive(Debug, Clone)]
pub struct ChainPayoutRequest<T: ChainConfig> {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub chain: ChainType,
    pub asset_id: T::AssetId,
    pub asset_name: String,
    pub source_address: T::AccountId,
    pub destination_address: T::AccountId,
    pub amount: Decimal,
    pub retry_meta: RetryMeta,
}

impl<T: ChainConfig> ChainPayoutRequest<T> {
    pub fn new(
        id: Uuid,
        invoice_id: Uuid,
        transfer_info: TransferInfo,
        retry_meta: RetryMeta,
    ) -> Result<Self, ()> {
        Ok(Self {
            id,
            invoice_id,
            chain: transfer_info.chain,
            asset_id: T::AssetId::from_str(&transfer_info.asset_id).map_err(|_| ())?,
            asset_name: transfer_info.asset_name,
            source_address: T::AccountId::from_str(&transfer_info.source_address)
                .map_err(|_| ())?,
            destination_address: T::AccountId::from_str(&transfer_info.destination_address)
                .map_err(|_| ())?,
            amount: transfer_info.amount,
            retry_meta,
        })
    }
}

#[derive(Debug)]
pub enum ChainPayoutRequestTyped {
    AssetHub(ChainPayoutRequest<AssetHubChainConfig>),
    Polygon(ChainPayoutRequest<PolygonChainConfig>),
}

// TODO: perhaps it might be just `From`? Used `TryFrom` when had `chain` field
// as string
impl TryFrom<Payout> for ChainPayoutRequestTyped {
    // TODO: handle errors properly
    type Error = ();

    fn try_from(value: Payout) -> Result<Self, Self::Error> {
        tracing::info!(
            invoice_id = %value.invoice_id,
            payout_id = %value.id,
            source_address = %value.transfer_info.source_address,
            destination_address = %value.transfer_info.destination_address,
            asset_id = %value.transfer_info.asset_id,
            amount = %value.transfer_info.amount,
            chain = ?value.transfer_info.chain,
            "Preparing payout request for processing",
        );
        let request = match value.transfer_info.chain {
            ChainType::PolkadotAssetHub => {
                ChainPayoutRequestTyped::AssetHub(ChainPayoutRequest::new(
                    value.id,
                    value.invoice_id,
                    value.transfer_info,
                    value.retry_meta,
                )?)
            },
            ChainType::Polygon => ChainPayoutRequestTyped::Polygon(ChainPayoutRequest::new(
                value.id,
                value.invoice_id,
                value.transfer_info,
                value.retry_meta,
            )?),
        };

        Ok(request)
    }
}

#[derive(Debug)]
struct TransactionExecutionError {
    // Can be fully empty if transaction ID is not available
    transaction_id: GeneralTransactionId,
    retry_meta: RetryMeta,
    is_retriable: bool,
}

#[derive(Debug)]
struct TransactionExecutionData {
    transaction_id: Uuid,
    invoice_id: Uuid,
    origin: TransactionOrigin,
    result: Result<GeneralChainTransfer, TransactionExecutionError>,
}

pub struct TransfersExecutor<
    D: DaoInterface + 'static = DAO,
    AH: BlockChainClient<AssetHubChainConfig> + 'static = AssetHubClient,
    PG: BlockChainClient<PolygonChainConfig> + 'static = PolygonClient,
> {
    asset_hub_client: Arc<AH>,
    polygon_client: Arc<PG>,
    dao: D,
    keyring_client: KeyringClient,
}

type BoxedTransferFuture = std::pin::Pin<Box<dyn Future<Output = TransactionExecutionData> + Send>>;

async fn send_transfer_request<T: ChainConfig, C: BlockChainClient<T>>(
    client: Arc<C>,
    signed_transaction: SignedTransaction<T>,
    request: ChainPayoutRequest<T>,
    transaction: Transaction,
) -> TransactionExecutionData {
    let response = client
        .submit_and_watch_transaction(signed_transaction)
        .await;

    let mut meta = request.retry_meta;

    let result = match response {
        Ok(transfer) => Ok(transfer.into()),
        Err(TransactionError::SubmissionStatusUnknown) => {
            // TODO: rework errors
            tracing::warn!(
                invoice_id = %request.invoice_id,
                payout_id = %request.id,
                "Transaction submission status is unknown, it may be retried",
            );
            meta.increment_retry(String::new());

            Err(TransactionExecutionError {
                transaction_id: GeneralTransactionId::empty(),
                retry_meta: meta,
                is_retriable: true,
            })
        },
        Err(TransactionError::ExecutionFailed {
            transaction_id,
            error_code,
        }) => {
            tracing::warn!(
                invoice_id = %request.invoice_id,
                payout_id = %request.id,
                error_code = %error_code,
                transaction_id = ?transaction_id,
                "Transaction execution failed on chain",
            );

            meta.increment_retry(error_code);

            Err(TransactionExecutionError {
                transaction_id: transaction_id.into(),
                retry_meta: meta,
                is_retriable: false,
            })
        },
        Err(TransactionError::TransactionInfoFetchFailed {
            transaction_id,
        }) => {
            tracing::warn!(
                invoice_id = %request.invoice_id,
                payout_id = %request.id,
                transaction_id = ?transaction_id,
                "Failed to fetch transaction info from chain, it may be retried",
            );

            meta.increment_retry(String::new());

            Err(TransactionExecutionError {
                transaction_id: transaction_id.into(),
                retry_meta: meta,
                is_retriable: true,
            })
        },
        Err(TransactionError::InsufficientBalance {
            transaction_id,
        }) => {
            tracing::warn!(
                invoice_id = %request.invoice_id,
                payout_id = %request.id,
                transaction_id = ?transaction_id,
                "Insufficient balance for transaction",
            );

            meta.increment_retry(String::new());

            Err(TransactionExecutionError {
                transaction_id: transaction_id.into(),
                retry_meta: meta,
                is_retriable: false,
            })
        },
        Err(TransactionError::UnknownAsset {
            transaction_id,
            asset_id,
        }) => {
            tracing::warn!(
                invoice_id = %request.invoice_id,
                payout_id = %request.id,
                transaction_id = ?transaction_id,
                asset_id = ?asset_id,
                "Unknown asset for transaction",
            );

            meta.increment_retry(asset_id.to_string());

            Err(TransactionExecutionError {
                transaction_id: transaction_id.into(),
                retry_meta: meta,
                is_retriable: false,
            })
        },
        Err(TransactionError::BuildFailed {
            ..
        }) => unreachable!(),
    };

    TransactionExecutionData {
        transaction_id: transaction.id,
        invoice_id: transaction.invoice_id,
        origin: transaction.origin,
        result,
    }
}

impl<
    D: DaoInterface + 'static,
    AH: BlockChainClient<AssetHubChainConfig> + 'static,
    PG: BlockChainClient<PolygonChainConfig> + 'static,
> TransfersExecutor<D, AH, PG>
{
    async fn collect_pending_payout_requests(
        &self,
        limit: u32,
    ) -> Result<Vec<ChainPayoutRequestTyped>, ChainExecutorError> {
        let payout_requests = self
            .dao
            .get_pending_payouts(limit)
            .await
            .map_err(|e| {
                tracing::warn!(
                    error = %e,
                    "Failed to fetch pending payout requests from database",
                );

                ChainExecutorError::FetchTransfers
            })?
            .into_iter()
            // TODO: add error handling and logging here
            .map(TryFrom::try_from)
            .filter_map(Result::ok)
            .collect::<Vec<ChainPayoutRequestTyped>>();

        Ok(payout_requests)
    }

    #[instrument(skip(self, client, request))]
    async fn build_and_sign_transfer<T: ChainConfig, C: BlockChainClient<T>>(
        &self,
        client: &Arc<C>,
        request: &ChainPayoutRequest<T>,
    ) -> Result<SignedTransaction<T>, ChainExecutorError> {
        let transaction = client
            .build_transfer_all(
                &request.source_address,
                &request.destination_address,
                &request.asset_id,
            )
            .await
            .map_err(|e| {
                tracing::warn!(
                    invoice_id = %request.invoice_id,
                    payout_id = %request.id,
                    error = ?e,
                    "Failed to build transfer_all transaction",
                );

                ChainExecutorError::BuildTransfer {
                    reason: format!("Failed to build transfer_all transaction: {e}"),
                }
            })?;

        let derivation_params = vec![request.invoice_id.to_string()];

        let signed_transaction = client
            .sign_transaction(
                transaction,
                derivation_params,
                &self.keyring_client,
            )
            .await
            .map_err(|e| {
                tracing::warn!(
                    invoice_id = %request.invoice_id,
                    payout_id = %request.id,
                    error = ?e,
                    "Failed to sign transfer transaction",
                );

                ChainExecutorError::BuildTransfer {
                    reason: format!("Failed to sign transfer transaction: {e}"),
                }
            })?;

        Ok(signed_transaction)
    }

    #[instrument(skip(self, request, signed_transaction))]
    async fn store_built_transfer<T: ChainConfig>(
        &self,
        request: &ChainPayoutRequest<T>,
        signed_transaction: &SignedTransaction<T>,
    ) -> Result<Transaction, ChainExecutorError> {
        let outgoing = OutgoingTransaction {
            id: Uuid::new_v4(),
            invoice_id: request.invoice_id,
            transfer_info: TransferInfo {
                chain: request.chain,
                asset_id: request.asset_id.to_string(),
                asset_name: request.asset_name.clone(),
                amount: request.amount,
                source_address: request.source_address.to_string(),
                destination_address: request.destination_address.to_string(),
            },
            tx_hash: signed_transaction.hash(),
            transaction_bytes: signed_transaction.to_raw_string(),
            origin: TransactionOrigin::payout(request.id),
        };

        let transaction = self
            .dao
            .create_transaction(outgoing.into())
            .await
            .map_err(|e| {
                tracing::warn!(
                    invoice_id = %request.invoice_id,
                    payout_id = %request.id,
                    error = ?e,
                    "Failed to store built transfer transaction",
                );

                ChainExecutorError::DaoTransactionError {
                    reason: format!("Failed to store built transfer transaction: {e}"),
                }
            })?;

        Ok(transaction)
    }

    #[instrument(
        skip(self, client, request),
        fields(
            invoice_id = %request.invoice_id,
            payout_id = %request.id,
            chain = ?request.chain,
        ),
    )]
    async fn prepare_transfer<T: ChainConfig + 'static, C: BlockChainClient<T> + 'static>(
        &self,
        client: Arc<C>,
        request: ChainPayoutRequest<T>,
    ) -> Result<BoxedTransferFuture, ChainExecutorError> {
        let signed_transaction = self
            .build_and_sign_transfer(&client, &request)
            .await?;

        let transaction = self
            .store_built_transfer(&request, &signed_transaction)
            .await?;

        let fut = Box::pin(send_transfer_request(
            client,
            signed_transaction,
            request,
            transaction,
        ));

        Ok(fut)
    }

    async fn schedule_transfers(
        &self,
        futures_set: &mut FuturesUnordered<BoxedTransferFuture>,
    ) -> Result<(), ChainExecutorError> {
        // Will be 0 if we reached the limit or overflowed (but it's not really
        // expected)
        let limit = MAX_CONCURRENT_TRANSFERS.saturating_sub(
            futures_set
                .len()
                .to_u32()
                .unwrap_or(u32::MAX),
        );

        if limit == 0 {
            return Ok(());
        }

        // Normally we should collect transfers in the next order:
        // 1. FailedRetriable Refunds
        // 2. FailedRetriable Payouts
        // 3. New Refunds
        // 4. New Payouts

        // TODO: prepare refunds/payouts requests on this stap and later collect all of
        // them into transactions collection. Further code (iterator) should
        // operate with unified collection of transaction instead of different
        // types so we can easily call different functions depending on the
        // Transaction status (schedule transfer for new one, retry for failed)
        // but with the same result.
        let payout_requests = self
            .collect_pending_payout_requests(limit)
            .await?;

        for request in payout_requests {
            match request {
                ChainPayoutRequestTyped::AssetHub(request) => {
                    let invoice_id = request.invoice_id;
                    let payout_id = request.id;
                    let mut retry_meta = request.retry_meta.clone();

                    let client = self.asset_hub_client.clone();
                    let prepared_transfer = self
                        .prepare_transfer(client, request)
                        .await;

                    match prepared_transfer {
                        Ok(transfer) => {
                            tracing::info!(
                                invoice_id = %invoice_id,
                                payout_id = %payout_id,
                                chain = "AssetHub",
                                "Scheduled transfer for processing on chain",
                            );
                            futures_set.push(transfer);
                        },
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                invoice_id = %invoice_id,
                                payout_id = %payout_id,
                                chain = "AssetHub",
                                "Failed to prepare transfer request, it will be marked as failed and retriable"
                            );
                            retry_meta.increment_retry(e.to_string());

                            if let Err(error) = self
                                .dao
                                .update_payout_retry(payout_id, retry_meta, true)
                                .await
                            {
                                tracing::error!(
                                    %error,
                                    invoice_id = %invoice_id,
                                    payout_id = %payout_id,
                                    chain = "AssetHub",
                                    "Error while trying to mark payout request failed but retriable. It might stuck in In Progress status"
                                );
                            };
                        },
                    }
                },
                ChainPayoutRequestTyped::Polygon(request) => {
                    let invoice_id = request.invoice_id;
                    let payout_id = request.id;
                    let mut retry_meta = request.retry_meta.clone();

                    let client = self.polygon_client.clone();
                    let prepared_transfer = self
                        .prepare_transfer(client, request)
                        .await;

                    match prepared_transfer {
                        Ok(transfer) => {
                            tracing::info!(
                                invoice_id = %invoice_id,
                                payout_id = %payout_id,
                                chain = "Polygon",
                                "Scheduled transfer for processing on chain",
                            );
                            futures_set.push(transfer);
                        },
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                invoice_id = %invoice_id,
                                payout_id = %payout_id,
                                chain = "Polygon",
                                "Failed to prepare transfer request, it will be marked as failed and retriable"
                            );
                            retry_meta.increment_retry(e.to_string());

                            if let Err(error) = self
                                .dao
                                .update_payout_retry(payout_id, retry_meta, true)
                                .await
                            {
                                tracing::error!(
                                    %error,
                                    invoice_id = %invoice_id,
                                    payout_id = %payout_id,
                                    chain = "AssetHub",
                                    "Error while trying to mark payout request failed but retriable. It might stuck in In Progress status"
                                );
                            };
                        },
                    }
                },
            }
        }

        Ok(())
    }

    #[instrument(skip(self, dao_transaction, origin, transfer))]
    async fn handle_transfer_result_sucess(
        &self,
        dao_transaction: D::Transaction,
        transaction_id: Uuid,
        invoice_id: Uuid,
        origin: TransactionOrigin,
        transfer: GeneralChainTransfer,
    ) -> Result<(), ChainExecutorError> {
        let chain_transaction_id = transfer.general_transaction_id();

        dao_transaction
            .update_transaction_successful(
                transaction_id,
                chain_transaction_id,
                // TODO: use transfer.timestamp
                Utc::now(),
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    "Failed to update transaction as successful in database",
                );

                ChainExecutorError::DaoTransactionError {
                    reason: "Failed to update transaction as successful in database".to_string(),
                }
            })?;

        #[expect(clippy::single_match)]
        match origin.variant() {
            TransactionOriginVariant::Payout(payout_id) => {
                dao_transaction
                    .update_payout_status(payout_id, PayoutStatus::Completed)
                    .await
                    .map_err(|e| {
                        tracing::error!(
                            error = %e,
                            "Failed to update payout as completed in database",
                        );

                        ChainExecutorError::DaoTransactionError {
                            reason: "Failed to update payout as completed in database".to_string(),
                        }
                    })?;
            },
            // TODO: should be implemented later, not necessary for now
            _ => {},
        }

        dao_transaction
            .commit()
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    "Failed to commit database transaction while handling transfer result",
                );

                ChainExecutorError::DaoTransactionError {
                    reason: "Failed to commit database transaction".to_string(),
                }
            })?;

        tracing::info!(
            transaction_id = %transaction_id,
            invoice_id = %invoice_id,
            chain = ?transfer.chain,
            "Transfer completed successfully",
        );

        Ok(())
    }

    #[instrument(skip(self, dao_transaction, origin, error))]
    async fn handle_transfer_result_error(
        &self,
        dao_transaction: D::Transaction,
        transaction_id: Uuid,
        invoice_id: Uuid,
        origin: TransactionOrigin,
        error: TransactionExecutionError,
    ) -> Result<(), ChainExecutorError> {
        dao_transaction
            .update_transaction_failed(
                transaction_id,
                error.transaction_id,
                error
                    .retry_meta
                    .failure_message
                    .clone()
                    .unwrap_or_default(),
                Utc::now(),
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    "Failed to update transaction as failed in database",
                );

                ChainExecutorError::DaoTransactionError {
                    reason: "Failed to update transaction as failed in database".to_string(),
                }
            })?;

        #[expect(clippy::single_match)]
        match origin.variant() {
            TransactionOriginVariant::Payout(payout_id) => {
                dao_transaction
                    .update_payout_retry(
                        payout_id,
                        error.retry_meta,
                        error.is_retriable,
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!(
                            error = %e,
                            "Failed to update payout retry metadata in database",
                        );

                        ChainExecutorError::DaoTransactionError {
                            reason: "Failed to update payout retry metadata in database"
                                .to_string(),
                        }
                    })?;
            },
            // TODO: should be implemented later, not necessary for now
            _ => {},
        }

        dao_transaction
            .commit()
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    "Failed to commit database transaction while handling transfer error",
                );

                ChainExecutorError::DaoTransactionError {
                    reason: "Failed to commit database transaction".to_string(),
                }
            })?;

        tracing::warn!(
            transaction_id = %transaction_id,
            invoice_id = %invoice_id,
            is_retriable = error.is_retriable,
            "Transfer execution failed",
        );

        Ok(())
    }

    #[instrument(
        skip(self, result),
        fields(
            transaction_id = %result.transaction_id,
            invoice_id = %result.invoice_id,
        ),
    )]
    async fn handle_transfer_result(
        &self,
        result: TransactionExecutionData,
    ) -> Result<(), ChainExecutorError> {
        let dao_transaction = self
            .dao
            .begin_transaction()
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    "Failed to start database transaction",
                );

                ChainExecutorError::DaoTransactionError {
                    reason: "Failed to start database transaction".to_string(),
                }
            })?;

        match result.result {
            Ok(transfer) => {
                self.handle_transfer_result_sucess(
                    dao_transaction,
                    result.transaction_id,
                    result.invoice_id,
                    result.origin,
                    transfer,
                )
                .await
            },
            Err(error) => {
                self.handle_transfer_result_error(
                    dao_transaction,
                    result.transaction_id,
                    result.invoice_id,
                    result.origin,
                    error,
                )
                .await
            },
        }
    }

    async fn perform(
        &self,
        token: CancellationToken,
    ) {
        let mut futures_set: FuturesUnordered<BoxedTransferFuture> = FuturesUnordered::new();
        let mut polling_interval = interval(Duration::from_millis(
            POLLING_INTERVAL_MILLIS,
        ));

        tracing::info!("Transfers executor started for AssetHub and Polygon chains.");

        loop {
            tokio::select! {
                biased;

                () = token.cancelled() => {
                    tracing::info!("Cancellation requested, finishing pending transfers...");
                    break;
                },
                // First check if there are any results ready
                Some(result) = futures_set.next() => {
                    if let Err(e) = self.handle_transfer_result(result).await {
                        tracing::error!(error = %e, "Failed to handle transfer result");
                    }
                },
                // Then schedule more transfers
                _ = polling_interval.tick() => {
                    if let Err(e) = self.schedule_transfers(&mut futures_set).await {
                        tracing::error!(error = %e, "Failed to schedule transfers");
                    }
                },
            }
        }

        // Wait for all pending transfers to complete before exiting
        while let Some(result) = futures_set.next().await {
            if let Err(e) = self
                .handle_transfer_result(result)
                .await
            {
                tracing::error!(error = %e, "Failed to handle transfer result during shutdown");
            }
        }

        tracing::info!("Transfers executor has been shut down.");
    }

    pub fn new(
        asset_hub_client: AH,
        polygon_client: PG,
        dao: D,
        keyring_client: KeyringClient,
    ) -> Self {
        Self {
            asset_hub_client: Arc::new(asset_hub_client),
            polygon_client: Arc::new(polygon_client),
            dao,
            keyring_client,
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
    use mockall::predicate;

    use crate::chain_client::MockBlockChainClient;
    use crate::dao::MockDaoInterface;
    use crate::types::default_payout;

    use super::*;

    #[tokio::test]
    async fn test_collect_pending_payout_requests() {
        let keyring_client = KeyringClient::default();

        let mut dao = MockDaoInterface::new();

        dao.expect_get_pending_payouts()
            .once()
            .with(predicate::eq(10))
            .returning(|_| Ok(vec![default_payout(Uuid::new_v4())]));

        let asset_hub_client = MockBlockChainClient::<AssetHubChainConfig>::default();
        let polygon_client = MockBlockChainClient::<PolygonChainConfig>::default();

        let executor = TransfersExecutor::new(
            asset_hub_client,
            polygon_client,
            dao,
            keyring_client,
        );

        let requests = executor
            .collect_pending_payout_requests(10)
            .await
            .unwrap();

        assert_eq!(requests.len(), 1);
    }
}
