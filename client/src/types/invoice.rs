use chrono::{
    DateTime,
    Utc,
};
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use strum::{
    Display,
    EnumString,
};
use uuid::Uuid;

use super::{
    ChainType,
    Transaction,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "sqlx-types", derive(sqlx::Type))]
pub enum InvoiceStatus {
    // Active statuses
    Waiting,
    PartiallyPaid,
    // Final statuses
    Paid,
    OverPaid,
    // Expired statuses
    UnpaidExpired,
    PartiallyPaidExpired,
    // Canceled statuses
    CustomerCanceled,
    AdminCanceled,
}

impl InvoiceStatus {
    /// Check if invoice is in an active state (still being monitored)
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Waiting | Self::PartiallyPaid
        )
    }

    /// Check if invoice is in a final state (completed)
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Paid | Self::OverPaid)
    }

    /// Check if invoice is expired
    pub const fn is_expired(self) -> bool {
        matches!(
            self,
            Self::UnpaidExpired | Self::PartiallyPaidExpired
        )
    }

    /// Check if invoice is canceled
    pub const fn is_canceled(self) -> bool {
        matches!(
            self,
            Self::CustomerCanceled | Self::AdminCanceled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceCartItem {
    pub name: String,
    pub quantity: u32,
    pub price: Decimal, // Price per single item
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discount: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceCart {
    #[serde(default)]
    pub items: Vec<InvoiceCartItem>,
}

impl InvoiceCart {
    // Prefer to create an empty cart explicitly over using Default trait
    pub fn empty() -> Self {
        Self {
            items: vec![],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: Uuid,
    pub order_id: String,
    pub asset_name: String,
    pub asset_id: String,
    pub chain: ChainType,
    pub amount: Decimal,
    pub payment_address: String,
    pub status: InvoiceStatus,
    pub payment_url: String,
    pub redirect_url: String,
    pub cart: InvoiceCart,
    /// Opaque merchant-provided metadata, stored and echoed back verbatim
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub total_received_amount: Decimal,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transactions: Vec<Transaction>,
    pub valid_till: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
