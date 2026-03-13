use thiserror::Error;

use crate::utils::logging::category::CHAIN_CLIENT;

use super::{
    ChainConfig,
    KeyringError,
};

// ============================================================================
// Domain 1: Client Initialization Errors
// ============================================================================

/// Errors that occur during chain client initialization
#[derive(Debug, Error)]
pub enum ClientError {
    /// All configured RPC endpoints are unreachable
    #[error("All configured RPC endpoints are unreachable")]
    AllEndpointsUnreachable,

    /// Failed to fetch chain metadata during initialization
    #[error("Failed to fetch chain metadata during initialization")]
    MetadataFetchFailed,

    /// Invalid configuration detected
    #[error("Invalid configuration: {field}")]
    InvalidConfiguration { field: String },

    #[expect(dead_code)]
    /// Unknown asset ID in configuration (validated at init AND runtime per
    /// Principle 1)
    #[error("Unknown asset ID in configuration: {asset_id}")]
    UnknownAssetId { asset_id: u32 },
}

// ============================================================================
// Domain 2: Query Operation Errors (one-off blockchain state queries)
// ============================================================================

/// Errors for one-off blockchain queries (balance, asset info, etc.)
#[derive(Debug, Error)]
pub enum QueryError {
    /// RPC request failed - triggers endpoint failover
    #[error("RPC request failed")]
    RpcRequestFailed,

    /// Storage query returned no data
    #[error("Storage query returned no data: {query_type}")]
    NotFound { query_type: String },

    #[expect(dead_code)]
    /// Data decoding failed (SCALE or other format)
    #[error("Data decoding failed: {data_type}")]
    DecodeFailed { data_type: String },
}

// ============================================================================
// Domain 3: Subscription Operation Errors (block streaming)
// ============================================================================

/// Errors for block subscription and streaming operations
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SubscriptionError {
    /// Asset info for the asset is not presented in local asset info store
    #[error("Asset info not found for asset id {asset_id}")]
    AssetNotFound { asset_id: u32 },

    /// Failed to establish initial block subscription
    #[error("Failed to establish block subscription")]
    SubscriptionFailed,

    /// Block stream ended unexpectedly
    #[error("Block stream ended unexpectedly")]
    StreamClosed,

    /// Failed to process an individual block (non-fatal for stream)
    #[error("Failed to process block {block_number}")]
    BlockProcessingFailed { block_number: u32 },
}

// ============================================================================
// Domain 4: Transaction Lifecycle Errors
// ============================================================================

/// Errors for transaction building, submission, and finalization
#[derive(Debug, Error)]
pub enum TransactionError<T: ChainConfig> {
    // TODO: should be either splitted to different retriable/non-retriable errors or have a flag
    // Asset Hub client makes some requests to the chain that can fail transiently and should be
    // retried, on the other hand some errors are permanent and should not be retried.
    /// Transaction building failed (invalid parameters, signing failure, etc.)
    #[error("Transaction building failed: {reason}")]
    BuildFailed { reason: String },

    /// Transaction was submitted but final status is unknown
    #[error("Transaction submission status unknown")]
    SubmissionStatusUnknown,

    /// Transaction was finalized but it's status unknown
    #[error("Transaction finalized but it's status unknown")]
    TransactionInfoFetchFailed {
        /// Blockchain coordinates: (`block_number`, `extrinsic_index`)
        transaction_id: T::TransactionId,
    },

    /// Transaction finalized but execution failed on-chain
    #[error("Transaction execution failed on-chain: {error_code}")]
    ExecutionFailed {
        /// Blockchain coordinates: (`block_number`, `extrinsic_index`)
        transaction_id: T::TransactionId,
        /// Runtime error code (e.g., "`Assets::BalanceLow`")
        error_code: String,
    },

    /// Insufficient balance for transaction
    #[error("Insufficient balance for transaction")]
    InsufficientBalance {
        /// Blockchain coordinates (available after finalization)
        transaction_id: T::TransactionId,
    },

    /// Unknown asset ID (runtime check, despite init validation - defense in
    /// depth)
    #[error("Unknown asset: {asset_id:?}")]
    UnknownAsset {
        transaction_id: T::TransactionId,
        asset_id: T::AssetId,
    },
}

// ============================================================================
// Cross-Domain Conversions
// ============================================================================

/// Convert `KeyringError` to `TransactionError`
///
/// Signing errors occur during transaction building phase
impl<T: ChainConfig> From<KeyringError> for TransactionError<T> {
    fn from(e: KeyringError) -> Self {
        // Log conversion per Principle 2
        tracing::debug!(
            error.category = CHAIN_CLIENT,
            error.source = ?e,
            "Keyring error during transaction operations"
        );

        TransactionError::BuildFailed {
            reason: format!("Signing failed: {e}"),
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Check if a `DispatchError` indicates insufficient balance
///
/// Examines error details to determine if failure was due to low balance.
/// This enables converting generic `ExecutionFailed` to more specific
/// `InsufficientBalance`.
///
/// Note: Generic over any type that implements Debug to support different
/// runtime error types
pub fn is_insufficient_balance_error<T: std::fmt::Debug>(error: &T) -> bool {
    // Check if error contains balance-related error strings
    let error_details = format!("{error:?}");
    error_details.contains("BalanceLow")
        || error_details.contains("InsufficientBalance")
        || error_details.contains("BalanceTooLow")
}
