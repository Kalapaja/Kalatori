//! Payout types for `SQLite` schema

use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use sqlx::{
    FromRow,
    Type,
};
use kalatori_client::strum::{
    Display,
    EnumString,
};
use uuid::Uuid;

use rust_decimal::Decimal;
use sqlx::types::Text;

use super::Invoice;
use super::common::{
    ChainType,
    InitiatorType,
    RetryMeta,
};
use super::refund::TransferDestinationParams;
use super::swap::SwapChainType;

/// Payout status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Display, EnumString)]
#[strum(crate = "kalatori_client::strum")]
pub enum PayoutStatus {
    Waiting,
    InProgress,
    Completed,
    FailedRetriable,
    Failed,
}

/// Payout from `SQLite`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payout {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub initiator_type: InitiatorType,
    pub initiator_id: Option<Uuid>,
    pub status: PayoutStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // Source transfer info
    pub chain: ChainType,
    pub asset_id: String,
    pub asset_name: String,
    pub amount: Decimal,
    pub source_address: String,
    // Destination info (chain/asset for cross-chain payouts via swap).
    // Defaults to Polygon — currently the only supported swap destination chain.
    #[serde(flatten)]
    pub destination_params: TransferDestinationParams,
    #[serde(flatten)]
    pub retry_meta: RetryMeta,
}

impl Payout {
    pub fn from_invoice(
        invoice: Invoice,
        destination_params: TransferDestinationParams,
        amount: Decimal,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            invoice_id: invoice.id,
            chain: invoice.chain,
            asset_id: invoice.asset_id,
            asset_name: invoice.asset_name,
            amount,
            source_address: invoice.payment_address,
            destination_params,
            initiator_type: InitiatorType::System,
            initiator_id: None,
            status: PayoutStatus::Waiting,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            retry_meta: RetryMeta::default(),
        }
    }
}

#[derive(FromRow)]
pub struct PayoutRow {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub initiator_type: InitiatorType,
    pub initiator_id: Option<Uuid>,
    pub status: PayoutStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // Source transfer info
    pub chain: ChainType,
    pub asset_id: String,
    pub asset_name: String,
    pub amount: Text<Decimal>,
    pub source_address: String,
    // Destination info
    pub destination_address: String,
    pub destination_chain: SwapChainType,
    pub destination_asset_id: String,
    #[sqlx(flatten)]
    pub retry_meta: RetryMeta,
}

impl From<PayoutRow> for Payout {
    fn from(value: PayoutRow) -> Self {
        Self {
            id: value.id,
            invoice_id: value.invoice_id,
            initiator_type: value.initiator_type,
            initiator_id: value.initiator_id,
            status: value.status,
            created_at: value.created_at,
            updated_at: value.updated_at,
            chain: value.chain,
            asset_id: value.asset_id,
            asset_name: value.asset_name,
            amount: value.amount.into_inner(),
            source_address: value.source_address,
            destination_params: TransferDestinationParams {
                destination_address: value.destination_address,
                destination_chain: value.destination_chain,
                destination_asset_id: value.destination_asset_id,
            },
            retry_meta: value.retry_meta,
        }
    }
}

#[cfg(test)]
pub fn default_payout(invoice_id: Uuid) -> Payout {
    Payout {
        id: Uuid::new_v4(),
        invoice_id,
        chain: ChainType::Polygon,
        asset_id: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359".to_string(),
        asset_name: "USDC".to_string(),
        amount: Decimal::new(1000, 2),
        source_address: "0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7".to_string(),
        // Defaults to Polygon — currently the only supported swap destination chain.
        destination_params: TransferDestinationParams {
            destination_address: "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9".to_string(),
            destination_chain: SwapChainType::Polygon,
            destination_asset_id: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359".to_string(),
        },
        initiator_type: InitiatorType::System,
        initiator_id: None,
        status: PayoutStatus::Waiting,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        retry_meta: RetryMeta::default(),
    }
}
