use std::collections::HashMap;

use futures::{
    StreamExt,
    stream,
};
use rust_decimal::prelude::{
    Decimal,
    ToPrimitive,
};
use subxt::blocks::{
    Block,
    ExtrinsicDetails,
    FoundExtrinsic,
};
use subxt::config::{
    DefaultExtrinsicParams,
    DefaultExtrinsicParamsBuilder,
    ExtrinsicParams,
};
use subxt::utils::H256;
use subxt::{
    Config,
    SubstrateConfig,
};
use tracing::{
    debug,
    instrument,
    warn,
};

use crate::types::ChainType;

use super::{
    AssetInfo,
    AssetInfoStore,
    BlockChainClient,
    BlockChainClientExt,
    ChainConfig,
    ChainTransfer,
    ClientError,
    GeneralTransactionId,
    KeyringClient,
    QueryError,
    SignedTransaction,
    SignedTransactionUtils,
    SubscriptionError,
    TransactionError,
    TransfersStream,
    UnsignedTransaction,
};

use super::errors::is_insufficient_balance_error;
use super::keyring::SignTransactionRequestData;

#[subxt::subxt(
    runtime_metadata_path = "../metadata.scale",
    generate_docs,
    derive_for_type(
        path = "staging_xcm::v3::multilocation::MultiLocation",
        derive = "Clone, subxt::ext::codec::Encode",
        recursive
    )
)]
pub mod runtime {}

use runtime::runtime_types::staging_xcm::v3::multilocation::MultiLocation;
use runtime::runtime_types::xcm::v3::junction::Junction;
use runtime::runtime_types::xcm::v3::junctions::Junctions;

const DEFAULT_MORTALITY: u64 = 32;
const DEFAULT_MULTILOCATION_PARENTS: u8 = 0;
const DEFAULT_PALLET_INSTANCE: u8 = 50;

// We don't need to construct this at runtime, so an empty enum is appropriate.
#[derive(Debug)]
pub enum SubxtAssetHubConfig {}

impl Config for SubxtAssetHubConfig {
    type AccountId = <SubstrateConfig as Config>::AccountId;
    type Address = <SubstrateConfig as Config>::Address;
    // Here we use the MultiLocation from the metadata as a part of the config:
    // The `ChargeAssetTxPayment` signed extension that is part of the
    // ExtrinsicParams above, now uses the type:
    type AssetId = MultiLocation;
    type ExtrinsicParams = DefaultExtrinsicParams<SubxtAssetHubConfig>;
    type Hasher = <SubstrateConfig as Config>::Hasher;
    type Header = <SubstrateConfig as Config>::Header;
    type Signature = <SubstrateConfig as Config>::Signature;
}

type SubxtAssetHubClient = subxt::OnlineClient<SubxtAssetHubConfig>;

// Runtime type aliases for Asset Hub transfer operations
type TransferExtrinsic = runtime::assets::calls::types::Transfer;
type TransferAllExtrinsic = runtime::assets::calls::types::TransferAll;
type TransferredEvent = runtime::assets::events::Transferred;

pub type AssetHubUnsignedTransaction =
    subxt::tx::PartialTransaction<SubxtAssetHubConfig, SubxtAssetHubClient>;
pub type AssetHubSignedTransaction =
    subxt::tx::SubmittableTransaction<SubxtAssetHubConfig, SubxtAssetHubClient>;
pub type AssetHubAccountId = subxt::utils::AccountId32;

impl SignedTransactionUtils for AssetHubSignedTransaction {
    fn to_raw_string(&self) -> String {
        const_hex::encode_prefixed(self.encoded())
    }

    fn hash(&self) -> String {
        self.hash().to_string()
    }
}

#[derive(Debug, Clone)]
pub enum AssetHubChainConfig {}

impl ChainConfig for AssetHubChainConfig {
    type AccountId = AssetHubAccountId;
    type AssetId = u32;
    type BlockHash = H256;
    type SignedTransaction = AssetHubSignedTransaction;
    // (block number, position in block)
    type TransactionHash = H256;
    type TransactionId = (u32, u32);
    type UnsignedTransaction = AssetHubUnsignedTransaction;

    const CHAIN_TYPE: ChainType = ChainType::PolkadotAssetHub;
}

impl From<(u32, u32)> for GeneralTransactionId {
    fn from(value: (u32, u32)) -> Self {
        GeneralTransactionId {
            block_number: Some(value.0),
            position_in_block: Some(value.1),
            tx_hash: None,
        }
    }
}

enum AnyTransferExtrinsic {
    Transfer(FoundExtrinsic<SubxtAssetHubConfig, SubxtAssetHubClient, TransferExtrinsic>),
    TransferAll(FoundExtrinsic<SubxtAssetHubConfig, SubxtAssetHubClient, TransferAllExtrinsic>),
}

impl AnyTransferExtrinsic {
    pub fn details(&self) -> &ExtrinsicDetails<SubxtAssetHubConfig, SubxtAssetHubClient> {
        match self {
            AnyTransferExtrinsic::Transfer(e) => &e.details,
            AnyTransferExtrinsic::TransferAll(e) => &e.details,
        }
    }
}

#[derive(Clone)]
pub struct AssetHubClient {
    config: crate::configs::ChainConfig,
    client: SubxtAssetHubClient,
    asset_info_store: AssetInfoStore<AssetHubChainConfig>,
}

impl AssetHubClient {
    #[instrument(skip(config, asset_info_store))]
    async fn from_config(
        config: &crate::configs::ChainConfig,
        asset_info_store: AssetInfoStore<AssetHubChainConfig>,
    ) -> Result<Self, ClientError> {
        // TODO: implement circuit breaker for endpoints
        // (should be another wrapper structure with endpoints hidden behind sync
        // primitives with error counters and usage timeouts)
        let endpoint = config
            .get_random_endpoint()
            .ok_or(ClientError::InvalidConfiguration {
                field: "endpoints".to_string(),
            })?;

        tracing::debug!(
            url = endpoint,
            chain = %Self::chain_type(),
            "Trying to connect to endpoint...",
        );

        let client = if config.allow_insecure_endpoints {
            SubxtAssetHubClient::from_insecure_url(&endpoint).await
        } else {
            SubxtAssetHubClient::from_url(&endpoint).await
        }
        .inspect_err(|e| {
            tracing::debug!(
                error.category = crate::utils::logging::category::CHAIN_CLIENT,
                error.operation = crate::utils::logging::operation::CONNECT_CLIENT,
                error.source = ?e,
                chain = %Self::chain_type(),
                endpoint = %endpoint,
                "Failed to connect to Asset Hub RPC endpoint"
            );
        })
        .map_err(|_| ClientError::AllEndpointsUnreachable)?;

        Ok(AssetHubClient {
            config: config.clone(),
            client,
            asset_info_store,
        })
    }

    #[instrument(skip(self))]
    async fn fetch_block_by_hash(
        &self,
        block_hash: H256,
    ) -> Result<Block<SubxtAssetHubConfig, SubxtAssetHubClient>, QueryError> {
        self.client
            .blocks()
            .at(block_hash)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.source = ?e,
                    block_hash = ?block_hash,
                    "Failed to fetch finalized block information"
                );
            })
            .map_err(|_| QueryError::RpcRequestFailed)
    }

    #[instrument(skip(self, block, assets), fields(block_number = block.number()))]
    async fn process_block(
        &self,
        block: Block<SubxtAssetHubConfig, SubxtAssetHubClient>,
        assets: &HashMap<u32, AssetInfo<AssetHubChainConfig>>,
    ) -> Result<Vec<ChainTransfer<AssetHubChainConfig>>, SubscriptionError> {
        // Implementation for processing a block
        let block_number = block.number();

        // Extract timestamp from storage
        let timestamp = match block
            .storage()
            .fetch(&runtime::storage().timestamp().now())
            .await
        {
            Ok(Some(ts)) => ts,
            #[expect(clippy::cast_sign_loss)]
            Ok(None) => {
                tracing::warn!("Block {block_number} missing timestamp, using 0");
                // TODO: fix expects. Maybe just use `chrono::DateTime`?
                chrono::Utc::now().timestamp_millis() as u64
            },
            #[expect(clippy::cast_sign_loss)]
            Err(e) => {
                tracing::warn!("Failed to fetch timestamp for block {block_number}: {e}");
                chrono::Utc::now().timestamp_millis() as u64
            },
        };

        // Get extrinsics
        let extrinsics = match block.extrinsics().await {
            Ok(e) => e,
            Err(e) => {
                tracing::error!("Failed to fetch extrinsics for block {block_number}: {e}");
                return Err(
                    SubscriptionError::BlockProcessingFailed {
                        block_number,
                    },
                );
            },
        };

        // Find transfer and transfer_all extrinsics
        // TODO: Handle errors in decoding extrinsics
        let transfer_extrinsics = extrinsics
            .find::<TransferExtrinsic>()
            .filter_map(Result::ok)
            .map(AnyTransferExtrinsic::Transfer);

        let transfer_all_extrinsics = extrinsics
            .find::<TransferAllExtrinsic>()
            .filter_map(Result::ok)
            .map(AnyTransferExtrinsic::TransferAll);

        let all_transfer_extrinsics = transfer_extrinsics.chain(transfer_all_extrinsics);

        let events = stream::iter(all_transfer_extrinsics)
            .filter_map(|ext| async move {
                let index = ext.details().index();

                ext.details()
                    .events()
                    .await
                    .ok()
                    .map(|evs| (index, evs))
            })
            .collect::<Vec<_>>()
            .await;

        let transfers = events
            .into_iter()
            .flat_map(|(index, events)| {
                events
                    .find::<TransferredEvent>()
                    .filter_map(Result::ok)
                    .filter_map(|event| {
                        let asset_info = assets.get(&event.asset_id)?;

                        Some(ChainTransfer {
                            asset_id: event.asset_id,
                            asset_name: asset_info.name.clone(),
                            // TODO: check event.amount? Cast is quite unsafe
                            #[expect(clippy::cast_possible_truncation)]
                            amount: Decimal::new(
                                event.amount as i64,
                                asset_info.decimals.into(),
                            ),
                            sender: event.from,
                            recipient: event.to,
                            transaction_id: (block_number, index),
                            timestamp,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        Ok(transfers)
    }

    #[expect(clippy::unused_self)]
    fn build_tx_config(
        &self,
        asset_id: u32,
    ) -> <DefaultExtrinsicParams<SubxtAssetHubConfig> as ExtrinsicParams<SubxtAssetHubConfig>>::Params
    {
        let location = MultiLocation {
            parents: DEFAULT_MULTILOCATION_PARENTS,
            interior: Junctions::X2(
                Junction::PalletInstance(DEFAULT_PALLET_INSTANCE),
                Junction::GeneralIndex(u128::from(asset_id)),
            ),
        };

        DefaultExtrinsicParamsBuilder::<SubxtAssetHubConfig>::new()
            .tip_of(0, location)
            .mortal(DEFAULT_MORTALITY)
            .build()
    }
}

impl BlockChainClient<AssetHubChainConfig> for AssetHubClient {
    // TODO: need to add validation on startup.
    // Iterate over all provided RPC URLs and ensure they all belongs to the
    // configured chain
    fn chain_name(&self) -> &'static str {
        "statemint"
    }

    fn asset_info_store(&self) -> &AssetInfoStore<AssetHubChainConfig> {
        &self.asset_info_store
    }

    #[instrument(skip(config))]
    async fn new(config: &crate::configs::ChainConfig) -> Result<Self, ClientError> {
        AssetHubClient::from_config(config, AssetInfoStore::new()).await
    }

    #[instrument(skip(config, asset_info_store))]
    async fn new_with_store(
        config: &crate::configs::ChainConfig,
        asset_info_store: AssetInfoStore<AssetHubChainConfig>,
    ) -> Result<Self, ClientError> {
        AssetHubClient::from_config(config, asset_info_store).await
    }

    #[instrument(skip(self))]
    async fn recreate(&self) -> Result<Self, ClientError> {
        Self::from_config(
            &self.config,
            self.asset_info_store.clone(),
        )
        .await
    }

    #[instrument(skip(self))]
    async fn fetch_asset_info(
        &self,
        asset_id: &u32,
    ) -> Result<AssetInfo<AssetHubChainConfig>, QueryError> {
        debug!(message = "Trying to fetch asset info...");
        let request_data = runtime::storage()
            .assets()
            .metadata(*asset_id);

        self.client
            .storage()
            .at_latest()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::FETCH_STORAGE,
                    error.source = ?e,
                    asset_id = %asset_id,
                    "Failed to get latest storage"
                );
            })
            .map_err(|_e| QueryError::RpcRequestFailed)?
            .fetch(&request_data)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::FETCH_ASSET_INFO,
                    error.source = ?e,
                    asset_id = %asset_id,
                    "Failed to fetch asset metadata from storage"
                );
            })
            .map_err(|_e| QueryError::RpcRequestFailed)?
            .ok_or_else(|| QueryError::NotFound {
                query_type: format!("asset metadata for asset {asset_id}"),
            })
            .inspect_err(|_| warn!(message = "Asset metadata wasn't found (None returned)"))
            .map(|resp| AssetInfo {
                id: *asset_id,
                name: String::from_utf8(resp.symbol.0)
                    .inspect_err(|e| {
                        tracing::warn!(
                            asset_id = %asset_id,
                            error = ?e,
                            "Asset symbol contains invalid UTF-8, using fallback"
                        );
                    })
                    .unwrap_or_else(|_| format!("Asset_{asset_id}")),
                decimals: resp.decimals,
            })
            .inspect(|val| debug!(message = "Asset info fetched successfully", asset_info = ?val))
    }

    // TODO: probably will be better to return some `Balance` structure with asset
    // id and account id
    #[instrument(skip(self))]
    async fn fetch_asset_balance(
        &self,
        asset_id: &u32,
        account_id: &AssetHubAccountId,
    ) -> Result<Decimal, QueryError> {
        debug!("Trying to fetch asset balance...");

        let decimals = self
            .asset_info_store
            .get_asset_info(asset_id)
            .await
            .or_else(|| {
                warn!("AssetInfo wasn't found in local AssetInfoStore");
                None
            })
            .ok_or_else(|| QueryError::NotFound {
                query_type: format!("asset info for asset {asset_id}"),
            })?
            .decimals;

        let request_data = runtime::storage()
            .assets()
            .account(*asset_id, account_id.clone());

        let amount = self
            .client
            .storage()
            .at_latest()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::FETCH_STORAGE,
                    error.source = ?e,
                    asset_id = %asset_id,
                    account = %account_id,
                    "Failed to get latest storage"
                );
            })
            .map_err(|_e| QueryError::RpcRequestFailed)?
            .fetch(&request_data)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::FETCH_BALANCE,
                    error.source = ?e,
                    asset_id = %asset_id,
                    account = %account_id,
                    "Failed to fetch balance from storage"
                );
            })
            .map_err(|_e| QueryError::RpcRequestFailed)?
            .map_or(Decimal::ZERO, |acc| {
                // TODO: check acc.balance? Cast is quite unsafe
                #[expect(clippy::cast_possible_truncation)]
                Decimal::new(acc.balance as i64, decimals.into())
            });

        Ok(amount)
    }

    #[instrument(skip(self))]
    async fn subscribe_transfers(
        &self,
        asset_ids: &[u32],
    ) -> Result<TransfersStream<AssetHubChainConfig>, SubscriptionError> {
        let client = self.clone();

        let assets = self
            .asset_info_store
            .get_assets_info(asset_ids)
            .await;

        for asset_id in asset_ids {
            if !assets.contains_key(asset_id) {
                return Err(SubscriptionError::AssetNotFound {
                    asset_id: *asset_id,
                });
            }
        }

        // Subscribe to finalized blocks
        let mut blocks = client
            .client
            .blocks()
            .subscribe_finalized()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::SUBSCRIBE_TRANSFERS,
                    error.source = ?e,
                    "Failed to subscribe to finalized blocks"
                );
            })
            .map_err(|_| SubscriptionError::SubscriptionFailed)?;

        let stream = async_stream::try_stream! {
            // Process each block
            while let Some(block_result) = blocks.next().await {
                let block = block_result
                    .inspect_err(|e| {
                        tracing::debug!(
                            error.category = crate::utils::logging::category::CHAIN_CLIENT,
                            error.operation = crate::utils::logging::operation::SUBSCRIBE_TRANSFERS,
                            error.source = ?e,
                            "Block subscription stream closed or errored"
                        );
                    })
                    .map_err(|_e| SubscriptionError::StreamClosed)?;

                let result = client.process_block(block, &assets).await?;

                if !result.is_empty() {
                    yield result
                }
            }

            // Stream ended naturally (connection closed)
            tracing::info!("Block subscription stream ended");
        };

        Ok(Box::pin(stream))
    }

    #[instrument(skip(self))]
    async fn init_asset_info(
        &self,
        asset_ids: &[String],
    ) -> Result<(), ClientError> {
        // Delegate to the extension trait default implementation
        BlockChainClientExt::init_asset_info_impl(self, asset_ids).await
    }

    #[instrument(skip(self), fields(asset_id = %asset_id, amount = %amount))]
    async fn build_transfer(
        &self,
        sender: &AssetHubAccountId,
        recipient: &AssetHubAccountId,
        asset_id: &u32,
        amount: Decimal,
    ) -> Result<UnsignedTransaction<AssetHubChainConfig>, TransactionError<AssetHubChainConfig>>
    {
        // TODO: in order to support native asset, we need to check if asset_id = 0 and
        // use other methods to build transfer_all for native asset
        let decimals = self
            .asset_info_store()
            .get_asset_info(asset_id)
            .await
            .ok_or_else(|| TransactionError::BuildFailed {
                reason: format!("Asset ID {asset_id} not found in asset info store"),
            })?
            .decimals;

        #[expect(clippy::arithmetic_side_effects)]
        let normalized_amount = amount / Decimal::new(1, decimals.into());

        let transaction_amount = normalized_amount
            .to_u128()
            .ok_or_else(|| {
                tracing::error!(
                    amount = %amount,
                    normalized = %normalized_amount,
                    "Amount exceeds u128::MAX after normalization"
                );
                TransactionError::BuildFailed {
                    reason: format!("Amount {amount} exceeds u128::MAX after normalization"),
                }
            })?;

        let tx_config = self.build_tx_config(*asset_id);

        let call = runtime::tx().assets().transfer(
            *asset_id,
            recipient.clone().into(),
            transaction_amount,
        );

        let transaction = self
            .client
            .tx()
            .create_partial(&call, sender, tx_config)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::BUILD_TRANSFER,
                    error.source = ?e,
                    asset_id = %asset_id,
                    amount = %amount,
                    "Failed to create partial transaction"
                );
            })
            .map_err(|_e| TransactionError::BuildFailed {
                reason: "Failed to create partial transaction".to_string(),
            })?;

        Ok(UnsignedTransaction {
            transaction,
        })
    }

    #[instrument(skip(self), fields(asset_id = %asset_id))]
    async fn build_transfer_all(
        &self,
        sender: &AssetHubAccountId,
        recipient: &AssetHubAccountId,
        asset_id: &u32,
    ) -> Result<UnsignedTransaction<AssetHubChainConfig>, TransactionError<AssetHubChainConfig>>
    {
        // TODO: in order to support native asset, we need to check if asset_id = 0 and
        // use other methods to build transfer_all for native asset
        let tx_config = self.build_tx_config(*asset_id);

        let call = runtime::tx().assets().transfer_all(
            *asset_id,
            recipient.clone().into(),
            false,
        );

        let transaction = self
            .client
            .tx()
            .create_partial(&call, sender, tx_config)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::BUILD_TRANSFER,
                    error.source = ?e,
                    asset_id = %asset_id,
                    "Failed to create partial transaction for transfer_all"
                );
            })
            .map_err(|_e| TransactionError::BuildFailed {
                reason: "Failed to create partial transaction for transfer_all".to_string(),
            })?;

        Ok(UnsignedTransaction {
            transaction,
        })
    }

    #[instrument(skip(self, transaction, keyring_client))]
    async fn sign_transaction(
        &self,
        transaction: UnsignedTransaction<AssetHubChainConfig>,
        derivation_params: Vec<String>,
        keyring_client: &KeyringClient,
    ) -> Result<SignedTransaction<AssetHubChainConfig>, TransactionError<AssetHubChainConfig>> {
        let data = SignTransactionRequestData {
            transaction: transaction.transaction,
            derivation_params,
        };

        let transaction = keyring_client
            .sign_asset_hub_transaction(data)
            .await?;

        Ok(SignedTransaction {
            transaction,
        })
    }

    // TODO: inspect too_many_lines
    #[expect(clippy::too_many_lines)]
    #[instrument(skip(self, transaction), fields(transaction_hash = %transaction.transaction.hash()))]
    async fn submit_and_watch_transaction(
        &self,
        transaction: SignedTransaction<AssetHubChainConfig>,
    ) -> Result<ChainTransfer<AssetHubChainConfig>, TransactionError<AssetHubChainConfig>> {
        let SignedTransaction {
            transaction,
        } = transaction;

        let tx_hash = transaction.hash();

        // Submit the transaction and wait for it's finalization
        let tx_progress = transaction
            .submit_and_watch()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::SUBMIT_TRANSACTION,
                    error.source = ?e,
                    transaction_hash = %tx_hash,
                    "Transaction submission failed"
                );
            })
            .map_err(|_| TransactionError::SubmissionStatusUnknown)?;

        // Wait for tx finalization. We don't really know neither it's status or block
        // info at this point
        let finalized_tx = tx_progress
            .wait_for_finalized()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::WATCH_TRANSACTION,
                    error.source = ?e,
                    transaction_hash = %tx_hash,
                    "Failed to watch transaction finalization"
                );
            })
            .map_err(|_| TransactionError::SubmissionStatusUnknown)?;

        // At this point we know that transaction was finalized and included in block
        let block_hash = finalized_tx.block_hash();

        // We still need to fetch some additional block info like it's number and
        // timestamp
        let block = self
            .fetch_block_by_hash(block_hash)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::WATCH_TRANSACTION,
                    error.source = ?e,
                    block_hash = ?block_hash,
                    "Failed to fetch finalized block information"
                );
            })
            .map_err(|_| TransactionError::SubmissionStatusUnknown)?;

        let block_number = block.number();

        let events = finalized_tx
            .fetch_events()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::WATCH_TRANSACTION,
                    error.source = ?e,
                    transaction_hash = %tx_hash,
                    block_number = %block_number,
                    block_hash = ?block_hash,
                    "Failed to fetch transaction events from finalized block"
                );
            })
            .map_err(|_| TransactionError::SubmissionStatusUnknown)?;

        // We finally have extrinsic index and it's events so we can find extrinsic
        // status
        let extrinsic_index = events.extrinsic_index();
        let transaction_id = (block_number, extrinsic_index);

        let error_extrinsic = events
            .find_first::<runtime::system::events::ExtrinsicFailed>()
            .map_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::WATCH_TRANSACTION,
                    error.source = ?e,
                    transaction_hash = %tx_hash,
                    block_number = %block_number,
                    extrinsic_index = %extrinsic_index,
                    "Failed to decode ExtrinsicFailed event"
                );
                TransactionError::TransactionInfoFetchFailed {
                    transaction_id,
                }
            })?;

        // Check if transaction failed on-chain
        if let Some(failed_event) = error_extrinsic {
            let dispatch_error = &failed_event.dispatch_error;

            // Discriminate error types based on runtime error
            if is_insufficient_balance_error(dispatch_error) {
                return Err(TransactionError::InsufficientBalance {
                    transaction_id,
                });
            }

            // Generic execution failure
            let error_code = format!("{dispatch_error:?}");
            return Err(TransactionError::ExecutionFailed {
                transaction_id,
                error_code,
            });
        }

        let event = events
            .find_first::<TransferredEvent>()
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::WATCH_TRANSACTION,
                    error.source = ?e,
                    transaction_hash = %tx_hash,
                    block_number = %block_number,
                    extrinsic_index = %extrinsic_index,
                    "Failed to decode Transferred event"
                );
            })
            .map_err(
                |_| TransactionError::TransactionInfoFetchFailed {
                    transaction_id,
                },
            )?
            .ok_or_else(|| {
                tracing::debug!(
                    error.category = crate::utils::logging::category::CHAIN_CLIENT,
                    error.operation = crate::utils::logging::operation::WATCH_TRANSACTION,
                    transaction_hash = %tx_hash,
                    block_number = %block_number,
                    extrinsic_index = %extrinsic_index,
                    "No Transferred event found for successful transaction"
                );
                TransactionError::TransactionInfoFetchFailed {
                    transaction_id,
                }
            })?;

        let asset_info = self
            .asset_info_store()
            .get_asset_info(&event.asset_id)
            .await
            .ok_or(TransactionError::UnknownAsset {
                transaction_id: (block_number, extrinsic_index),
                asset_id: event.asset_id,
            })?;

        // TODO: check event.amount, cast is unsafe
        #[expect(clippy::cast_possible_truncation)]
        let amount = Decimal::new(
            event.amount as i64,
            asset_info.decimals.into(),
        );

        Ok(ChainTransfer {
            amount,
            asset_id: event.asset_id,
            asset_name: asset_info.name.clone(),
            sender: event.from,
            recipient: event.to,
            transaction_id: (block_number, extrinsic_index),
            // TODO: fetch block's timestamp
            #[expect(clippy::cast_sign_loss)]
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
        })
    }
}
