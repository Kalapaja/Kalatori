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

use crate::types::ChainType;

/// Transaction type (incoming or outgoing)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "sqlx", derive(sqlx::Type))]
pub enum TransactionType {
    Incoming,
    Outgoing,
}

/// Transaction status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[cfg_attr(feature = "sqlx", derive(sqlx::Type))]
pub enum TransactionStatus {
    Waiting,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub block_number: Option<u32>,
    pub position_in_block: Option<u32>,
    pub tx_hash: Option<String>,
    pub transaction_type: TransactionType,
    pub asset_name: String,
    pub asset_id: String,
    pub chain: ChainType,
    pub amount: Decimal,
    pub source_address: String,
    pub destination_address: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: TransactionStatus,
    pub transaction_link: String,
}
