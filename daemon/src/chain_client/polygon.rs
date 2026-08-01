//! Polygon (PoS) chain client implementation.
//!
//! This module provides a client for interacting with the Polygon PoS network,
//! implementing the `BlockChainClient` trait for ERC-20 token transfers
//! (primarily USDC).
mod consts;
mod pimlico_client;

use std::time::Duration;

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
use tracing::instrument;

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

const WS_MESSAGES_TIMEOUT_DURATION: Duration = Duration::from_secs(10);

/// A decoded Transfer event waiting to become `confirmations` blocks deep.
struct PendingTransfer {
    transaction_hash: TxHash,
    log_index: u64,
    transfer: ChainTransfer<PolygonChainConfig>,
}

/// Buffers Transfer events until they are buried under enough blocks, dropping
/// entries whose logs are reorged away (`removed: true`) in the meantime.
#[derive(Default)]
struct ConfirmationBuffer {
    pending: std::collections::BTreeMap<u64, Vec<PendingTransfer>>,
}

impl ConfirmationBuffer {
    fn insert(
        &mut self,
        block_number: u64,
        transaction_hash: TxHash,
        log_index: u64,
        transfer: ChainTransfer<PolygonChainConfig>,
    ) {
        self.pending
            .entry(block_number)
            .or_default()
            .push(PendingTransfer {
                transaction_hash,
                log_index,
                transfer,
            });
    }

    /// Drops a reorged-away log from the buffer. Returns `false` if no matching
    /// entry was pending — either it was never seen or it has already been
    /// released past the confirmation depth.
    fn remove(
        &mut self,
        transaction_hash: TxHash,
        log_index: u64,
    ) -> bool {
        let mut removed = false;

        self.pending.retain(|_, transfers| {
            let len_before = transfers.len();
            transfers.retain(|pending| {
                !(pending.transaction_hash == transaction_hash && pending.log_index == log_index)
            });
            removed |= transfers.len() != len_before;
            !transfers.is_empty()
        });

        removed
    }

    /// Releases every transfer that is at least `confirmations` blocks below
    /// `latest_block`, in block order.
    fn take_confirmed(
        &mut self,
        latest_block: u64,
        confirmations: u64,
    ) -> Vec<ChainTransfer<PolygonChainConfig>> {
        let cutoff = latest_block.saturating_sub(confirmations);
        let still_pending = self
            .pending
            .split_off(&cutoff.saturating_add(1));
        let confirmed = std::mem::replace(&mut self.pending, still_pending);

        confirmed
            .into_values()
            .flatten()
            .map(|pending| pending.transfer)
            .collect()
    }
}

/// Reconnect budget handed to alloy, rather than inherited from it.
///
/// alloy retries the *same* URL internally. Only our own layer can rotate to
/// another endpoint: `TransfersTracker` reacts to a dead stream by calling
/// `recreate()`, which re-picks from the configured endpoints. So every second
/// alloy spends retrying is a second we cannot spend moving away from a bad
/// node, which is the failure in #333 -- three months on a public endpoint that
/// dropped every 60s.
///
/// The upstream defaults are 10 retries at 3s. Under alloy 1.x that is a flat
/// 27s; alloy 2.x kept the same two numbers but reads them as the base of a
/// capped exponential backoff (cap 30s), making the same defaults 195s. Same
/// call, same configuration, ~7x the outage -- which is exactly why these are
/// now stated here instead of inherited. They will drift again.
///
/// 3 retries from a 2s base is 6s under 2.x semantics and 4s under 1.x, both
/// comfortably inside `WS_MESSAGES_TIMEOUT_DURATION`, so alloy gives up while
/// our own supervision is still waiting rather than after it has given up.
/// `ws_reconnect_budget_stays_within_our_own_timeout` pins that relationship.
const WS_RECONNECT_MAX_RETRIES: u32 = 3;
const WS_RECONNECT_RETRY_INTERVAL: Duration = Duration::from_secs(2);

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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[cfg(test)]
pub fn default_polygon_unsigned_transaction() -> PolygonUnsignedTransaction {
    use alloy::primitives::address;

    PolygonUnsignedTransaction {
        sender: address!("0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7"),
        recipient: address!("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359"),
        entrypoint_nonce: U256::from(100),
        call_data: vec![],
        gas_price: GasPrice {
            max_fee_per_gas: U256::from(100),
            max_priority_fee_per_gas: U256::from(50),
        },
        gas_params: GasParams::dummy(),
        permit_hash: B256::ZERO,
        asset_id: address!("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359"),
        amount_wei: U256::from(1_000_000),
        authorization: Authorization {
            chain_id: U256::from(137),
            address: Address::ZERO,
            nonce: 50,
        },
        transfer_all: false,
        op_hash: None,
        paymaster_data: None,
    }
}

#[derive(Debug, Clone)]
pub struct SignedPermit {
    pub signature: Signature,
}

/// Signed transaction for Polygon
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolygonSignedTransaction {
    /// User operation params with required signatures and permit, ready to send
    /// to the bundler
    pub op_params: UserOperationParams,
    /// Hash of the user operation params
    pub op_hash: B256,
    /// Unsigned transaction data required to build `ChainTransfer`
    pub unsigned_transaction: PolygonUnsignedTransaction,
}

#[cfg(test)]
pub fn default_polygon_signed_transaction() -> PolygonSignedTransaction {
    use alloy::primitives::address;
    use pimlico_client::Eip7702Auth;

    PolygonSignedTransaction {
        op_params: UserOperationParams {
            sender: address!("0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7"),
            nonce: U256::from(100),
            call_data: "".to_string(),
            paymaster: Address::ZERO,
            paymaster_data: "".to_string(),
            signature: "".to_string(),
            gas_params: GasParams::dummy(),
            gas_price: GasPrice {
                max_fee_per_gas: U256::from(100),
                max_priority_fee_per_gas: U256::from(50),
            },
            eip7702_auth: Eip7702Auth {
                chain_id: U256::from(137),
                address: Address::ZERO,
                nonce: U256::from(100),
                y_parity: U256::from(10),
                r: U256::from(20),
                s: U256::from(30),
            },
        },
        op_hash: B256::ZERO,
        unsigned_transaction: default_polygon_unsigned_transaction(),
    }
}

impl SignedTransactionUtils for PolygonSignedTransaction {
    fn to_raw_string(&self) -> String {
        #[expect(
            clippy::unwrap_used,
            reason = "`op_params` is a flat struct of strings and numbers, so serialisation has no failure mode"
        )]
        let raw = serde_json::to_string(&self.op_params).unwrap();

        raw
    }

    fn hash(&self) -> String {
        format!("{:?}", self.op_hash)
    }
}

// ============================================================================
// Chain Configuration
// ============================================================================

/// Polygon chain configuration type marker
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// 10^18, the wei-per-ether scaling factor Pimlico denominates exchange rates
/// in. Built from limbs so it is a `const` and cannot panic at runtime.
const WEI_PER_ETHER: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);

/// Convert a U256 amount in base units to `Decimal` with the given number of
/// decimals.
///
/// Returns `None` when the value cannot be represented exactly by `Decimal`
/// (more than 28 significant digits, or `decimals` above `Decimal`'s maximum
/// scale of 28). This *must* stay fallible: substituting `Decimal::ZERO` on
/// failure — as this function used to do — records a real incoming payment as
/// a zero-amount transfer, i.e. as if the customer had paid nothing.
fn u256_to_decimal(
    value: U256,
    decimals: u8,
) -> Option<Decimal> {
    // Convert U256 to string and parse as Decimal. `from_str_exact` (unlike
    // `from_str`) refuses to silently round away significant digits.
    let raw_decimal = Decimal::from_str_exact(&value.to_string()).ok()?;

    // Apply decimal places. `Decimal::new` panics for scale > 28, so go through
    // the fallible constructor.
    let scale = Decimal::try_new(1, u32::from(decimals)).ok()?;
    raw_decimal.checked_mul(scale)
}

/// Convert a `Decimal` amount to U256 base units with the given number of
/// decimals.
///
/// Returns `None` when the scaled value does not fit `u128`. Also fallible for
/// the money-safety reason above: returning `U256::ZERO` here would build a
/// transfer that moves nothing while the payout is marked as sent.
fn decimal_to_u256(
    value: Decimal,
    decimals: u8,
) -> Option<U256> {
    // Scale up by decimals. `10_i64.pow` overflows for decimals > 18.
    let multiplier = Decimal::try_new(
        10_i64.checked_pow(u32::from(decimals))?,
        0,
    )
    .ok()?;

    // Convert to U256
    value
        .checked_mul(multiplier)?
        .to_u128()
        .map(U256::from)
}

/// Compute the paymaster's maximum charge, denominated in the fee token.
///
/// Every input here (`gas_params`, `gas_price`, `quote`) comes verbatim from a
/// Pimlico bundler JSON-RPC response, so none of it is trusted. A hostile or
/// buggy bundler can hand back values whose sum or product exceeds `U256::MAX`;
/// unchecked `+`/`*` would then panic (with overflow checks on) or wrap to a
/// tiny number that is silently subtracted from the customer's payout. Both are
/// unacceptable, so overflow is a build failure.
fn calculate_max_cost_in_token<T: ChainConfig>(
    gas_params: &GasParams,
    gas_price: &GasPrice,
    quote: &TokenQuote,
) -> Result<U256, TransactionError<T>> {
    let overflow = || TransactionError::BuildFailed {
        reason: "Paymaster gas quote overflows U256".to_string(),
    };

    // Calculate max gas
    let user_op_max_gas = gas_params
        .pre_verification_gas
        .checked_add(gas_params.call_gas_limit)
        .and_then(|g| g.checked_add(gas_params.verification_gas_limit))
        .and_then(|g| g.checked_add(gas_params.paymaster_post_op_gas_limit))
        .and_then(|g| g.checked_add(gas_params.paymaster_verification_gas_limit))
        .ok_or_else(overflow)?;

    let user_op_max_cost = user_op_max_gas
        .checked_mul(gas_price.max_fee_per_gas)
        .ok_or_else(overflow)?;
    let post_op_cost = quote
        .post_op_gas
        .checked_mul(gas_price.max_fee_per_gas)
        .ok_or_else(overflow)?;
    let total_cost_wei = user_op_max_cost
        .checked_add(post_op_cost)
        .ok_or_else(overflow)?;

    total_cost_wei
        .checked_mul(quote.exchange_rate)
        .ok_or_else(overflow)?
        .checked_div(WEI_PER_ETHER)
        .ok_or_else(overflow)
}

/// Narrow a U256 gas/fee field to the `u128` slot that ERC-4337 packs it into.
///
/// `U256::to::<u128>()` panics above `u128::MAX`, and every value passed here
/// originates from an unvalidated Pimlico bundler response, so a malformed or
/// hostile reply would crash the daemon mid-payout. Report it as a build
/// failure instead.
fn u256_to_u128<T: ChainConfig>(
    value: U256,
    field: &'static str,
) -> Result<u128, TransactionError<T>> {
    u128::try_from(value).map_err(|_| {
        tracing::warn!(
            error.category = CHAIN_CLIENT,
            error.operation = "compute_user_op_hash",
            field,
            value = %value,
            "Bundler returned a gas field that exceeds u128"
        );
        TransactionError::BuildFailed {
            reason: format!("Bundler field `{field}` exceeds u128"),
        }
    })
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
    subscription_provider: PolygonProvider,
    pimlico_client: PimlicoClient,
}

impl PolygonClient {
    /// Create a new Polygon client from configuration
    #[instrument(skip(config, asset_info_store))]
    async fn from_config(
        config: &crate::configs::ChainConfig,
        asset_info_store: AssetInfoStore<PolygonChainConfig>,
    ) -> Result<Self, ClientError> {
        let endpoint = config
            .get_random_requests_endpoint()
            .ok_or(ClientError::InvalidConfiguration {
                field: "endpoints".to_string(),
            })?;

        tracing::debug!(
            url = endpoint,
            chain = %Self::chain_type(),
            "Trying to connect to endpoint...",
        );

        // Test connection and get chain ID
        let ws_connect = WsConnect::new(&endpoint)
            .with_max_retries(WS_RECONNECT_MAX_RETRIES)
            .with_retry_interval(WS_RECONNECT_RETRY_INTERVAL);
        let provider = ProviderBuilder::new()
            .connect_ws(ws_connect)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = CHAIN_CLIENT,
                    error.operation = "connect_client",
                    error.source = ?e,
                    endpoint = %endpoint,
                    chain = %Self::chain_type(),
                    "Failed to connect to Polygon RPC endpoint"
                );
            })
            .map_err(|_| ClientError::AllEndpointsUnreachable)?;

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

        let endpoint = config
            .get_random_subscriptions_endpoint()
            .ok_or(ClientError::InvalidConfiguration {
                field: "endpoints".to_string(),
            })?;

        // Test connection and get chain ID
        let ws_connect = WsConnect::new(&endpoint)
            .with_max_retries(WS_RECONNECT_MAX_RETRIES)
            .with_retry_interval(WS_RECONNECT_RETRY_INTERVAL);
        let subscription_provider = ProviderBuilder::new()
            .connect_ws(ws_connect)
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = CHAIN_CLIENT,
                    error.operation = "connect_client",
                    error.source = ?e,
                    endpoint = %endpoint,
                    chain = %Self::chain_type(),
                    "Failed to connect to Polygon RPC endpoint"
                );
            })
            .map_err(|_| ClientError::AllEndpointsUnreachable)?;

        tracing::info!(
            chain_id = chain_id,
            endpoint = %endpoint,
            "Connected to Polygon network"
        );

        Ok(Self {
            config: config.clone(),
            asset_info_store,
            provider,
            subscription_provider,
            pimlico_client: PimlicoClient::new(),
        })
    }

    /// Convert a log entry to a ChainTransfer
    async fn log_to_transfer(
        asset_info_store: &AssetInfoStore<PolygonChainConfig>,
        log: &Log,
        event: &IERC20::Transfer,
    ) -> Result<ChainTransfer<PolygonChainConfig>, SubscriptionError> {
        let asset_id = log.address();

        let asset_info = asset_info_store
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

        let block_number = log.block_number.ok_or(
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

        let error_block_number = u32::try_from(block_number).unwrap_or(u32::MAX);
        let amount = u256_to_decimal(event.value, asset_info.decimals).ok_or(
            SubscriptionError::BlockProcessingFailed {
                block_number: error_block_number,
            },
        )?;

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

    async fn buffer_transfer_log(
        asset_info_store: &AssetInfoStore<PolygonChainConfig>,
        buffer: &mut ConfirmationBuffer,
        log: &Log,
        event: &IERC20::Transfer,
        block_number: u64,
        transaction_hash: TxHash,
        log_index: u64,
    ) {
        match Self::log_to_transfer(asset_info_store, log, event).await {
            Ok(transfer) => {
                tracing::trace!(
                    from = %transfer.sender,
                    to = %transfer.recipient,
                    amount = %transfer.amount,
                    asset = %transfer.asset_name,
                    block_number,
                    "Detected ERC-20 transfer, waiting for confirmations"
                );
                buffer.insert(
                    block_number,
                    transaction_hash,
                    log_index,
                    transfer,
                );
            },
            Err(error) => {
                tracing::error!(
                    ?error,
                    %transaction_hash,
                    block_number,
                    raw_value = %event.value,
                    "Failed to process transfer event, skipping it"
                );
            },
        }
    }

    // Takes no `self`: the digest depends only on its arguments and the module
    // constants, which is what lets the known-answer tests call it directly.
    fn build_permit_hash(
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

    // See the note on `build_permit_hash`: no `self`, so tests can pin the hash.
    fn compute_user_op_hash(
        transaction: &PolygonUnsignedTransaction,
        paymaster_data: &[u8],
    ) -> Result<B256, TransactionError<PolygonChainConfig>> {
        let type_hash = keccak256(b"PackedUserOperation(address sender,uint256 nonce,bytes initCode,bytes callData,bytes32 accountGasLimits,uint256 preVerificationGas,bytes32 gasFees,bytes paymasterAndData)");

        let account_gas_limits = pack_u128_to_bytes(
            u256_to_u128(
                transaction
                    .gas_params
                    .verification_gas_limit,
                "verification_gas_limit",
            )?,
            u256_to_u128(
                transaction.gas_params.call_gas_limit,
                "call_gas_limit",
            )?,
        );

        let gas_fees = pack_u128_to_bytes(
            u256_to_u128(
                transaction
                    .gas_price
                    .max_priority_fee_per_gas,
                "max_priority_fee_per_gas",
            )?,
            u256_to_u128(
                transaction.gas_price.max_fee_per_gas,
                "max_fee_per_gas",
            )?,
        );

        let paymaster_gas_limits = pack_u128_to_bytes(
            u256_to_u128(
                transaction
                    .gas_params
                    .paymaster_verification_gas_limit,
                "paymaster_verification_gas_limit",
            )?,
            u256_to_u128(
                transaction
                    .gas_params
                    .paymaster_post_op_gas_limit,
                "paymaster_post_op_gas_limit",
            )?,
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

        Ok(keccak256(
            [
                &[0x19, 0x01],
                domain.hash_struct().as_slice(),
                struct_hash.as_slice(),
            ]
            .concat(),
        ))
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
    async fn new(config: &crate::configs::ChainConfig) -> Result<Self, ClientError> {
        Self::from_config(config, AssetInfoStore::new()).await
    }

    #[instrument(skip(config, asset_info_store))]
    async fn new_with_store(
        config: &crate::configs::ChainConfig,
        asset_info_store: AssetInfoStore<PolygonChainConfig>,
    ) -> Result<Self, ClientError> {
        Self::from_config(config, asset_info_store).await
    }

    #[instrument(skip(self))]
    async fn recreate(&self) -> Result<Self, ClientError> {
        // For now, just return a clone
        // TODO: Implement proper reconnection logic
        Self::from_config(
            &self.config,
            self.asset_info_store.clone(),
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
        asset_id: PolygonAssetId,
        account: PolygonAccountId,
    ) -> Result<Decimal, QueryError> {
        tracing::trace!("Fetching ERC-20 balance...");

        let decimals = self
            .asset_info_store
            .get_asset_info(&asset_id)
            .await
            .ok_or_else(|| {
                tracing::warn!("Asset info not found in local store");
                QueryError::NotFound {
                    query_type: format!("asset info for {asset_id}"),
                }
            })?
            .decimals;

        let contract = IERC20::new(asset_id, self.provider.clone());

        let balance_result = contract
            .balanceOf(account)
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
        let balance_decimal = u256_to_decimal(balance, decimals).ok_or_else(|| {
            tracing::warn!(
                error.category = CHAIN_CLIENT,
                error.operation = "fetch_balance",
                asset_id = %asset_id,
                account = %account,
                balance = %balance,
                decimals,
                "Balance is not representable as a Decimal"
            );
            // The balance was fetched successfully; it is the conversion that
            // failed, so this must not be reported as missing data.
            QueryError::DecodeFailed {
                data_type: format!("ERC-20 balance {balance} with {decimals} decimals"),
            }
        })?;

        tracing::trace!(
            ?balance,
            ?balance_decimal,
            "Fetched ERC-20 balance"
        );

        Ok(balance_decimal)
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
            .event_signature(IERC20::Transfer::SIGNATURE_HASH);

        let client = self.clone();
        let confirmations = self.config.confirmations;

        // Subscribe to logs
        let subscription = client
            .subscription_provider
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

        // Subscribe to new heads to know how deep pending transfers are buried
        let blocks_subscription = client
            .subscription_provider
            .subscribe_blocks()
            .await
            .inspect_err(|e| {
                tracing::debug!(
                    error.category = CHAIN_CLIENT,
                    error.operation = "subscribe_transfers",
                    error.source = ?e,
                    "Failed to subscribe to new heads"
                );
            })
            .map_err(|_| SubscriptionError::SubscriptionFailed)?;

        tracing::info!(
            asset_count = asset_ids.len(),
            confirmations,
            "Subscribed to ERC-20 Transfer events"
        );

        let stream = async_stream::try_stream! {
            let mut sub = subscription.into_stream();
            let mut heads = blocks_subscription.into_stream();
            // NOTE: pending (not yet confirmed) transfers are lost when the
            // subscription is recreated; the balance checker is the safety net
            // for transfers missed between subscriptions.
            let mut buffer = ConfirmationBuffer::default();

            loop {
                tokio::select! {
                    log = sub.next() => {
                        let Some(log) = log else {
                            tracing::warn!("Polygon subscription task received None, probably ws connection has been closed");
                            break
                        };

                        let (Some(block_number), Some(transaction_hash), Some(log_index)) =
                            (log.block_number, log.transaction_hash, log.log_index)
                        else {
                            tracing::warn!(
                                ?log,
                                "Transfer log without block number, transaction hash or log index, skipping"
                            );
                            continue
                        };

                        if log.removed {
                            if buffer.remove(transaction_hash, log_index) {
                                tracing::warn!(
                                    %transaction_hash,
                                    log_index,
                                    block_number,
                                    "Transfer log has been reorged away before reaching confirmation depth, dropped"
                                );
                            } else {
                                tracing::error!(
                                    %transaction_hash,
                                    log_index,
                                    block_number,
                                    "Reorged Transfer log wasn't pending — a transfer released past the configured confirmation depth may have been reverted, manual reconciliation required"
                                );
                            }
                            continue
                        }

                        // Decode Transfer event from log
                        match log.log_decode::<IERC20::Transfer>() {
                            Ok(decoded) => {
                                let event = decoded.inner.data;
                                Self::buffer_transfer_log(
                                    &client.asset_info_store,
                                    &mut buffer,
                                    &log,
                                    &event,
                                    block_number,
                                    transaction_hash,
                                    log_index,
                                ).await;
                            },
                            Err(e) => {
                                // One log we cannot decode must not take down tracking for
                                // every other asset on this subscription.
                                tracing::debug!(
                                    error = ?e,
                                    "Failed to decode Transfer event from log, skipping it"
                                );
                                continue
                            },
                        }
                    },
                    head = heads.next() => {
                        let Some(head) = head else {
                            tracing::warn!("Polygon heads subscription received None, probably ws connection has been closed");
                            break
                        };

                        let confirmed = buffer.take_confirmed(head.number, confirmations);

                        if !confirmed.is_empty() {
                            tracing::trace!(
                                latest_block = head.number,
                                transfers = confirmed.len(),
                                "Releasing confirmed ERC-20 transfers"
                            );
                            yield confirmed;
                        }
                    },
                    () = tokio::time::sleep(WS_MESSAGES_TIMEOUT_DURATION) => {
                        tracing::error!("Polygon subscription didn't receive any updates for {} secs, force subscription recreate", WS_MESSAGES_TIMEOUT_DURATION.as_secs());
                        break
                    },
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
        sender: PolygonAccountId,
        recipient: PolygonAccountId,
        asset_id: PolygonAssetId,
        amount: Decimal,
    ) -> Result<UnsignedTransaction<PolygonChainConfig>, TransactionError<PolygonChainConfig>> {
        let decimals = self
            .asset_info_store
            .get_asset_info(&asset_id)
            .await
            .ok_or_else(|| TransactionError::BuildFailed {
                reason: format!("Asset {asset_id} not found in asset info store"),
            })?
            .decimals;

        let amount_wei =
            decimal_to_u256(amount, decimals).ok_or_else(|| TransactionError::BuildFailed {
                reason: format!(
                    "Amount {amount} with {decimals} decimals does not fit u128 base units"
                ),
            })?;

        let contract = IERC20::new(asset_id, self.provider.clone());
        let entrypoint_contract = IERC20::new(ENTRYPOINT, self.provider.clone());

        let sender_nonce = self
            .provider
            .get_transaction_count(sender)
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
            .nonces(sender)
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
                sender,
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
        let permit_hash = Self::build_permit_hash(&sender, permit_nonce);
        let call_data = self.build_call(recipient, amount_wei, asset_id);

        let authorization = Authorization {
            chain_id: U256::from(CHAIN_ID),
            address: ACCOUNT_IMPL,
            nonce: sender_nonce,
        };

        let transaction = PolygonUnsignedTransaction {
            transfer_all: false,
            sender,
            recipient,
            asset_id,
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
        sender: PolygonAccountId,
        recipient: PolygonAccountId,
        asset_id: PolygonAssetId,
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
        inner.op_hash = Some(Self::compute_user_op_hash(
            &inner,
            &paymaster_data,
        )?);
        let encoded_paymaster_data = const_hex::encode_prefixed(paymaster_data.clone());
        inner.paymaster_data = Some(encoded_paymaster_data.clone());

        // if the amount we put is full balance amount we'll get an error that we don't
        // have enough balance for transfer + fees, so we put some dummy amount for now,
        // paymaster fee shouldn't be significantly different depending on amount
        let call_data_for_estimate = self.build_call(
            inner.recipient,
            U256::from(100),
            inner.asset_id,
        );

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

        let usdc_quote = quotes
            .quotes
            .first()
            .ok_or_else(|| TransactionError::BuildFailed {
                reason: "Failed to get quote from paymaster".to_string(),
            })?;

        let max_cost_in_usdc_wei = calculate_max_cost_in_token::<PolygonChainConfig>(
            &inner.gas_params,
            &inner.gas_price,
            usdc_quote,
        )?;

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
        let op_hash = Self::compute_user_op_hash(&inner, &paymaster_data)?;

        if amount_wei.is_zero() {
            return Err(TransactionError::InsufficientBalance {
                transaction_id: op_hash.to_string(),
            })
        }

        // have to recalculate op_hash
        inner.op_hash = Some(op_hash);

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
                    let amount = u256_to_decimal(unsigned.amount_wei, asset_info.decimals)
                        .ok_or_else(|| TransactionError::BuildFailed {
                            reason: format!(
                                "Transferred amount {} with {} decimals is not representable as a Decimal",
                                unsigned.amount_wei, asset_info.decimals
                            ),
                        })?;

                    return Ok(ChainTransfer {
                        asset_id,
                        asset_name: asset_info.name,
                        amount,
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
    use alloy::primitives::{
        address,
        b256,
    };

    use super::*;

    // Known-answer tests for the hashes that money depends on.
    //
    // `alloy-primitives` picks its Keccak backend at build time, and on aarch64
    // `sha3`/`keccak` select an assembly path at runtime via `cpufeatures`. A
    // silently substituted backend would change permit digests, user operation
    // hashes and address derivation without failing to compile. These vectors are
    // hardcoded on purpose: they must fail on any change of result, including one
    // that looks like an upgrade.
    //
    // Provenance matters more than the values here: a vector generated by the code
    // it guards locks in whatever bug that code has. Every constant below was
    // obtained independently of this implementation, and each one records how. Do
    // not refresh a failing value from the new output -- re-derive it the way the
    // comment says it was derived.

    /// Canonical Keccak-256 vectors. Note these are Keccak, not SHA3-256 --
    /// SHA3-256("") is `a7ffc6f8...`, which is what a padding regression looks
    /// like.
    ///
    /// Provenance: recomputed from a from-scratch Keccak-f[1600]
    /// implementation, cross-checked against CPython `hashlib.sha3_256` for
    /// the SHA3 counterpart.
    #[test]
    fn keccak256_matches_known_vectors() {
        assert_eq!(
            keccak256([]),
            b256!("0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"),
        );
        assert_eq!(
            keccak256(b"abc"),
            b256!("0x4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45"),
        );
    }

    /// EIP-712 digest signed to let the paymaster spend USDC. A change here
    /// means permits stop being accepted, or are accepted for something we
    /// did not intend.
    ///
    /// Provenance: the two inputs this digest is built from were read off-chain
    /// rather than taken from this code -- USDC's `DOMAIN_SEPARATOR()` on
    /// Polygon mainnet matches the `eip712_domain!` block, and its
    /// `PERMIT_TYPEHASH()` matches the literal, both exactly.
    #[test]
    fn permit_hash_matches_known_answer() {
        assert_eq!(
            PolygonClient::build_permit_hash(
                &address!("0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7"),
                U256::from(7u64),
            ),
            b256!("0xbc14b20455c000bfec66c92ab17ec39d14d8af591451fcf80437489af2a33a9e"),
        );
    }

    /// ERC-4337 `PackedUserOperation` hash, signed and sent to the bundler.
    ///
    /// Provenance: a live `eth_call` to `getUserOpHash()` on EntryPoint v0.8
    /// (`0x4337084D9E255Ff0702461CF8895CE9E3b5Ff108`, Polygon mainnet) with
    /// this exact fixture returned this value byte-for-byte.
    ///
    /// That address is v0.8, and the code hashes under its `("ERC4337", "1")`
    /// EIP-712 domain. Confirming the pair is the point: a v0.6-style
    /// construction applied to a v0.7+ EntryPoint passes its own test happily
    /// and gets every userOp rejected in production.
    #[test]
    fn user_op_hash_matches_known_answer() {
        assert_eq!(
            PolygonClient::compute_user_op_hash(
                &default_polygon_unsigned_transaction(),
                &[0xde, 0xad, 0xbe, 0xef],
            )
            .unwrap(),
            b256!("0xadcbc48bfdb2401ec19ac83775527235c635fa609de423e3130c436a35dc1853"),
        );
    }

    #[test]
    fn test_u256_decimal_conversion() {
        // 1 USDC = 1_000_000 (6 decimals)
        let value = U256::from(1_000_000_u64);
        let decimal = u256_to_decimal(value, 6).unwrap();
        assert_eq!(decimal, Decimal::new(1, 0)); // 1.0

        // Convert back
        let back = decimal_to_u256(decimal, 6).unwrap();
        assert_eq!(back, value);
    }

    fn test_transfer(amount: u64) -> ChainTransfer<PolygonChainConfig> {
        use alloy::primitives::address;

        ChainTransfer {
            asset_id: address!("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359"),
            asset_name: "USDC".to_string(),
            amount: Decimal::new(amount.try_into().unwrap(), 6),
            sender: address!("0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7"),
            recipient: address!("0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9"),
            transaction_id: "0xdead".to_string(),
            timestamp: 0,
        }
    }

    #[test]
    fn confirmation_buffer_releases_only_deep_enough_blocks() {
        let mut buffer = ConfirmationBuffer::default();
        let tx_a = TxHash::with_last_byte(1);
        let tx_b = TxHash::with_last_byte(2);

        buffer.insert(100, tx_a, 0, test_transfer(1));
        buffer.insert(105, tx_b, 3, test_transfer(2));

        // Head 111 with 12 confirmations: nothing is deep enough yet
        assert!(
            buffer
                .take_confirmed(111, 12)
                .is_empty()
        );

        // Head 112: block 100 is exactly 12 blocks deep
        let released = buffer.take_confirmed(112, 12);
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].amount, Decimal::new(1, 6));

        // Head 117: block 105 becomes deep enough
        let released = buffer.take_confirmed(117, 12);
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].amount, Decimal::new(2, 6));

        // Nothing left
        assert!(
            buffer
                .take_confirmed(1000, 12)
                .is_empty()
        );
    }

    #[test]
    fn confirmation_buffer_zero_confirmations_releases_on_next_head() {
        let mut buffer = ConfirmationBuffer::default();

        buffer.insert(
            100,
            TxHash::with_last_byte(1),
            0,
            test_transfer(1),
        );

        let released = buffer.take_confirmed(100, 0);
        assert_eq!(released.len(), 1);
    }

    #[test]
    fn confirmation_buffer_removes_reorged_logs() {
        let mut buffer = ConfirmationBuffer::default();
        let tx_a = TxHash::with_last_byte(1);
        let tx_b = TxHash::with_last_byte(2);

        buffer.insert(100, tx_a, 0, test_transfer(1));
        buffer.insert(100, tx_b, 1, test_transfer(2));

        // Reorged log is dropped and never released
        assert!(buffer.remove(tx_a, 0));
        let released = buffer.take_confirmed(200, 12);
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].amount, Decimal::new(2, 6));

        // Removing an unknown (or already released) log reports false
        assert!(!buffer.remove(tx_a, 0));
        assert!(!buffer.remove(tx_b, 1));
    }

    #[test]
    fn confirmation_buffer_releases_blocks_in_order() {
        let mut buffer = ConfirmationBuffer::default();

        buffer.insert(
            105,
            TxHash::with_last_byte(2),
            0,
            test_transfer(2),
        );
        buffer.insert(
            100,
            TxHash::with_last_byte(1),
            0,
            test_transfer(1),
        );

        let released = buffer.take_confirmed(200, 12);
        assert_eq!(released.len(), 2);
        assert_eq!(released[0].amount, Decimal::new(1, 6));
        assert_eq!(released[1].amount, Decimal::new(2, 6));
    }

    #[tokio::test]
    async fn unrepresentable_log_does_not_drop_other_transfers_in_batch() {
        let asset_id = address!("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359");
        let store = AssetInfoStore::new();
        store.assets.write().await.insert(
            asset_id,
            AssetInfo {
                name: "USDC".to_string(),
                id: asset_id,
                decimals: 6,
            },
        );

        let mut buffer = ConfirmationBuffer::default();
        let mut bad_log = Log::default();
        bad_log.inner.address = asset_id;
        bad_log.block_number = Some(100);
        bad_log.transaction_hash = Some(TxHash::with_last_byte(1));
        let bad_event = IERC20::Transfer {
            from: Address::with_last_byte(1),
            to: Address::with_last_byte(2),
            value: U256::MAX,
        };

        let mut good_log = bad_log.clone();
        good_log.transaction_hash = Some(TxHash::with_last_byte(2));
        let good_event = IERC20::Transfer {
            value: U256::from(1_000_000_u64),
            ..bad_event.clone()
        };

        PolygonClient::buffer_transfer_log(
            &store,
            &mut buffer,
            &bad_log,
            &bad_event,
            100,
            TxHash::with_last_byte(1),
            0,
        )
        .await;
        PolygonClient::buffer_transfer_log(
            &store,
            &mut buffer,
            &good_log,
            &good_event,
            100,
            TxHash::with_last_byte(2),
            1,
        )
        .await;

        let delivered = buffer.take_confirmed(112, 12);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].amount, Decimal::ONE);
        assert_eq!(
            delivered[0].transaction_id,
            const_hex::encode_prefixed(TxHash::with_last_byte(2))
        );
    }

    /// The budget we hand alloy has to run out while our own supervision is
    /// still waiting, not after it has given up. alloy retries one URL;
    /// `TransfersTracker::recreate()` is the only thing that can rotate to
    /// another endpoint, and it cannot start until alloy stops.
    ///
    /// This duplicates alloy 2.x's backoff formula deliberately. It is the
    /// assumption our two constants were chosen against, and alloy has already
    /// changed it once -- 1.x read the same two numbers as a flat delay, which
    /// is what turned the shared defaults from 27s into 195s. If it changes
    /// again, this is where the stale assumption is written down, and this test
    /// is what fails instead of a payment going unnoticed in production.
    #[test]
    fn ws_reconnect_budget_stays_within_our_own_timeout() {
        // `MAX_RECONNECT_RETRY_INTERVAL` in alloy-pubsub 2.x.
        const ALLOY_BACKOFF_CAP: Duration = Duration::from_secs(30);

        let cap = WS_RECONNECT_RETRY_INTERVAL.max(ALLOY_BACKOFF_CAP);

        // alloy sleeps after every failed attempt except the last, which gives
        // up instead of sleeping.
        let worst_case: Duration = (1..WS_RECONNECT_MAX_RETRIES)
            .map(|retry_count| {
                let multiplier = 1u32
                    .checked_shl(retry_count.saturating_sub(1))
                    .unwrap_or(u32::MAX);

                WS_RECONNECT_RETRY_INTERVAL
                    .saturating_mul(multiplier)
                    .min(cap)
            })
            .sum();

        assert_eq!(worst_case, Duration::from_secs(6));
        assert!(
            worst_case < WS_MESSAGES_TIMEOUT_DURATION,
            "alloy would still be retrying after {worst_case:?}, past our own \
             {WS_MESSAGES_TIMEOUT_DURATION:?} timeout, blocking endpoint rotation",
        );
    }

    #[test]
    fn u256_to_decimal_handles_18_decimal_tokens() {
        // 10 units of an 18-decimal token: 10e18 base units. This exceeds
        // `i64::MAX` (~9.22e18 is close, 10e18 is over) and used to be the
        // motivating case for silent corruption.
        let value = U256::from(10_000_000_000_000_000_000_u128);
        assert_eq!(
            u256_to_decimal(value, 18).unwrap(),
            Decimal::new(10, 0)
        );
    }

    #[test]
    fn u256_to_decimal_rejects_values_beyond_decimal_range() {
        // Decimal's mantissa is 96 bits (~7.9e28); 1e30 base units cannot be
        // represented. The old code returned `Decimal::ZERO`, silently turning
        // a huge payment into "nothing received".
        let value = U256::from(10u8).pow(U256::from(30u8));
        assert_eq!(u256_to_decimal(value, 18), None);

        // U256::MAX must not panic either.
        assert_eq!(u256_to_decimal(U256::MAX, 18), None);
    }

    #[test]
    fn u256_to_decimal_rejects_scale_above_decimal_max() {
        // `Decimal::new(1, 29)` panics; the fallible path must return None.
        assert_eq!(
            u256_to_decimal(U256::from(1u8), 29),
            None
        );
    }

    #[test]
    fn decimal_to_u256_rejects_unrepresentable_values() {
        // `10_i64.pow(19)` overflows i64.
        assert_eq!(decimal_to_u256(Decimal::ONE, 19), None);

        // Scaled value beyond u128 must not silently become zero.
        assert_eq!(decimal_to_u256(Decimal::MAX, 18), None);
    }

    #[test]
    fn u256_to_u128_accepts_max_and_rejects_one_over() {
        let max = U256::from(u128::MAX);
        assert_eq!(
            u256_to_u128::<PolygonChainConfig>(max, "call_gas_limit").unwrap(),
            u128::MAX
        );

        let over = max
            .checked_add(U256::from(1u8))
            .unwrap();
        let err = u256_to_u128::<PolygonChainConfig>(over, "call_gas_limit").unwrap_err();
        assert!(matches!(
            err,
            TransactionError::BuildFailed { .. }
        ));
    }

    fn quote(
        post_op_gas: U256,
        exchange_rate: U256,
    ) -> TokenQuote {
        TokenQuote {
            token: Address::ZERO,
            paymaster: Address::ZERO,
            exchange_rate,
            post_op_gas,
            exchange_rate_native_to_usd: U256::ZERO,
            balance_slot: U256::ZERO,
            allowance_slot: U256::ZERO,
        }
    }

    #[test]
    fn calculate_max_cost_in_token_computes_realistic_quote() {
        // 115_000 total gas * 100 gwei = 1.15e16 wei, + 40_000 * 100 gwei
        // = 1.55e16 wei; times an exchange rate of 1e18 (1:1), divided by 1e18.
        let gas_params = GasParams::dummy();
        let gas_price = GasPrice {
            max_fee_per_gas: U256::from(100_000_000_000_u64),
            max_priority_fee_per_gas: U256::from(1_000_000_000_u64),
        };

        let cost = calculate_max_cost_in_token::<PolygonChainConfig>(
            &gas_params,
            &gas_price,
            &quote(U256::from(40_000), WEI_PER_ETHER),
        )
        .unwrap();

        assert_eq!(
            cost,
            U256::from(15_500_000_000_000_000_u64)
        );
    }

    #[test]
    fn calculate_max_cost_in_token_rejects_hostile_quote() {
        // A bundler returning U256::MAX everywhere must produce an error, not
        // a panic and not a wrapped-around (tiny) fee silently deducted from
        // the customer's payout.
        let gas_params = GasParams {
            pre_verification_gas: U256::MAX,
            call_gas_limit: U256::MAX,
            verification_gas_limit: U256::MAX,
            paymaster_post_op_gas_limit: U256::MAX,
            paymaster_verification_gas_limit: U256::MAX,
        };
        let gas_price = GasPrice {
            max_fee_per_gas: U256::MAX,
            max_priority_fee_per_gas: U256::MAX,
        };

        let err = calculate_max_cost_in_token::<PolygonChainConfig>(
            &gas_params,
            &gas_price,
            &quote(U256::MAX, U256::MAX),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            TransactionError::BuildFailed { .. }
        ));
    }

    #[test]
    fn calculate_max_cost_in_token_rejects_overflowing_exchange_rate() {
        // Gas sums fine, but the exchange rate multiplication overflows.
        let gas_price = GasPrice {
            max_fee_per_gas: U256::from(1u8),
            max_priority_fee_per_gas: U256::from(1u8),
        };

        let err = calculate_max_cost_in_token::<PolygonChainConfig>(
            &GasParams::dummy(),
            &gas_price,
            &quote(U256::from(1u8), U256::MAX),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            TransactionError::BuildFailed { .. }
        ));
    }

    #[test]
    fn wei_per_ether_is_ten_to_the_eighteenth() {
        assert_eq!(
            WEI_PER_ETHER,
            U256::from(10u8).pow(U256::from(18u8))
        );
    }

    #[test]
    fn decimal_to_u256_rejects_negative_amounts() {
        // The sign check is what `to_u128` provides. Without it a negative
        // payout amount would scale into an unsigned base-unit value and move
        // real funds; the existing cases here all use positive inputs.
        assert_eq!(decimal_to_u256(Decimal::try_new(-1, 0).unwrap(), 6), None);
    }
}
