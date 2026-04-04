use std::fmt;

use chrono::{
    DateTime,
    Utc,
};
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use sqlx::types::Text;
use sqlx::{
    FromRow,
    Type,
};
use uuid::Uuid;

use super::common::{
    ChainType,
    InitiatorType,
    RetryMeta,
};
use super::invoice::Invoice;
use super::swap::SwapChainType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum RefundStatus {
    Waiting,
    InProgress,
    Completed,
    FailedRetriable,
    Failed,
}

impl fmt::Display for RefundStatus {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        match self {
            Self::Waiting => write!(f, "Waiting"),
            Self::InProgress => write!(f, "InProgress"),
            Self::Completed => write!(f, "Completed"),
            Self::FailedRetriable => write!(f, "FailedRetriable"),
            Self::Failed => write!(f, "Failed"),
        }
    }
}

impl std::str::FromStr for RefundStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Waiting" => Ok(Self::Waiting),
            "InProgress" => Ok(Self::InProgress),
            "Completed" => Ok(Self::Completed),
            "FailedRetriable" => Ok(Self::FailedRetriable),
            "Failed" => Ok(Self::Failed),
            _ => Err(format!("Unknown refund status: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferDestinationParams {
    pub destination_address: String,
    pub destination_chain: SwapChainType,
    pub destination_asset_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refund {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub initiator_type: InitiatorType,
    pub initiator_id: Option<Uuid>,
    pub status: RefundStatus,
    // Source transfer info
    pub chain: ChainType,
    pub asset_id: String,
    pub asset_name: String,
    pub amount: Decimal,
    pub source_address: String,
    // Destination info. Destination might be optional on creation step,
    // it will be determined based on incoming swaps and transactions
    #[serde(flatten)]
    pub destination_params: Option<TransferDestinationParams>,
    #[serde(flatten)]
    pub retry_meta: RetryMeta,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Refund {
    pub fn from_invoice(invoice: Invoice, amount: Decimal) -> Self {
        let now = Utc::now();

        Self {
            id: Uuid::new_v4(),
            invoice_id: invoice.id,
            initiator_type: InitiatorType::System,
            initiator_id: None,
            status: RefundStatus::Waiting,
            chain: invoice.chain,
            asset_id: invoice.asset_id,
            asset_name: invoice.asset_name,
            amount,
            source_address: invoice.payment_address,
            destination_params: None,
            retry_meta: RetryMeta::default(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(FromRow)]
pub struct RefundRow {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub initiator_type: InitiatorType,
    pub initiator_id: Option<Uuid>,
    pub status: RefundStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // Source transfer info
    pub chain: ChainType,
    pub asset_id: String,
    pub asset_name: String,
    pub amount: Text<Decimal>,
    pub source_address: String,
    // Destination info
    pub destination_address: Option<String>,
    pub destination_chain: Option<SwapChainType>,
    pub destination_asset_id: Option<String>,
    #[sqlx(flatten)]
    pub retry_meta: RetryMeta,
}

impl From<RefundRow> for Refund {
    fn from(row: RefundRow) -> Self {
        let destination_params = if let (
            Some(destination_address),
            Some(destination_chain),
            Some(destination_asset_id)
        ) = (row.destination_address, row.destination_chain, row.destination_asset_id) {
            Some(TransferDestinationParams {
                destination_address,
                destination_chain,
                destination_asset_id
            })
        } else {
            None
        };

        Self {
            id: row.id,
            invoice_id: row.invoice_id,
            initiator_type: row.initiator_type,
            initiator_id: row.initiator_id,
            status: row.status,
            created_at: row.created_at,
            updated_at: row.updated_at,
            chain: row.chain,
            asset_id: row.asset_id,
            asset_name: row.asset_name,
            amount: row.amount.into_inner(),
            source_address: row.source_address,
            destination_params,
            retry_meta: row.retry_meta,
        }
    }
}

#[cfg(test)]
pub fn default_refund(invoice_id: Uuid) -> Refund {
    Refund {
        id: Uuid::new_v4(),
        invoice_id,
        chain: ChainType::Polygon,
        asset_id: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359".to_string(),
        asset_name: "USDT".to_string(),
        amount: Decimal::new(5000, 2), // 50.00
        source_address: "0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7".to_string(),
        destination_params: Some(TransferDestinationParams {
            destination_address: "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9".to_string(),
            destination_chain: SwapChainType::Polygon,
            destination_asset_id: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359".to_string(),
        }),
        initiator_type: InitiatorType::Admin,
        initiator_id: Some(Uuid::new_v4()),
        status: RefundStatus::Waiting,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        retry_meta: RetryMeta::default(),
    }
}
