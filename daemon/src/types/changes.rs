//! Types for the invoice changes/sync endpoint.
//!
//! These types support fetching invoices with all related entities
//! (transactions, payouts, refunds) that have been modified after a given
//! timestamp.

use chrono::{
    DateTime,
    Utc,
};
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use uuid::Uuid;

use kalatori_client::types::{
    ChainType,
    Invoice as PublicInvoice,
    Transaction as PublicTransaction,
    TransactionType,
};

use super::{
    FrontEndSwap,
    InitiatorType,
    Invoice,
    OutgoingTransactionMeta,
    Payout,
    PayoutStatus,
    Refund,
    RefundStatus,
    RetryMeta,
    SwapChainType,
    Transaction,
    TransactionOrigin,
    TransactionStatus,
    TransferDestinationParams,
    TransferInfo,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetChangesParams {
    /// Return entities modified after this timestamp. If timestamp is not
    /// specified, returns all entities.
    pub since: Option<DateTime<Utc>>,
}

// ============================================================================
// Public types for API responses
// ============================================================================

/// Payout with its related outgoing transactions (for API responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicPayoutChanges {
    #[serde(flatten)]
    pub payout: Payout,
    /// Outgoing transactions that belong to this payout
    pub transactions: Vec<PublicTransaction>,
}

/// Refund with its related outgoing transactions (for API responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicRefundChanges {
    #[serde(flatten)]
    pub refund: Refund,
    /// Outgoing transactions that belong to this refund
    pub transactions: Vec<PublicTransaction>,
}

/// Public invoice with all related entities (for API responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicInvoiceChanges {
    #[serde(flatten)]
    pub invoice: PublicInvoice,
    /// Incoming transactions (payments from customers)
    pub incoming_transactions: Vec<PublicTransaction>,
    /// Payouts with their outgoing transactions
    pub payouts: Vec<PublicPayoutChanges>,
    /// Refunds with their outgoing transactions
    pub refunds: Vec<PublicRefundChanges>,
    /// Front-end swaps (cross-chain payments via bridge)
    pub swaps: Vec<FrontEndSwap>,
}

/// Public response for the changes sync endpoint (for API responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicChangesResponse {
    pub invoices: Vec<PublicInvoiceChanges>,
    /// Use this timestamp for the next sync request
    pub sync_timestamp: DateTime<Utc>,
    pub kalatori_version: &'static str,
}

// ============================================================================
// Internal types for DAO layer
// ============================================================================

/// Payout with its related outgoing transactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayoutChanges {
    #[serde(flatten)]
    pub payout: Payout,
    /// Outgoing transactions that belong to this payout (have this payout's id
    /// in origin)
    pub transactions: Vec<Transaction>,
}

/// Refund with its related outgoing transactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefundChanges {
    #[serde(flatten)]
    pub refund: Refund,
    /// Outgoing transactions that belong to this refund (have this refund's id
    /// in origin)
    pub transactions: Vec<Transaction>,
}

/// Invoice with all related entities for the changes endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceChanges {
    #[serde(flatten)]
    pub invoice: Invoice,
    /// Incoming transactions (payments from customers)
    pub transactions: Vec<Transaction>,
    /// Payouts with their outgoing transactions
    pub payouts: Vec<PayoutChanges>,
    /// Refunds with their outgoing transactions
    pub refunds: Vec<RefundChanges>,
    /// Front-end swaps (cross-chain payments via bridge)
    pub swaps: Vec<FrontEndSwap>,
}

/// Response for the changes sync endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesResponse {
    pub invoices: Vec<InvoiceChanges>,
    /// Use this timestamp for the next sync request
    pub sync_timestamp: DateTime<Utc>,
}

// ============================================================================
// Conversion from internal to public types
// ============================================================================

impl ChangesResponse {
    /// Convert internal response to public response for API.
    /// Requires `payment_url_base` to generate invoice payment URLs.
    pub fn into_public(
        self,
        payment_url_base: &str,
    ) -> PublicChangesResponse {
        PublicChangesResponse {
            invoices: self
                .invoices
                .into_iter()
                .map(|ic| ic.into_public(payment_url_base))
                .collect(),
            sync_timestamp: self.sync_timestamp,
            kalatori_version: VERSION,
        }
    }
}

impl InvoiceChanges {
    fn into_public(
        self,
        payment_url_base: &str,
    ) -> PublicInvoiceChanges {
        // Calculate total received amount from incoming transactions
        let total_received_amount: Decimal = self
            .transactions
            .iter()
            .map(|t| t.transfer_info.amount)
            .sum();

        // Convert invoice to public invoice
        let public_invoice = self
            .invoice
            .with_amount(total_received_amount)
            .into_public_invoice(payment_url_base);

        PublicInvoiceChanges {
            invoice: public_invoice,
            incoming_transactions: self
                .transactions
                .into_iter()
                .map(PublicTransaction::from)
                .collect(),
            payouts: self
                .payouts
                .into_iter()
                .map(PayoutChanges::into_public)
                .collect(),
            refunds: self
                .refunds
                .into_iter()
                .map(RefundChanges::into_public)
                .collect(),
            swaps: self.swaps,
        }
    }
}

impl PayoutChanges {
    fn into_public(self) -> PublicPayoutChanges {
        PublicPayoutChanges {
            payout: self.payout,
            transactions: self
                .transactions
                .into_iter()
                .map(PublicTransaction::from)
                .collect(),
        }
    }
}

impl RefundChanges {
    fn into_public(self) -> PublicRefundChanges {
        PublicRefundChanges {
            refund: self.refund,
            transactions: self
                .transactions
                .into_iter()
                .map(PublicTransaction::from)
                .collect(),
        }
    }
}

// ============================================================================
// Intermediate types for JSON parsing from SQLite
// ============================================================================

/// Transaction as returned from SQLite JSON aggregation.
/// UUIDs come as hex strings, nested JSON fields need parsing.
#[derive(Debug, Clone, Deserialize)]
pub struct TransactionJson {
    pub id: String,         // hex-encoded UUID
    pub invoice_id: String, // hex-encoded UUID
    pub asset_id: String,
    pub asset_name: String,
    pub chain: ChainType,
    pub amount: Decimal,
    pub source_address: String,
    pub destination_address: String,
    pub block_number: Option<u32>,
    pub position_in_block: Option<u32>,
    pub tx_hash: Option<String>,
    pub origin: TransactionOrigin, // nested JSON
    pub status: TransactionStatus,
    pub transaction_type: TransactionType,
    pub outgoing_meta: OutgoingTransactionMeta, // nested JSON
    #[serde(deserialize_with = "deserialize_sqlite_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(deserialize_with = "deserialize_sqlite_datetime")]
    pub updated_at: DateTime<Utc>,
}

/// Payout as returned from SQLite JSON aggregation.
#[derive(Debug, Clone, Deserialize)]
pub struct PayoutJson {
    pub id: String,         // hex-encoded UUID
    pub invoice_id: String, // hex-encoded UUID
    pub asset_id: String,
    pub asset_name: String,
    pub chain: ChainType,
    pub amount: Decimal,
    pub source_address: String,
    pub destination_address: String,
    pub destination_chain: SwapChainType,
    pub destination_asset_id: String,
    pub initiator_type: InitiatorType,
    #[serde(deserialize_with = "deserialize_optional_hex_uuid")]
    pub initiator_id: Option<String>, // hex-encoded UUID, empty string, or null
    pub status: PayoutStatus,
    pub retry_count: u32,
    #[serde(deserialize_with = "deserialize_sqlite_datetime_opt")]
    pub last_attempt_at: Option<DateTime<Utc>>,
    #[serde(deserialize_with = "deserialize_sqlite_datetime_opt")]
    pub next_retry_at: Option<DateTime<Utc>>,
    pub failure_message: Option<String>,
    #[serde(deserialize_with = "deserialize_sqlite_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(deserialize_with = "deserialize_sqlite_datetime")]
    pub updated_at: DateTime<Utc>,
}

/// Refund as returned from SQLite JSON aggregation.
#[derive(Debug, Clone, Deserialize)]
pub struct RefundJson {
    pub id: String,         // hex-encoded UUID
    pub invoice_id: String, // hex-encoded UUID
    pub asset_id: String,
    pub asset_name: String,
    pub chain: ChainType,
    pub amount: Decimal,
    pub source_address: String,
    pub destination_address: Option<String>,
    pub destination_chain: Option<SwapChainType>,
    pub destination_asset_id: Option<String>,
    pub initiator_type: InitiatorType,
    #[serde(deserialize_with = "deserialize_optional_hex_uuid")]
    pub initiator_id: Option<String>, // hex-encoded UUID, empty string, or null
    pub status: RefundStatus,
    pub retry_count: u32,
    #[serde(deserialize_with = "deserialize_sqlite_datetime_opt")]
    pub last_attempt_at: Option<DateTime<Utc>>,
    #[serde(deserialize_with = "deserialize_sqlite_datetime_opt")]
    pub next_retry_at: Option<DateTime<Utc>>,
    pub failure_message: Option<String>,
    #[serde(deserialize_with = "deserialize_sqlite_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(deserialize_with = "deserialize_sqlite_datetime")]
    pub updated_at: DateTime<Utc>,
}

/// Front-end swap as returned from SQLite JSON aggregation.
#[derive(Debug, Clone, Deserialize)]
pub struct FrontEndSwapJson {
    pub id: String,
    pub invoice_id: String,        // hex-encoded UUID
    pub from_amount_units: String, // u128 stored as TEXT
    pub from_chain_id: u32,
    pub from_asset_id: String, // hex address
    pub transaction_hash: String,
    #[serde(deserialize_with = "deserialize_sqlite_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(deserialize_with = "deserialize_sqlite_datetime")]
    pub updated_at: DateTime<Utc>,
}

// ============================================================================
// Custom deserializers for SQLite JSON output
// ============================================================================

use serde::de::{
    self,
    Deserializer,
};

/// Deserialize SQLite datetime format "YYYY-MM-DD HH:MM:SS.SSSSSS" to
/// DateTime<Utc>
fn deserialize_sqlite_datetime<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;

    // Try ISO 8601 first, then SQLite format
    if let Ok(dt) = DateTime::parse_from_rfc3339(&s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // SQLite format: "2026-02-05 21:34:39.794335"
    chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S"))
        .map(|ndt| ndt.and_utc())
        .map_err(|e| {
            de::Error::custom(format!(
                "invalid datetime '{}': {}",
                s, e
            ))
        })
}

/// Deserialize optional SQLite datetime
fn deserialize_sqlite_datetime_opt<'de, D>(
    deserializer: D
) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Deserialize::deserialize(deserializer)?;

    match opt {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => {
            // Try ISO 8601 first, then SQLite format
            if let Ok(dt) = DateTime::parse_from_rfc3339(&s) {
                return Ok(Some(dt.with_timezone(&Utc)));
            }

            chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S"))
                .map(|ndt| Some(ndt.and_utc()))
                .map_err(|e| {
                    de::Error::custom(format!(
                        "invalid datetime '{}': {}",
                        s, e
                    ))
                })
        },
    }
}

/// Deserialize optional hex UUID that might be empty string or null
fn deserialize_optional_hex_uuid<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Deserialize::deserialize(deserializer)?;

    match opt {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => Ok(Some(s)),
    }
}

// ============================================================================
// Conversion from JSON types to domain types
// ============================================================================

/// Parse a hex-encoded UUID string to Uuid.
fn parse_hex_uuid(hex: &str) -> Result<Uuid, String> {
    let bytes = const_hex::decode(hex).map_err(|e| format!("Invalid hex string: {e}"))?;

    Uuid::from_slice(&bytes).map_err(|e| format!("Invalid UUID bytes: {e}"))
}

/// Parse an optional hex-encoded UUID string to Option<Uuid>.
fn parse_hex_uuid_opt(hex: Option<&str>) -> Result<Option<Uuid>, String> {
    match hex {
        Some(h) if !h.is_empty() => parse_hex_uuid(h).map(Some),
        _ => Ok(None),
    }
}

impl TryFrom<TransactionJson> for Transaction {
    type Error = String;

    fn try_from(json: TransactionJson) -> Result<Self, Self::Error> {
        use super::GeneralTransactionId;

        Ok(Transaction {
            id: parse_hex_uuid(&json.id)?,
            invoice_id: parse_hex_uuid(&json.invoice_id)?,
            transfer_info: TransferInfo {
                chain: json.chain,
                asset_id: json.asset_id,
                asset_name: json.asset_name,
                amount: json.amount,
                source_address: json.source_address,
                destination_address: json.destination_address,
            },
            transaction_id: GeneralTransactionId {
                block_number: json.block_number,
                position_in_block: json.position_in_block,
                tx_hash: json.tx_hash,
            },
            origin: json.origin,
            status: json.status,
            transaction_type: json.transaction_type,
            outgoing_meta: json.outgoing_meta,
            created_at: json.created_at,
            updated_at: json.updated_at,
        })
    }
}

impl TryFrom<PayoutJson> for Payout {
    type Error = String;

    fn try_from(json: PayoutJson) -> Result<Self, Self::Error> {
        Ok(Payout {
            id: parse_hex_uuid(&json.id)?,
            invoice_id: parse_hex_uuid(&json.invoice_id)?,
            initiator_type: json.initiator_type,
            initiator_id: parse_hex_uuid_opt(json.initiator_id.as_deref())?,
            status: json.status,
            created_at: json.created_at,
            updated_at: json.updated_at,
            chain: json.chain,
            asset_id: json.asset_id,
            asset_name: json.asset_name,
            amount: json.amount,
            source_address: json.source_address,
            destination_params: TransferDestinationParams {
                destination_address: json.destination_address,
                destination_chain: json.destination_chain,
                destination_asset_id: json.destination_asset_id,
            },
            retry_meta: RetryMeta {
                retry_count: json.retry_count,
                last_attempt_at: json.last_attempt_at,
                next_retry_at: json.next_retry_at,
                failure_message: json.failure_message,
            },
            fee: None,
        })
    }
}

impl TryFrom<RefundJson> for Refund {
    type Error = String;

    fn try_from(json: RefundJson) -> Result<Self, Self::Error> {
        let destination_params = if let (
            Some(destination_address),
            Some(destination_chain),
            Some(destination_asset_id),
        ) = (
            json.destination_address,
            json.destination_chain,
            json.destination_asset_id,
        ) {
            Some(TransferDestinationParams {
                destination_address,
                destination_chain,
                destination_asset_id,
            })
        } else {
            None
        };

        Ok(Refund {
            id: parse_hex_uuid(&json.id)?,
            invoice_id: parse_hex_uuid(&json.invoice_id)?,
            initiator_type: json.initiator_type,
            initiator_id: parse_hex_uuid_opt(json.initiator_id.as_deref())?,
            status: json.status,
            created_at: json.created_at,
            updated_at: json.updated_at,
            chain: json.chain,
            asset_id: json.asset_id,
            asset_name: json.asset_name,
            amount: json.amount,
            source_address: json.source_address,
            destination_params,
            retry_meta: RetryMeta {
                retry_count: json.retry_count,
                last_attempt_at: json.last_attempt_at,
                next_retry_at: json.next_retry_at,
                failure_message: json.failure_message,
            },
        })
    }
}

impl TryFrom<FrontEndSwapJson> for FrontEndSwap {
    type Error = String;

    fn try_from(json: FrontEndSwapJson) -> Result<Self, Self::Error> {
        use alloy::primitives::Address;

        let id = parse_hex_uuid(&json.id)?;
        let invoice_id = parse_hex_uuid(&json.invoice_id)?;
        let from_amount_units: u128 = json
            .from_amount_units
            .parse()
            .map_err(|e| {
                format!(
                    "Invalid from_amount_units '{}': {e}",
                    json.from_amount_units
                )
            })?;
        let from_asset_id: Address = json
            .from_asset_id
            .parse()
            .map_err(|e| {
                format!(
                    "Invalid from_asset_id '{}': {e}",
                    json.from_asset_id
                )
            })?;

        Ok(FrontEndSwap {
            id,
            invoice_id,
            from_amount_units,
            from_chain_id: json.from_chain_id,
            from_asset_id,
            transaction_hash: json.transaction_hash,
            created_at: json.created_at,
            updated_at: json.updated_at,
        })
    }
}
