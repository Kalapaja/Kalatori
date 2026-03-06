use std::io::Error as IoError;

use serde_json::{
    Error as JsonError,
    Value,
};
use thiserror::Error;
use tokio::task::JoinError;
use tracing_subscriber::filter::ParseError;

#[derive(Debug, Error)]
#[expect(dead_code)]
pub enum Error {
    #[error("failed to read the config file at {0:?}")]
    ConfigFileRead(String, #[source] IoError),

    #[error("failed to parse the config parameter `{0}`")]
    ConfigParse(&'static str),

    #[error("chain {0:?} doesn't have any `endpoints` in the config")]
    EmptyEndpoints(String),

    #[error("RPC server error is occurred")]
    Chain(#[from] ChainError),

    #[error("database error is occurred")]
    Db(#[from] DbError),

    #[error("DAO error is occurred")]
    Dao(#[from] DaoError),

    #[error("order error is occurred")]
    Order(#[from] OrderError),

    #[error("signer error is occurred")]
    Signer(#[from] SignerError),

    #[error("failed to listen for the shutdown signal")]
    ShutdownSignal(#[source] IoError),

    #[error("failed to initialize the asynchronous runtime")]
    Runtime(#[source] IoError),

    #[error("failed to parse given filter directives for the logger")]
    LoggerDirectives(#[from] ParseError),

    #[error("receiver account couldn't be parsed: {0}")]
    RecipientAccount(String),

    #[error("fatal error is occurred")]
    Fatal,

    #[error("found duplicate config record for the token {0:?}")]
    DuplicateCurrency(String),

    #[error("keyring error {0:?}")]
    KeyringError(#[from] crate::chain_client::KeyringError),

    #[error("chain client initialization failed")]
    ChainClientInit(#[from] crate::chain_client::ClientError),

    #[error("chain query failed")]
    ChainQuery(#[from] crate::chain_client::QueryError),

    #[error("chain subscription failed")]
    ChainSubscription(#[from] crate::chain_client::SubscriptionError),

    #[error("transaction failed")]
    ChainTransaction(
        #[from] crate::chain_client::TransactionError<crate::chain_client::AssetHubChainConfig>,
    ),
}

impl From<crate::dao::DaoInvoiceError> for Error {
    fn from(e: crate::dao::DaoInvoiceError) -> Self {
        // Convert InvoiceError to the legacy DaoError variant
        // Log the conversion for debugging
        tracing::debug!(
            error.category = "error_conversion",
            error.source = ?e,
            "Converting InvoiceError to Error::Dao"
        );

        // Map to appropriate DaoError variant
        let dao_error = match e {
            crate::dao::DaoInvoiceError::NotFound {
                ..
            } => DaoError::InvoiceNotFound,
            crate::dao::DaoInvoiceError::UpdateNotAllowed {
                ..
            } => DaoError::VersionConflict,
            _ => DaoError::Sqlx(sqlx::Error::RowNotFound),
        };

        Error::Dao(dao_error)
    }
}

impl From<crate::dao::DaoTransactionError> for Error {
    fn from(e: crate::dao::DaoTransactionError) -> Self {
        tracing::debug!(
            error.category = "error_conversion",
            error.source = ?e,
            "Converting TransactionError to Error::Dao"
        );

        // Map TransactionError to DaoError
        Error::Dao(DaoError::Sqlx(sqlx::Error::RowNotFound))
    }
}

impl From<Error> for ChainError {
    fn from(_err: Error) -> Self {
        ChainError::Util(UtilError::NotHex(
            NotHexError::BlockHash,
        ))
    }
}

#[derive(Debug, Error)]
#[expect(dead_code)]
pub enum ChainError {
    // TODO: this should be prevented by typesafety
    #[error("asset ID is missing")]
    AssetId,

    #[error("asset ID isn't `u32`")]
    AssetIdFormat,

    #[error("invalid assets for the chain {0:?}")]
    AssetsInvalid(String),

    #[error("asset key has no parceable part")]
    AssetKeyEmpty,

    #[error("asset key isn't single hash")]
    AssetKeyNotSingleHash,

    #[error("asset metadata isn't a map")]
    AssetMetadataPlain,

    #[error("unexpected assets metadata value structure")]
    AssetMetadataUnexpected,

    #[error("wrong data type")]
    AssetMetadataType,

    #[error("expected a map with a single entry, got multiple entries")]
    AssetMetadataMapSize,

    #[error("asset balance format is unexpected")]
    AssetBalanceFormat,

    #[error("no balance field in an asset record")]
    AssetBalanceNotFound,

    #[error("format of the fetched Base58 prefix {0:?} isn't supported")]
    Base58PrefixFormatNotSupported(String),

    #[error("Base58 prefixes in metadata ({meta:?}) and specs ({specs:?}) do not match.")]
    Base58PrefixMismatch { specs: u16, meta: u16 },

    #[error("unexpected block number format")]
    BlockNumberFormat,

    #[error("unexpected block hash format")]
    BlockHashFormat,

    #[error("unexpected block hash length")]
    BlockHashLength,

    #[error("threading error is occurred")]
    Tokio(#[from] JoinError),

    #[error("format of fetched decimals ({0}) isn't supported")]
    DecimalsFormatNotSupported(String),

    #[error("unexpected genesis hash format")]
    GenesisHashFormat,

    #[error("...")]
    MetadataFormat,

    #[error("...")]
    MetadataNotDecodeable,

    #[error("no Base58 prefix is fetched as system properties or found in metadata")]
    NoBase58Prefix,

    #[error("block number definition isn't found")]
    NoBlockNumberDefinition,

    #[error("no decimals value is fetched")]
    NoDecimals,

    #[error("metadata v15 isn't available through RPC")]
    NoMetadataV15,

    #[error("metadata must start with the `meta` prefix")]
    NoMetaPrefix,

    #[error("pallet isn't found")]
    NoPallet,

    #[error("no pallets with a storage found")]
    NoStorage,

    #[error("\"System\" pallet isn't found")]
    NoSystem,

    #[error("no storage variants in the \"System\" pallet")]
    NoStorageInSystem,

    #[error("no unit value is fetched")]
    NoUnit,

    #[error("...")]
    PropertiesFormat,

    #[error("...")]
    RawMetadataNotDecodeable,

    #[error("format of the fetched unit ({0}) isn't supported")]
    UnitFormatNotSupported(String),

    #[error("unexpected storage value format for the key \"{0:?}\"")]
    StorageValueFormat(Value),

    #[error("internal error is occurred")] // TODO this should be replaced by specific errors
    Util(#[from] UtilError),

    #[error("invoice account couldn't be parsed")]
    InvoiceAccount(String),

    #[error("chain {0:?} isn't found")]
    InvalidChain(String),

    #[error("currency {0:?} isn't found")]
    InvalidCurrency(String),

    #[error(
        "chain manager dropped a message, probably due to a chain disconnect; maybe it should be sent again"
    )]
    MessageDropped,

    #[error("block subscription is terminated")]
    BlockSubscriptionTerminated,

    #[error("balance wasn't found")]
    BalanceNotFound,

    #[error("storage query couldn't be formed")]
    StorageQuery,

    #[error("events couldn't be fetched")]
    EventsMissing,

    #[error("no events in this chain")]
    EventsNonexistant,

    #[error("transaction isn't ready to be signed: {0:?}")]
    TransactionNotSignable(String),

    #[error("signing was failed")]
    Signer(#[from] SignerError),

    #[error("transaction couldn't be completed")]
    NothingToSend,

    #[error("storage entry isn't a map")]
    StorageEntryNotMap,

    #[error("storage entry map has more than one record")]
    StorageEntryMapMultiple,

    #[error("storage key {0:?} isn't found")]
    StorageKeyNotFound(String),

    #[error("storage key isn't `u32`")]
    StorageKeyNotU32,

    #[error(
        "RPC runs on an unexpected network: instead of {expected:?}, found {actual:?} at {rpc:?}"
    )]
    WrongNetwork {
        expected: String,
        actual: String,
        rpc: String,
    },

    #[error("failed to parse JSON data from a block stream")]
    Serde(#[from] JsonError),

    #[error("failed to send a constructed transaction back to the state")]
    TransactionNotSaved,

    #[error("timestamp wasn't found in the block")]
    TimestampNotFoundForBlock,

    #[error("transfer event has no matching extrinsic")]
    TransferEventNoExtrinsic,

    // TODO: improve error details
    #[error("subxt error")]
    Subxt(Box<subxt::Error>),
}

impl From<subxt::Error> for ChainError {
    fn from(err: subxt::Error) -> Self {
        ChainError::Subxt(Box::new(err))
    }
}

#[derive(Debug, Error)]
#[expect(dead_code)]
pub enum DbError {
    #[error("currency key isn't found")]
    CurrencyKeyNotFound,

    #[error("database engine isn't running")]
    DbEngineDown,

    #[error("operating system related I/O error is occurred")]
    IoError(#[from] IoError),

    #[error("order {0:?} isn't found")]
    OrderNotFound(String),

    #[error("order {0:?} was already paid")]
    AlreadyPaid(String),

    #[error("order {0:?} isn't paid yet")]
    NotPaid(String),

    #[error("there was already an attempt to withdraw order {0:?}")]
    WithdrawalWasAttempted(String),

    #[error("wasn't able to serialize {0:?} field")]
    SerializationError(String),

    #[error("wasn't able to deserialize {0:?} table")]
    DeserializationError(String),
}

#[derive(Debug, Error)]
pub enum DaoError {
    #[error("SQLite database error")]
    Sqlx(#[from] sqlx::Error),

    #[error("invoice not found")]
    InvoiceNotFound,

    #[error("version conflict: invoice was modified by another request")]
    VersionConflict,
}

#[derive(Debug, Error)]
#[expect(dead_code)]
pub enum OrderError {
    #[error("invoice amount is less than the existential deposit")]
    LessThanExistentialDeposit(f64),

    #[error("unknown currency")]
    UnknownCurrency,

    #[error("order parameter is missing: {0:?}")]
    MissingParameter(String),

    #[error("order parameter invalid: {0:?}")]
    InvalidParameter(String),

    #[error("internal error is occurred")]
    InternalError,
}

#[derive(Debug, Error)]
#[expect(dead_code)]
pub enum ForceWithdrawalError {
    #[error("order parameter is missing: {0:?}")]
    MissingParameter(String),

    #[error("order parameter is invalid: {0:?}")]
    InvalidParameter(String),

    #[error("withdrawal was failed: \"{0:?}\"")]
    WithdrawalError(String),
}

#[derive(Debug, Error)]
pub enum UtilError {
    #[error("...")]
    NotHex(NotHexError),
}

#[derive(Debug, Error)]
#[expect(dead_code)]
pub enum SignerError {
    #[error("failed to read {0:?}")]
    Env(String),

    #[error("signer is down")]
    SignerDown,

    #[error("mnemonic phrase is invalid")]
    InvalidMnemonic(#[from] subxt_signer::bip39::Error),

    #[error("seed phrase is invalid")]
    InvalidSeed(#[from] subxt_signer::sr25519::Error),

    #[error("derivation was failed")]
    InvalidDerivation(String),
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum NotHexError {
    #[error("block hash string isn't a valid hexadecimal")]
    BlockHash,
}
