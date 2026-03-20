//! Admin-specific request types for list/filter endpoints.

use std::collections::HashMap;

use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use serde_with::formats::CommaSeparator;
use serde_with::{
    StringWithSeparator,
    serde_as,
};
use uuid::Uuid;

use crate::configs::SlippageParams;

use super::{
    ChainType,
    InvoiceStatus,
    PaginationParams,
    PayoutStatus,
    SortOrder,
    SwapExecutorType,
    SwapStatus,
    TransactionStatus,
    TransactionType,
};

/// Query parameters for `GET /admin/invoices`.
#[serde_as]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListInvoicesParams {
    #[serde(flatten)]
    pub pagination: PaginationParams,

    /// Comma-separated list of statuses to filter by (e.g. `Waiting,Paid`).
    #[serde_as(as = "Option<StringWithSeparator::<CommaSeparator, InvoiceStatus>>")]
    pub status: Option<Vec<InvoiceStatus>>,

    /// Filter by chain type.
    pub chain: Option<ChainType>,

    /// Filter by asset ID.
    pub asset_id: Option<String>,

    /// Filter by order ID (substring match).
    pub order_id: Option<String>,

    /// Filter invoices created on or after this timestamp.
    pub created_from: Option<DateTime<Utc>>,

    /// Filter invoices created on or before this timestamp.
    pub created_to: Option<DateTime<Utc>>,

    /// Sort direction for `created_at` (default: `desc`).
    pub sort_order: Option<SortOrder>,
}

/// Query parameters for `GET /admin/payouts`.
#[serde_as]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListPayoutsParams {
    #[serde(flatten)]
    pub pagination: PaginationParams,

    /// Comma-separated list of statuses to filter by (e.g.
    /// `Waiting,Completed`).
    #[serde_as(as = "Option<StringWithSeparator::<CommaSeparator, PayoutStatus>>")]
    pub status: Option<Vec<PayoutStatus>>,

    /// Filter by chain type.
    pub chain: Option<ChainType>,

    /// Filter by asset ID.
    pub asset_id: Option<String>,

    /// Filter by parent invoice ID.
    pub invoice_id: Option<Uuid>,

    /// Filter payouts created on or after this timestamp.
    pub created_from: Option<DateTime<Utc>>,

    /// Filter payouts created on or before this timestamp.
    pub created_to: Option<DateTime<Utc>>,

    /// Sort direction for `created_at` (default: `desc`).
    pub sort_order: Option<SortOrder>,
}

/// Query parameters for `GET /admin/swaps`.
#[serde_as]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListSwapsParams {
    #[serde(flatten)]
    pub pagination: PaginationParams,

    /// Comma-separated list of statuses to filter by (e.g.
    /// `Created,Completed`).
    #[serde_as(as = "Option<StringWithSeparator::<CommaSeparator, SwapStatus>>")]
    pub status: Option<Vec<SwapStatus>>,

    /// Filter by swap executor type (`Across` or `Bungee`).
    pub swap_executor: Option<SwapExecutorType>,

    /// Filter by parent invoice ID.
    pub invoice_id: Option<Uuid>,

    /// Filter swaps created on or after this timestamp.
    pub created_from: Option<DateTime<Utc>>,

    /// Filter swaps created on or before this timestamp.
    pub created_to: Option<DateTime<Utc>>,

    /// Sort direction for `created_at` (default: `desc`).
    pub sort_order: Option<SortOrder>,
}

/// Query parameters for `GET /admin/transactions`.
#[serde_as]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListTransactionsParams {
    #[serde(flatten)]
    pub pagination: PaginationParams,

    /// Comma-separated list of statuses to filter by (e.g.
    /// `Waiting,Completed`).
    #[serde_as(as = "Option<StringWithSeparator::<CommaSeparator, TransactionStatus>>")]
    pub status: Option<Vec<TransactionStatus>>,

    /// Filter by transaction type (`Incoming` or `Outgoing`).
    pub transaction_type: Option<TransactionType>,

    /// Filter by chain type.
    pub chain: Option<ChainType>,

    /// Filter by asset ID.
    pub asset_id: Option<String>,

    /// Filter by parent invoice ID.
    pub invoice_id: Option<Uuid>,

    /// Filter transactions created on or after this timestamp.
    pub created_from: Option<DateTime<Utc>>,

    /// Filter transactions created on or before this timestamp.
    pub created_to: Option<DateTime<Utc>>,

    /// Sort direction for `created_at` (default: `desc`).
    pub sort_order: Option<SortOrder>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicAssetDescription {
    pub asset_id: String,
    pub asset_name: String,
    // TODO: add asset decimals and specify chain
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KalatoriSettings {
    pub shop_url: String,
    pub shop_name: String,
    pub logo_url: Option<String>,
    pub recipient_addresses: HashMap<ChainType, String>,
    pub invoice_lifetime_millis: u64,
    pub default_chain: ChainType,
    pub default_asset_id: HashMap<ChainType, String>,
    pub payment_url_base: String,
    pub slippage_params: HashMap<ChainType, HashMap<String, SlippageParams>>,
    pub assets_description: HashMap<String, PublicAssetDescription>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KalatoriIntegrationSettings {
    pub invoices_webhook_url: Option<String>,
    pub signature_max_age_secs: u64,
    pub private_api_base_url: String,
    pub api_secret_key: String,
}
