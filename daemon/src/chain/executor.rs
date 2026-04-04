use std::future::Future;
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
use crate::swaps::SwapsExecutor;
use crate::types::{
    ChainType, CreateSwapData, GeneralTransactionId, OutgoingTransaction, Payout, PayoutStatus, Refund, RefundStatus, RetryMeta, SwapChainType, SwapDirection, SwapExecutorType, SwapSignatureParams, Transaction, TransactionOrigin, TransactionOriginVariant, TransferDestinationParams, TransferInfo
};
use crate::utils::RefundDestinationDetector;

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
    #[error("Swap from {from_chain} to {to_chain} is not supported")]
    UnsupportedSwapDirection {
        from_chain: SwapChainType,
        to_chain: SwapChainType,
    }
}

impl ChainExecutorError {
    pub fn is_retriable(&self) -> bool {
        use ChainExecutorError::*;

        match self {
            FetchTransfers => true,
            BuildTransfer { .. } => true,
            DaoTransactionError { .. } => true,
            UnsupportedSwapDirection { .. } => false,
        }
    }
}

const MAX_CONCURRENT_TRANSFERS: u32 = 10;
const POLLING_INTERVAL_MILLIS: u64 = 100;

#[derive(Debug, Clone)]
pub struct OutgoingTransferRequest {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub chain: ChainType,
    pub asset_id: String,
    pub asset_name: String,
    pub source_address: String,
    pub destination_params: TransferDestinationParams,
    pub amount: Decimal,
    pub origin: TransactionOrigin,
    pub retry_meta: RetryMeta,
}

impl From<Payout> for OutgoingTransferRequest {
    fn from(value: Payout) -> Self {
        Self {
            id: value.id,
            invoice_id: value.invoice_id,
            chain: value.chain,
            asset_id: value.asset_id,
            asset_name: value.asset_name,
            source_address: value.source_address,
            destination_params: value.destination_params,
            amount: value.amount,
            origin: TransactionOrigin::payout(value.id),
            retry_meta: value.retry_meta,
        }
    }
}

impl TryFrom<Refund> for OutgoingTransferRequest {
    type Error = ();

    fn try_from(value: Refund) -> Result<Self, Self::Error> {
        let Some(destination_params) = value.destination_params else {
            return Err(())
        };

        Ok(Self {
            id: value.id,
            invoice_id: value.invoice_id,
            chain: value.chain,
            asset_id: value.asset_id,
            asset_name: value.asset_name,
            source_address: value.source_address,
            destination_params,
            amount: value.amount,
            origin: TransactionOrigin::refund(value.id),
            retry_meta: value.retry_meta,
        })
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
    refund_destination_detector: RefundDestinationDetector<D>,
    asset_hub_client: Arc<AH>,
    polygon_client: Arc<PG>,
    dao: D,
    swaps_executor: SwapsExecutor<D>,
    keyring_client: KeyringClient,
}

type BoxedTransferFuture = std::pin::Pin<Box<dyn Future<Output = TransactionExecutionData> + Send>>;

async fn send_transfer_request<T: ChainConfig, C: BlockChainClient<T>>(
    client: Arc<C>,
    signed_transaction: SignedTransaction<T>,
    request: OutgoingTransferRequest,
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
                is_retriable: true,
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
    ) -> Result<Vec<OutgoingTransferRequest>, ChainExecutorError> {
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
            .map(From::from)
            .collect();

        Ok(payout_requests)
    }

    async fn collect_pending_refund_requests(
        &self,
        limit: u32,
    ) -> Vec<OutgoingTransferRequest> {
        if limit == 0 {
            return vec![]
        }

        self.refund_destination_detector
            .get_refunds_with_destination(limit)
            .await
            .into_iter()
            // refund destination detector actually either extend refund record with
            // destination params or filter it out so TryFrom will always success here
            // but leave it as is for now. Don't want to make it From with unwrap just
            // to avoid accidents if code changes in future
            .map(TryFrom::try_from)
            .filter_map(Result::ok)
            .collect()
    }

    #[instrument(skip_all)]
    async fn build_and_sign_transfer<T: ChainConfig, C: BlockChainClient<T>>(
        &self,
        client: &Arc<C>,
        request: &OutgoingTransferRequest,
    ) -> Result<SignedTransaction<T>, ChainExecutorError> {
        let sender = request.source_address
            .parse()
            .map_err(|_| ChainExecutorError::BuildTransfer { reason: "Invalid source address".to_string() })?;

        let recipient = request.destination_params.destination_address
            .parse()
            .map_err(|_| ChainExecutorError::BuildTransfer { reason: "Invalid destination address".to_string() })?;

        let asset_id = request.asset_id
            .parse()
            .map_err(|_| ChainExecutorError::BuildTransfer { reason: "Invalid asset id".to_string() })?;

        // TODO: build common transfer with amount specified, not transfer all
        let transaction = client
            .build_transfer(
                &sender,
                &recipient,
                &asset_id,
                request.amount,
            )
            .await
            .map_err(|e| {
                tracing::warn!(
                    invoice_id = %request.invoice_id,
                    payout_id = %request.id,
                    error = ?e,
                    "Failed to build transfer transaction",
                );

                ChainExecutorError::BuildTransfer {
                    reason: format!("Failed to build transfer transaction: {e}"),
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

    #[instrument(skip_all)]
    async fn store_built_transfer<T: ChainConfig>(
        &self,
        request: &OutgoingTransferRequest,
        signed_transaction: &SignedTransaction<T>,
    ) -> Result<Transaction, ChainExecutorError> {
        let outgoing = OutgoingTransaction {
            id: Uuid::new_v4(),
            invoice_id: request.invoice_id,
            transfer_info: TransferInfo {
                chain: request.chain,
                asset_id: request.asset_id.clone(),
                asset_name: request.asset_name.clone(),
                amount: request.amount,
                source_address: request.source_address.clone(),
                destination_address: request.destination_params.destination_address.clone(),
            },
            tx_hash: signed_transaction.hash(),
            transaction_bytes: signed_transaction.to_raw_string(),
            origin: request.origin.clone(),
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

    #[instrument(skip_all)]
    async fn prepare_transfer<T: ChainConfig + 'static, C: BlockChainClient<T> + 'static>(
        &self,
        client: Arc<C>,
        request: OutgoingTransferRequest,
    ) -> Result<BoxedTransferFuture, ChainExecutorError> {
        let signed_transaction = self
            .build_and_sign_transfer(&client, &request)
            .await?;

        let transaction = self
            .store_built_transfer(&request, &signed_transaction)
            .await?;

        tracing::trace!(
            transaction_id = %transaction.id,
            "Built and stored outgoing transaction"
        );

        let fut = Box::pin(send_transfer_request(
            client,
            signed_transaction,
            request,
            transaction,
        ));

        Ok(fut)
    }

    #[tracing::instrument(skip_all)]
    async fn schedule_chain_transfer(
        &self,
        request: OutgoingTransferRequest,
        futures_set: &mut FuturesUnordered<BoxedTransferFuture>,
    ) -> Result<(), ChainExecutorError> {
        let transfer = match request.chain {
            ChainType::PolkadotAssetHub => {
                let client = self.asset_hub_client.clone();
                self
                    .prepare_transfer(client, request)
                    .await
            },
            ChainType::Polygon => {
                let client = self.polygon_client.clone();
                self
                    .prepare_transfer(client, request)
                    .await
            },
        }?;

        tracing::info!(
            "Scheduled transfer for processing on chain",
        );

        futures_set.push(transfer);

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn schedule_swap(
        &self,
        request: OutgoingTransferRequest,
    ) -> Result<(), ChainExecutorError> {
        let from_chain = request.chain.into();
        let to_chain = request.destination_params.destination_chain;

        let Some(swap_executor) = SwapExecutorType::detect(from_chain, to_chain, SwapDirection::Outgoing) else {
            return Err(ChainExecutorError::UnsupportedSwapDirection {
                from_chain,
                to_chain,
            })
        };

        // TODO: make it more normally. Add some helpers for such operation, get precision from prestored values
        let from_amount_units = (request.amount / Decimal::new(1, 6)).to_u128().unwrap();

        let data = CreateSwapData {
            invoice_id: request.invoice_id,
            swap_executor,
            from_chain,
            to_chain,
            from_token_address: request.asset_id,
            to_token_address: request.destination_params.destination_asset_id,
            from_amount_units,
            // TODO: make this field optional
            expected_to_amount_units: 0,
            from_address: request.source_address,
            to_address: request.destination_params.destination_address,
            direction: SwapDirection::Outgoing,
            origin: request.origin,
        };

        let swap = self.swaps_executor
            .create_swap(data)
            .await
            // TODO: add normal error checking and logging
            .map_err(|e| ChainExecutorError::BuildTransfer { reason: e.to_string() })?;

        let signature = self.swaps_executor
            .sign_transaction(&self.keyring_client, &swap)
            .await
            .map_err(|e| ChainExecutorError::BuildTransfer { reason: e.to_string() })?;

        let signature_params = SwapSignatureParams {
            swap_id: swap.id,
            swap_executor,
            signature,
        };

        let _swap = self.swaps_executor
            .submit_with_signature(signature_params)
            .await
            .map_err(|e| ChainExecutorError::BuildTransfer { reason: e.to_string() })?;

        Ok(())
    }

    #[tracing::instrument(
        skip_all,
        fields(
            transfer_entity_id = %request.id,
            invoice_id = %request.invoice_id,
            source_address = %request.source_address,
            destination_address = %request.destination_params.destination_address,
            source_chain = %request.chain,
            destination_chain = %request.destination_params.destination_chain,
            source_asset_id = %request.asset_id,
            destination_asset_id = %request.destination_params.destination_asset_id,
            amount = %request.amount,
        )
    )]
    async fn send_transfer(
        &self,
        request: OutgoingTransferRequest,
        futures_set: &mut FuturesUnordered<BoxedTransferFuture>,
    ) {
        let origin = request.origin;
        let mut retry_meta = request.retry_meta.clone();

        let from_chain = SwapChainType::from(request.chain);
        let to_chain = request.destination_params.destination_chain;

        let is_same_chain = from_chain == to_chain;
        // TODO: might be inconsistensies, so make both strings lowercase now. In future it's better to make some wrapper type
        // to keep addresses consistent
        let is_same_asset_id = request.asset_id.to_lowercase() == request.destination_params.destination_asset_id.to_lowercase();

        let result = if is_same_chain && is_same_asset_id {
            self.schedule_chain_transfer(request, futures_set).await
        } else if is_same_chain {
            self.schedule_swap(request).await
        } else {
            // TODO: cross-chain outgoing swaps are not supported now
            Err(ChainExecutorError::UnsupportedSwapDirection { from_chain, to_chain })
        };

        if let Err(error) = result {
            let is_retriable = error.is_retriable();

            tracing::warn!(
                %error,
                %is_retriable,
                "Failed to prepare transfer request, it will be marked as failed"
            );
            retry_meta.increment_retry(error.to_string());

            match origin.variant() {
                TransactionOriginVariant::Payout(id) => {
                    if let Err(error) = self
                        .dao
                        .update_payout_retry(id, retry_meta, is_retriable)
                        .await
                    {
                        tracing::error!(
                            %error,
                            "Error while trying to mark payout request failed. It might stuck with In Progress status"
                        );
                    };
                },
                TransactionOriginVariant::Refund(id) => {
                    if let Err(error) = self
                        .dao
                        .update_refund_retry(id, retry_meta, is_retriable)
                        .await
                    {
                        tracing::error!(
                            %error,
                            "Error while trying to mark refund request failed. It might stuck with In Progress status"
                        );
                    };
                },
                TransactionOriginVariant::InternalTransfer(_) | TransactionOriginVariant::None => unreachable!(),
            }
        };
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

        let payout_requests = self
            .collect_pending_payout_requests(limit)
            .await?;

        let remaining_limit = limit - payout_requests.len() as u32;

        let refund_requests = self
            .collect_pending_refund_requests(remaining_limit)
            .await;

        for request in payout_requests.into_iter().chain(refund_requests) {
            self.send_transfer(request, futures_set).await;
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
            TransactionOriginVariant::Refund(refund_id) => {
                dao_transaction
                    .update_refund_status(refund_id, RefundStatus::Completed)
                    .await
                    .map_err(|e| {
                        tracing::error!(
                            error = %e,
                            "Failed to update refund as completed in database",
                        );

                        ChainExecutorError::DaoTransactionError {
                            reason: "Failed to update refund as completed in database".to_string(),
                        }
                    })?;
            },
            TransactionOriginVariant::InternalTransfer(_) | TransactionOriginVariant::None => unreachable!(),
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
            TransactionOriginVariant::Refund(refund_id) => {
                dao_transaction
                    .update_refund_retry(
                        refund_id,
                        error.retry_meta,
                        error.is_retriable,
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!(
                            error = %e,
                            "Failed to update refund retry metadata in database",
                        );

                        ChainExecutorError::DaoTransactionError {
                            reason: "Failed to update refund retry metadata in database"
                                .to_string(),
                        }
                    })?;
            },
            TransactionOriginVariant::InternalTransfer(_) | TransactionOriginVariant::None => unreachable!(),
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
        refund_destination_detector: RefundDestinationDetector<D>,
        asset_hub_client: AH,
        polygon_client: PG,
        dao: D,
        keyring_client: KeyringClient,
        swaps_executor: SwapsExecutor<D>,
    ) -> Self {
        Self {
            refund_destination_detector,
            asset_hub_client: Arc::new(asset_hub_client),
            polygon_client: Arc::new(polygon_client),
            dao,
            swaps_executor,
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
        let swaps_executor = SwapsExecutor::default();
        let refund_destination_detector = RefundDestinationDetector::default();

        let executor = TransfersExecutor::new(
            refund_destination_detector,
            asset_hub_client,
            polygon_client,
            dao,
            keyring_client,
            swaps_executor,
        );

        let requests = executor
            .collect_pending_payout_requests(10)
            .await
            .unwrap();

        assert_eq!(requests.len(), 1);
    }
}
