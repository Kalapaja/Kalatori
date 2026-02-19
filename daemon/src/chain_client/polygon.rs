//! Polygon (PoS) chain client implementation.
//!
//! This module provides a client for interacting with the Polygon PoS network,
//! implementing the `BlockChainClient` trait for ERC-20 token transfers
//! (primarily USDC).
mod consts;
mod pimlico_client;

use std::str::FromStr;
use std::sync::Arc;

use alloy::eips::BlockNumberOrTag;
use alloy::eips::eip7702::Authorization;
use alloy::primitives::{
    Address,
    B256,
    TxHash,
    U256,
    keccak256,
};
use alloy::providers::fillers::FillProvider;
use alloy::providers::utils::JoinedRecommendedFillers;
use alloy::providers::{
    Provider,
    ProviderBuilder,
    RootProvider,
    WsConnect,
};
use alloy::rpc::types::{
    Filter,
    Log,
};
use alloy::signers::Signature;
use alloy::sol;
use alloy::sol_types::{
    SolCall,
    SolEvent,
    eip712_domain,
};
use chrono::Utc;
use futures::StreamExt;
use rust_decimal::prelude::{
    Decimal,
    ToPrimitive,
};
use tokio::sync::RwLock;
use tracing::instrument;

use crate::chain_client::rotator::RpcEndpointRotator;
use crate::types::ChainType;
use crate::utils::logging::category::CHAIN_CLIENT;

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
    SignPermitRequestData,
    SignedTransaction,
    SignedTransactionUtils,
    SubscriptionError,
    TransactionError,
    TransfersStream,
    UnsignedTransaction,
};

use super::keyring::SignTransactionRequestData;

pub(super) use consts::{
    ACCOUNT_IMPL,
    CHAIN_ID,
    ENTRYPOINT,
    PAYMASTER,
    USDC,
};
pub(super) use pimlico_client::UserOperationParams;
use pimlico_client::{
    GasParams,
    GasPrice,
    PimlicoClient,
    TokenQuote,
};

// ============================================================================
// ERC-20 Interface Definition
// ============================================================================

sol! {
    /// Standard ERC-20 interface for token interactions
    #[sol(rpc)]
    interface IERC20 {
        function name() external view returns (string memory);
        function symbol() external view returns (string memory);
        function decimals() external view returns (uint8);
        function balanceOf(address account) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
        function execute(address dest, uint256 value, bytes calldata func) external;
        function getNonce(address sender, uint192 key) external view returns (uint256);
        function nonces(address owner) external view returns (uint256);

        event Transfer(address indexed from, address indexed to, uint256 value);
    }
}

// ============================================================================
// Type Definitions
// ============================================================================

/// Polygon account ID (Ethereum address)
pub type PolygonAccountId = Address;

/// Polygon asset ID (ERC-20 contract address)
pub type PolygonAssetId = Address;

/// Polygon transaction hash
pub type PolygonTransactionHash = TxHash;

/// Polygon block hash
pub type PolygonBlockHash = alloy::primitives::B256;

#[derive(Debug, Clone)]
pub struct PolygonUnsignedTransaction {
    pub sender: PolygonAccountId,
    pub recipient: PolygonAccountId,
    pub entrypoint_nonce: U256,
    pub call_data: Vec<u8>,
    pub gas_price: GasPrice,
    pub gas_params: GasParams,
    pub permit_hash: B256,
    pub asset_id: Address,
    pub amount_wei: U256,
    pub authorization: Authorization,
    pub transfer_all: bool,
    pub op_hash: Option<B256>,
    pub paymaster_data: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SignedPermit {
    pub signature: Signature,
}

/// Signed transaction for Polygon
#[derive(Debug, Clone)]
pub struct PolygonSignedTransaction {
    /// User operation params with required signatures and permit, ready to send
    /// to the bundler
    pub op_params: UserOperationParams,
    /// Hash of the user operation params
    pub op_hash: B256,
    /// Unsigned transaction data required to build `ChainTransfer`
    pub unsigned_transaction: PolygonUnsignedTransaction,
}

impl SignedTransactionUtils for PolygonSignedTransaction {
    fn to_raw_string(&self) -> String {
        serde_json::to_string(&self.op_params).unwrap()
    }

    fn hash(&self) -> String {
        format!("{:?}", self.op_hash)
    }
}

// ============================================================================
// Chain Configuration
// ============================================================================

/// Polygon chain configuration type marker
#[derive(Debug, Clone)]
pub enum PolygonChainConfig {}

impl ChainConfig for PolygonChainConfig {
    type AccountId = PolygonAccountId;
    type AssetId = PolygonAssetId;
    type BlockHash = PolygonBlockHash;
    type SignedTransaction = PolygonSignedTransaction;
    // transaction hash
    type TransactionHash = PolygonTransactionHash;
    // TODO: it's better to make a wrapper around a string for the specific chain
    // TODO: here we got quite specific situation: as long as we use Circle
    // Paymaster and Pimlico bundler for outgoing transactions, we've got
    // different IDs for incoming and outgoing transactions: transaction hash
    // and user operation hash respectively. It's better to refactor it in one
    // or another way to make it more obvious. For example can try to make it
    // enum but need to think how to store it in database properly.
    type TransactionId = String;
    type UnsignedTransaction = PolygonUnsignedTransaction;

    const CHAIN_TYPE: ChainType = ChainType::Polygon;
}

impl From<String> for GeneralTransactionId {
    fn from(value: String) -> Self {
        GeneralTransactionId {
            block_number: None,
            position_in_block: None,
            tx_hash: Some(value),
        }
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Convert a U256 value to Decimal with the given number of decimals
fn u256_to_decimal(
    value: U256,
    decimals: u8,
) -> Decimal {
    // Convert U256 to string and parse as Decimal
    let value_str = value.to_string();
    let raw_decimal = Decimal::from_str(&value_str).unwrap_or(Decimal::ZERO);

    // Apply decimal places
    let scale = Decimal::new(1, u32::from(decimals));
    raw_decimal * scale
}

/// Convert a Decimal to U256 with the given number of decimals
fn decimal_to_u256(
    value: Decimal,
    decimals: u8,
) -> U256 {
    // Scale up by decimals
    let multiplier = Decimal::new(10_i64.pow(u32::from(decimals)), 0);
    #[expect(clippy::arithmetic_side_effects)]
    let scaled = value * multiplier;

    // Convert to U256
    scaled
        .to_u128()
        .map(U256::from)
        .unwrap_or(U256::ZERO)
}

pub(super) fn pack_u128_to_bytes(
    first: u128,
    second: u128,
) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[0..16].copy_from_slice(&first.to_be_bytes());
    bytes[16..32].copy_from_slice(&second.to_be_bytes());
    bytes.into()
}

// ============================================================================
// Polygon Client
// ============================================================================

type PolygonProvider = FillProvider<JoinedRecommendedFillers, RootProvider>;

/// Client for interacting with Polygon PoS network
#[derive(Clone)]
pub struct PolygonClient {
    config: crate::configs::ChainConfig,
    asset_info_store: AssetInfoStore<PolygonChainConfig>,
    provider: PolygonProvider,
    pimlico_client: PimlicoClient,
    endpoint_rotator: Arc<RwLock<RpcEndpointRotator>>,
}

impl PolygonClient {
    /// Create a new Polygon client from configuration
    #[instrument(skip(config, asset_info_store))]
    async fn from_config(
        config: &crate::configs::ChainConfig,
        asset_info_store: AssetInfoStore<PolygonChainConfig>,
        endpoint_rotator: Arc<RwLock<RpcEndpointRotator>>,
    ) -> Result<Self, ClientError> {
        let endpoint = {
            let lock = endpoint_rotator.read().await;
            lock.get_endpoint_url()
        };

        tracing::debug!(
            url = endpoint,
            chain = %Self::chain_type(),
            "Trying to connect to endpoint...",
        );

        // Test connection and get chain ID
        let ws_connect = WsConnect::new(&endpoint);
        let provider = ProviderBuilder::new()
            .connect_ws(ws_connect)
            .await;

        match provider {
            Ok(provider) => {
                    tracing::debug!(
                    url = endpoint,
                    chain = %Self::chain_type(),
                    "Connection successful"
                );

                // Get chain ID for transaction signing
                let chain_id = provider
                    .get_chain_id()
                    .await
                    .inspect_err(|e| {
                        tracing::debug!(
                            error.category = CHAIN_CLIENT,
                            error.source = ?e,
                            "Failed to get chain ID"
                        );
                    })
                    .map_err(|_| ClientError::MetadataFetchFailed)?;

                tracing::info!(
                    chain_id = chain_id,
                    endpoint = %endpoint,
                    "Connected to Polygon network"
                );

                Ok(Self {
                    config: config.clone(),
                    asset_info_store,
                    provider,
                    pimlico_client: PimlicoClient::new(),
                    endpoint_rotator,
                })
            },
            Err(e) => {
                let mut lock = endpoint_rotator.write().await;
                lock.mark_unhealthy(&endpoint);

                tracing::debug!(
                    error.category = CHAIN_CLIENT,
                    error.operation = "connect_client",
                    error.source = ?e,
                    endpoint = %endpoint,
                    chain = %Self::chain_type(),
                    "Failed to connect to Polygon RPC endpoint"
                );

                Err(ClientError::EndpointUnavailable { endpoint_url: endpoint })
            }
        }
    }

    /// Convert a log entry to a ChainTransfer
    async fn log_to_transfer(
        &self,
        log: &Log,
        event: &IERC20::Transfer,
    ) -> Result<ChainTransfer<PolygonChainConfig>, SubscriptionError> {
        let asset_id = log.address();

        let asset_info = self
            .asset_info_store
            .get_asset_info(&asset_id)
            .await
            .ok_or_else(|| {
                tracing::warn!(
                    asset_id = %asset_id,
                    "Received transfer event for unknown asset"
                );
                SubscriptionError::AssetNotFound {
                    // TODO: change asset_id to String in the error and use real asset ID
                    asset_id: 0, // We don't have u32 for Polygon, using 0 as placeholder
                }
            })?;

        let tx_hash = log.transaction_hash.ok_or(
            SubscriptionError::BlockProcessingFailed {
                block_number: 0,
            },
        )?;

        // TODO: it's better to also have block number/index but need to refactor
        // `TransactionId` first let block_number = log
        //     .block_number
        //     .ok_or(SubscriptionError::BlockProcessingFailed { block_number: 0 })?;

        // let tx_index = log.transaction_index.ok_or(
        //     SubscriptionError::BlockProcessingFailed {
        //         #[expect(clippy::cast_possible_truncation)]
        //         block_number: block_number as u32,
        //     },
        // )?;

        // Use current time for timestamp (we could fetch block, but it's expensive)
        #[expect(clippy::cast_sign_loss)]
        let timestamp = chrono::Utc::now().timestamp_millis() as u64;

        let amount = u256_to_decimal(event.value, asset_info.decimals);

        Ok(ChainTransfer {
            asset_id,
            asset_name: asset_info.name.clone(),
            amount,
            sender: event.from,
            recipient: event.to,
            transaction_id: const_hex::encode_prefixed(tx_hash),
            timestamp,
        })
    }

    fn build_permit_hash(
        &self,
        sender: &Address,
        nonce: U256,
    ) -> B256 {
        let domain = eip712_domain! {
            name: "USD Coin",
            version: "2",
            chain_id: CHAIN_ID,
            verifying_contract: USDC,
        };

        let permit_typehash = keccak256(
            b"Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)",
        );

        let struct_hash = keccak256(
            [
                permit_typehash.as_slice(),
                &[0u8; 12],
                sender.as_slice(),
                &[0u8; 12],
                PAYMASTER.as_slice(),
                // allow to spend max, it's ok for our purposes
                &U256::MAX.to_be_bytes::<32>(),
                &nonce.to_be_bytes::<32>(),
                &U256::MAX.to_be_bytes::<32>(),
            ]
            .concat(),
        );

        keccak256(
            [
                &[0x19, 0x01],
                domain.hash_struct().as_slice(),
                struct_hash.as_slice(),
            ]
            .concat(),
        )
    }

    fn calculate_max_cost_in_token(
        &self,
        gas_params: &GasParams,
        gas_price: &GasPrice,
        quote: &TokenQuote,
    ) -> U256 {
        // Calculate max gas
        let user_op_max_gas = gas_params.pre_verification_gas
            + gas_params.call_gas_limit
            + gas_params.verification_gas_limit
            + gas_params.paymaster_post_op_gas_limit
            + gas_params.paymaster_verification_gas_limit;

        let user_op_max_cost = user_op_max_gas * gas_price.max_fee_per_gas;
        let post_op_cost = quote.post_op_gas * gas_price.max_fee_per_gas;
        let total_cost_wei = user_op_max_cost + post_op_cost;

        (total_cost_wei * quote.exchange_rate) / U256::from(10).pow(U256::from(18))
    }

    fn build_call(
        &self,
        recipient: Address,
        amount_wei: U256,
        token: Address,
    ) -> Vec<u8> {
        let inner_call = IERC20::transferCall {
            to: recipient,
            amount: amount_wei,
        };

        IERC20::executeCall {
            dest: token,
            value: U256::ZERO,
            func: inner_call.abi_encode().into(),
        }
        .abi_encode()
    }

    fn build_paymaster_data(
        &self,
        asset_id: Address,
        permit_signature: &[u8],
    ) -> Vec<u8> {
        [
            &[0u8],
            asset_id.as_slice(),
            &U256::MAX.to_be_bytes::<32>(),
            permit_signature,
        ]
        .concat()
    }

    fn compute_user_op_hash(
        &self,
        transaction: &PolygonUnsignedTransaction,
        paymaster_data: &[u8],
    ) -> B256 {
        let type_hash = keccak256(b"PackedUserOperation(address sender,uint256 nonce,bytes initCode,bytes callData,bytes32 accountGasLimits,uint256 preVerificationGas,bytes32 gasFees,bytes paymasterAndData)");

        let account_gas_limits = pack_u128_to_bytes(
            transaction
                .gas_params
                .verification_gas_limit
                .to(),
            transaction
                .gas_params
                .call_gas_limit
                .to(),
        );

        let gas_fees = pack_u128_to_bytes(
            transaction
                .gas_price
                .max_priority_fee_per_gas
                .to(),
            transaction
                .gas_price
                .max_fee_per_gas
                .to(),
        );

        let paymaster_gas_limits = pack_u128_to_bytes(
            transaction
                .gas_params
                .paymaster_verification_gas_limit
                .to(),
            transaction
                .gas_params
                .paymaster_post_op_gas_limit
                .to(),
        );

        let paymaster_and_data = [
            PAYMASTER.as_slice(),
            paymaster_gas_limits.as_slice(),
            paymaster_data,
        ]
        .concat();

        let struct_hash = keccak256(
            [
                type_hash.as_slice(),
                &[0u8; 12],
                transaction.sender.as_slice(),
                &transaction
                    .entrypoint_nonce
                    .to_be_bytes::<32>(),
                keccak256([]).as_slice(), // init code, empty
                keccak256(&transaction.call_data).as_slice(),
                account_gas_limits.as_slice(),
                &transaction
                    .gas_params
                    .pre_verification_gas
                    .to_be_bytes::<32>(),
                gas_fees.as_slice(),
                keccak256(paymaster_and_data).as_slice(),
            ]
            .concat(),
        );

        let domain = eip712_domain! {
            name: "ERC4337",
            version: "1",
            chain_id: CHAIN_ID,
            verifying_contract: ENTRYPOINT,
        };

        keccak256(
            [
                &[0x19, 0x01],
                domain.hash_struct().as_slice(),
                struct_hash.as_slice(),
            ]
            .concat(),
        )
    }
}

impl BlockChainClient<PolygonChainConfig> for PolygonClient {
    fn chain_name(&self) -> &'static str {
        "polygon"
    }

    fn asset_info_store(&self) -> &AssetInfoStore<PolygonChainConfig> {
        &self.asset_info_store
    }

    #[instrument(skip(config))]
    async fn new(
        config: &crate::configs::ChainConfig,
        rotator: Arc<RwLock<RpcEndpointRotator>>,
    ) -> Result<Self, ClientError> {
        Self::from_config(config, AssetInfoStore::new(), rotator).await
    }

    #[instrument(skip(config, asset_info_store))]
    async fn new_with_store(
        config: &crate::configs::ChainConfig,
        asset_info_store: AssetInfoStore<PolygonChainConfig>,
        rotator: Arc<RwLock<RpcEndpointRotator>>,
    ) -> Result<Self, ClientError> {
        Self::from_config(config, asset_info_store, rotator).await
    }

    #[instrument(skip(self))]
    async fn recreate(&self) -> Result<Self, ClientError> {
        // For now, just return a clone
        // TODO: Implement proper reconnection logic
        Self::from_config(
            &self.config,
            self.asset_info_store.clone(),
            self.endpoint_rotator.clone(),
        )
        .await
    }

    #[instrument(skip(self))]
    async fn fetch_asset_info(
        &self,
        asset_id: &PolygonAssetId,
    ) -> Result<AssetInfo<PolygonChainConfig>, QueryError> {
        tracing::trace!("Fetching ERC-20 token info...");
        let contract = IERC20::new(*asset_id, self.provider.clone());

        // Fetch symbol
        let symbol = contract
            .symbol()
            .call()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = CHAIN_CLIENT,
                    error.operation = "fetch_asset_info",
                    error.source = ?e,
                    asset_id = %asset_id,
                    "Failed to fetch token symbol"
                );
            })
            .map_err(|_| QueryError::RpcRequestFailed)?;

        // Fetch decimals
        let decimals = contract
            .decimals()
            .call()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = CHAIN_CLIENT,
                    error.operation = "fetch_asset_info",
                    error.source = ?e,
                    asset_id = %asset_id,
                    "Failed to fetch token decimals"
                );
            })
            .map_err(|_| QueryError::RpcRequestFailed)?;

        let info = AssetInfo {
            id: *asset_id,
            name: symbol,
            decimals,
        };

        tracing::trace!(asset_info = ?info, "Asset info fetched successfully");

        Ok(info)
    }

    #[instrument(skip(self))]
    async fn fetch_asset_balance(
        &self,
        asset_id: &PolygonAssetId,
        account: &PolygonAccountId,
    ) -> Result<Decimal, QueryError> {
        tracing::trace!("Fetching ERC-20 balance...");

        let decimals = self
            .asset_info_store
            .get_asset_info(asset_id)
            .await
            .ok_or_else(|| {
                tracing::warn!("Asset info not found in local store");
                QueryError::NotFound {
                    query_type: format!("asset info for {asset_id}"),
                }
            })?
            .decimals;

        let contract = IERC20::new(*asset_id, self.provider.clone());

        let balance_result = contract
            .balanceOf(*account)
            .call()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = CHAIN_CLIENT,
                    error.operation = "fetch_balance",
                    error.source = ?e,
                    asset_id = %asset_id,
                    account = %account,
                    "Failed to fetch token balance"
                );
            })
            .map_err(|_| QueryError::RpcRequestFailed)?;

        // alloy 1.4 returns the value directly
        let balance = balance_result;

        Ok(u256_to_decimal(balance, decimals))
    }

    #[instrument(skip(self))]
    async fn subscribe_transfers(
        &self,
        asset_ids: &[PolygonAssetId],
    ) -> Result<TransfersStream<PolygonChainConfig>, SubscriptionError> {
        // Verify all assets are in the store
        let assets = self
            .asset_info_store
            .get_assets_info(asset_ids)
            .await;

        for asset_id in asset_ids {
            if !assets.contains_key(asset_id) {
                return Err(SubscriptionError::AssetNotFound {
                    asset_id: 0, // Placeholder since Polygon uses Address not u32
                });
            }
        }

        // Build filter for Transfer events from all tracked ERC-20 contracts
        let filter = Filter::new()
            .address(asset_ids.to_vec())
            .event_signature(IERC20::Transfer::SIGNATURE_HASH)
            .from_block(BlockNumberOrTag::Latest);

        let client = self.clone();

        // Subscribe to logs
        let subscription = client
            .provider
            .subscribe_logs(&filter)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = CHAIN_CLIENT,
                    error.operation = "subscribe_transfers",
                    error.source = ?e,
                    "Failed to subscribe to Transfer events"
                );
            })
            .map_err(|_| SubscriptionError::SubscriptionFailed)?;

        tracing::info!(
            asset_count = asset_ids.len(),
            "Subscribed to ERC-20 Transfer events"
        );

        let stream = async_stream::try_stream! {
            let mut sub = subscription.into_stream();

            while let Some(log) = sub.next().await {
                // Decode Transfer event from log
                match log.log_decode::<IERC20::Transfer>() {
                    Ok(decoded) => {
                        let event = decoded.inner.data;
                        match client.log_to_transfer(&log, &event).await {
                            Ok(transfer) => {
                                // tracing::trace!(
                                //     from = %transfer.sender,
                                //     to = %transfer.recipient,
                                //     amount = %transfer.amount,
                                //     asset = %transfer.asset_name,
                                //     "Detected ERC-20 transfer"
                                // );
                                yield vec![transfer];
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = ?e,
                                    "Failed to process transfer event"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!(
                            error = ?e,
                            "Failed to decode Transfer event from log"
                        );
                    }
                }
            }

            tracing::info!("Transfer event subscription stream ended");
        };

        Ok(Box::pin(stream))
    }

    #[instrument(skip(self))]
    async fn init_asset_info(
        &self,
        asset_ids: &[String],
    ) -> Result<(), ClientError> {
        BlockChainClientExt::init_asset_info_impl(self, asset_ids).await
    }

    #[instrument(skip(self), fields(asset_id = %asset_id, amount = %amount))]
    async fn build_transfer(
        &self,
        sender: &PolygonAccountId,
        recipient: &PolygonAccountId,
        asset_id: &PolygonAssetId,
        amount: Decimal,
    ) -> Result<UnsignedTransaction<PolygonChainConfig>, TransactionError<PolygonChainConfig>> {
        let decimals = self
            .asset_info_store
            .get_asset_info(asset_id)
            .await
            .ok_or_else(|| TransactionError::BuildFailed {
                reason: format!("Asset {asset_id} not found in asset info store"),
            })?
            .decimals;

        let amount_wei = decimal_to_u256(amount, decimals);

        let contract = IERC20::new(*asset_id, self.provider.clone());
        let entrypoint_contract = IERC20::new(ENTRYPOINT, self.provider.clone());

        let sender_nonce = self
            .provider
            .get_transaction_count(*sender)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error = ?e,
                    "Failed to get sender nonce"
                );
                TransactionError::BuildFailed {
                    reason: "Failed to get sender nonce".to_string(),
                }
            })?;

        let permit_nonce = contract
            .nonces(*sender)
            .call()
            .await
            .map_err(|e| {
                tracing::debug!(
                    error = ?e,
                    "Failed to get contract nonce for permit"
                );
                TransactionError::BuildFailed {
                    reason: "Failed to get contract nonce for permit".to_string(),
                }
            })?;

        let entrypoint_nonce = entrypoint_contract
            .getNonce(
                *sender,
                alloy::primitives::Uint::<192, 3>::ZERO,
            )
            .call()
            .await
            .map_err(|e| {
                tracing::debug!(
                    error = ?e,
                    "Failed to get entrypoint nonce"
                );
                TransactionError::BuildFailed {
                    reason: "Failed to get entrypoint nonce for permit".to_string(),
                }
            })?;

        let gas_price = self
            .pimlico_client
            .get_gas_prices()
            .await
            .map_err(|e| {
                tracing::debug!(
                    error = ?e,
                    "Failed to get gas prices using pimlico client"
                );
                TransactionError::BuildFailed {
                    reason: "Failed to get gas prices using pimlico client".to_string(),
                }
            })?
            // TODO: use standard for now, later it's better to be able to configure it
            .standard;

        // use dummy gas params for now, for calculation of real params we need to have
        // a real signed permit which we can get only on signing step
        let gas_params = GasParams::dummy();
        let permit_hash = self.build_permit_hash(sender, permit_nonce);
        let call_data = self.build_call(*recipient, amount_wei, *asset_id);

        let authorization = Authorization {
            chain_id: U256::from(CHAIN_ID),
            address: ACCOUNT_IMPL,
            nonce: sender_nonce,
        };

        let transaction = PolygonUnsignedTransaction {
            transfer_all: false,
            sender: *sender,
            recipient: *recipient,
            asset_id: *asset_id,
            entrypoint_nonce,
            call_data,
            gas_price,
            gas_params,
            permit_hash,
            amount_wei,
            authorization,
            paymaster_data: None,
            op_hash: None,
        };

        Ok(UnsignedTransaction {
            transaction,
        })
    }

    #[instrument(skip(self), fields(asset_id = %asset_id))]
    async fn build_transfer_all(
        &self,
        sender: &PolygonAccountId,
        recipient: &PolygonAccountId,
        asset_id: &PolygonAssetId,
    ) -> Result<UnsignedTransaction<PolygonChainConfig>, TransactionError<PolygonChainConfig>> {
        // Fetch current balance
        let balance = self
            .fetch_asset_balance(asset_id, sender)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.source = ?e,
                    "Failed to fetch balance for transfer_all"
                );
                TransactionError::BuildFailed {
                    reason: "Failed to fetch balance".to_string(),
                }
            })?;

        if balance.is_zero() {
            return Err(TransactionError::BuildFailed {
                reason: "Zero balance, nothing to transfer".to_string(),
            });
        }

        // Initially set transaction amount as full balance. On signing step we'll fetch
        // accurate gas estimates and substruct their total amount from balance
        // value and rebuild transaction call
        let base_tx = self
            .build_transfer(sender, recipient, asset_id, balance)
            .await?;

        // Create the new transaction with the adjusted amount but same gas params
        let transaction = PolygonUnsignedTransaction {
            transfer_all: true,
            ..base_tx.transaction
        };

        Ok(UnsignedTransaction {
            transaction,
        })
    }

    #[instrument(skip(self, transaction, keyring_client))]
    async fn sign_transaction(
        &self,
        transaction: UnsignedTransaction<PolygonChainConfig>,
        derivation_params: Vec<String>,
        keyring_client: &KeyringClient,
    ) -> Result<SignedTransaction<PolygonChainConfig>, TransactionError<PolygonChainConfig>> {
        let mut inner = transaction.transaction;

        let sign_permit_data = SignPermitRequestData {
            permit_hash: inner.permit_hash,
            derivation_params: derivation_params.clone(),
        };

        let signed_permit = keyring_client
            .sign_polygon_permit(sign_permit_data)
            .await?
            .signature
            .as_bytes();

        let paymaster_data = self.build_paymaster_data(inner.asset_id, &signed_permit);
        inner.op_hash = Some(self.compute_user_op_hash(&inner, &paymaster_data));
        let encoded_paymaster_data = const_hex::encode_prefixed(paymaster_data.clone());
        inner.paymaster_data = Some(encoded_paymaster_data.clone());

        let call_data_for_estimate = if inner.transfer_all {
            // for transfer all if we'll put full amount we'll get an error that we don't
            // have enough balance for transfer + fees, so we put some dummy amount for now,
            // paymaster fee shouldn't be significantly different depending on amount
            self.build_call(
                inner.recipient,
                U256::from(100),
                inner.asset_id,
            )
        } else {
            inner.call_data.clone()
        };

        let mut gas_params = self
            .pimlico_client
            .get_estimate_gas(
                inner.sender,
                inner.entrypoint_nonce,
                call_data_for_estimate,
                encoded_paymaster_data,
                inner.gas_price,
            )
            .await
            .map_err(|e| {
                tracing::info!(
                    error = ?e,
                    "Failed to get estimated gas for transaction using pimlico client"
                );
                TransactionError::BuildFailed {
                    reason: "Failed to get estimated gas for transaction using pimlico client"
                        .to_string(),
                }
            })?;

        // Recommended minimal `paymaster_post_op_gas_limit` for Circle's paymaster.
        // Shown in their example but not documented anywhere. Anyway if returned limit
        // is lower then 15k, transaction fails, bundler return AA23 error.
        let recommended_minimal = U256::from(15_000);

        if gas_params.paymaster_post_op_gas_limit < recommended_minimal {
            gas_params.paymaster_post_op_gas_limit = recommended_minimal;
        }

        inner.gas_params = gas_params;

        if inner.transfer_all {
            let quotes = self
                .pimlico_client
                .get_token_quotes(&[inner.asset_id])
                .await
                .map_err(|e| {
                    tracing::debug!(
                        error = ?e,
                        "Failed to get USDC quote using pimlico client",
                    );

                    TransactionError::BuildFailed {
                        reason: "Failed to get quote using pimlico client".to_string(),
                    }
                })?;

            let usdc_quote =
                quotes
                    .quotes
                    .first()
                    .ok_or_else(|| TransactionError::BuildFailed {
                        reason: "Failed to get quote from paymaster".to_string(),
                    })?;

            let max_cost_in_usdc_wei = self.calculate_max_cost_in_token(
                &inner.gas_params,
                &inner.gas_price,
                usdc_quote,
            );

            let amount_wei = inner
                .amount_wei
                .saturating_sub(max_cost_in_usdc_wei)
                .saturating_sub(U256::from(100));

            let call_data = self.build_call(
                inner.recipient,
                amount_wei,
                inner.asset_id,
            );
            inner.call_data = call_data;
            let op_hash = self.compute_user_op_hash(&inner, &paymaster_data);

            if amount_wei.is_zero() {
                return Err(TransactionError::InsufficientBalance {
                    transaction_id: op_hash.to_string(),
                })
            }

            // have to recalculate op_hash
            inner.op_hash = Some(op_hash);
        }

        let data = SignTransactionRequestData {
            transaction: inner,
            derivation_params,
        };

        let signed = keyring_client
            .sign_polygon_transaction(data)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.source = ?e,
                    "Failed to sign Polygon transaction"
                );
                TransactionError::BuildFailed {
                    reason: format!("Signing failed: {e}"),
                }
            })?;

        Ok(SignedTransaction {
            transaction: signed,
        })
    }

    #[instrument(skip(self, transaction), fields(tx_hash = transaction.transaction.hash()))]
    async fn submit_and_watch_transaction(
        &self,
        transaction: SignedTransaction<PolygonChainConfig>,
    ) -> Result<ChainTransfer<PolygonChainConfig>, TransactionError<PolygonChainConfig>> {
        let PolygonSignedTransaction {
            op_params,
            op_hash,
            unsigned_transaction: unsigned,
        } = transaction.transaction;

        let asset_id = unsigned.asset_id;

        let asset_info = self
            .asset_info_store
            .get_asset_info(&unsigned.asset_id)
            .await
            .ok_or_else(|| TransactionError::BuildFailed {
                reason: format!("Asset {asset_id} not found in asset info store"),
            })?;

        let op_hash = self
            .pimlico_client
            .send_user_operation(op_params)
            .await
            .map_err(|e| {
                tracing::warn!(
                    error = ?e,
                    "Failed to send user operation using pimlico client"
                );
                TransactionError::ExecutionFailed {
                    transaction_id: const_hex::encode_prefixed(op_hash),
                    error_code: e.to_string(),
                }
            })?;

        // monitor up to 30 seconds, refetch operation with 1 second delay
        for _ in 0..30 {
            let receipt = self
                .pimlico_client
                .get_operation_receipt(&op_hash)
                .await;

            // unfortunately some of required data is not presented in receipt, so we have
            // to fill it from saved transaction parameters
            match receipt {
                Ok(Some(data)) if data.success => {
                    return Ok(ChainTransfer {
                        asset_id,
                        asset_name: asset_info.name,
                        amount: u256_to_decimal(unsigned.amount_wei, asset_info.decimals),
                        sender: data.sender,
                        recipient: data.receipt.to,
                        transaction_id: data.receipt.transaction_hash,
                        timestamp: Utc::now().timestamp_millis() as u64,
                    })
                },
                Ok(Some(data)) => {
                    tracing::warn!(response = ?data, "Got unsuccessful operation result from pimlico");
                    return Err(TransactionError::ExecutionFailed {
                        transaction_id: op_hash,
                        error_code: "".to_string(),
                    })
                },
                Ok(None) => tracing::trace!("No receipt returned yet, continue watching"),
                Err(e) => tracing::debug!(
                    error = ?e,
                    "Error while fetching receipt data from using pimlico client"
                ),
            };

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        Err(
            TransactionError::TransactionInfoFetchFailed {
                transaction_id: op_hash,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u256_decimal_conversion() {
        // 1 USDC = 1_000_000 (6 decimals)
        let value = U256::from(1_000_000_u64);
        let decimal = u256_to_decimal(value, 6);
        assert_eq!(decimal, Decimal::new(1, 0)); // 1.0

        // Convert back
        let back = decimal_to_u256(decimal, 6);
        assert_eq!(back, value);
    }
}
