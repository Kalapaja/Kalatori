//! Admin-specific request types for list/filter endpoints.

use chrono::{
    DateTime,
    Utc,
};
use serde::Deserialize;
use serde_with::formats::CommaSeparator;
use serde_with::{
    StringWithSeparator,
    serde_as,
};
use uuid::Uuid;

use super::{
    ChainType,
    InvoiceStatus,
    PaginationParams,
    PayoutStatus,
    SortOrder,
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
