//! Transaction types for `SQLite` schema

use chrono::{
    DateTime,
    Utc,
};
use serde::{
    Deserialize,
    Serialize,
};
use sqlx::FromRow;
use sqlx::types::Json;
use uuid::Uuid;

use crate::chain_client::GeneralChainTransfer;

pub use kalatori_client::types::{
    Transaction as PublicTransaction,
    TransactionStatus,
    TransactionType,
};

use super::common::{
    ChainType,
    TransferInfo,
    TransferInfoRow,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct GeneralTransactionId {
    pub block_number: Option<u32>,
    pub position_in_block: Option<u32>,
    pub tx_hash: Option<String>,
}

impl GeneralTransactionId {
    pub fn empty() -> Self {
        Self {
            block_number: None,
            position_in_block: None,
            tx_hash: None,
        }
    }
}

/// Origin field for transactions (what triggered this transaction)
#[expect(clippy::struct_field_names)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionOrigin {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refund_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payout_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_transfer_id: Option<Uuid>,
}

pub enum TransactionOriginVariant {
    Payout(Uuid),
    Refund(Uuid),
    InternalTransfer(Uuid),
    None,
}

impl TransactionOrigin {
    pub fn payout(payout_id: Uuid) -> Self {
        Self {
            payout_id: Some(payout_id),
            ..Default::default()
        }
    }

    pub fn refund(refund_id: Uuid) -> Self {
        Self {
            refund_id: Some(refund_id),
            ..Default::default()
        }
    }

    pub fn internal_transfer(internal_transfer_id: Uuid) -> Self {
        Self {
            internal_transfer_id: Some(internal_transfer_id),
            ..Default::default()
        }
    }

    pub fn variant(&self) -> TransactionOriginVariant {
        if let Some(payout_id) = self.payout_id {
            TransactionOriginVariant::Payout(payout_id)
        } else if let Some(refund_id) = self.refund_id {
            TransactionOriginVariant::Refund(refund_id)
        } else if let Some(internal_transfer_id) = self.internal_transfer_id {
            TransactionOriginVariant::InternalTransfer(internal_transfer_id)
        } else {
            TransactionOriginVariant::None
        }
    }
}

/// Metadata for outgoing transactions
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct OutgoingTransactionMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extrinsic_bytes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
}

/// Transaction from `SQLite`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct Transaction {
    pub id: Uuid,
    pub invoice_id: Uuid,
    // TODO: move TransferInfo to `client`?
    #[serde(flatten)]
    pub transfer_info: TransferInfo,
    #[expect(clippy::struct_field_names)]
    #[serde(flatten)]
    pub transaction_id: GeneralTransactionId,
    pub origin: TransactionOrigin,
    pub status: TransactionStatus,
    #[expect(clippy::struct_field_names)]
    pub transaction_type: TransactionType,
    pub outgoing_meta: OutgoingTransactionMeta,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Transaction {
    pub fn is_incoming(&self) -> bool {
        self.transaction_type == TransactionType::Incoming
    }
}

impl From<Transaction> for PublicTransaction {
    fn from(value: Transaction) -> Self {
        let transaction_link = match value.transfer_info.chain {
            ChainType::PolkadotAssetHub => format!(
                // For now we don't expect errors here cause we will make into public only
                // Incoming transaction. Although it will be better to refactor it later and
                // return an error if we trying to turn into PublicTransaction not finished
                // Outgoing transaction
                "https://assethub-polkadot.subscan.io/extrinsic/{}-{}",
                value
                    .transaction_id
                    .block_number
                    .unwrap_or_default(),
                value
                    .transaction_id
                    .position_in_block
                    .unwrap_or_default(),
            ),
            ChainType::Polygon => {
                // Use PolygonScan for Polygon transactions
                // If we have tx_hash, use that; otherwise use block/tx_index
                if let Some(ref tx_hash) = value.transaction_id.tx_hash {
                    format!("https://polygonscan.com/tx/{}", tx_hash)
                } else {
                    format!(
                        "https://polygonscan.com/block/{}/txs#tx-{}",
                        value
                            .transaction_id
                            .block_number
                            .unwrap_or_default(),
                        value
                            .transaction_id
                            .position_in_block
                            .unwrap_or_default(),
                    )
                }
            },
        };

        PublicTransaction {
            id: value.id,
            invoice_id: value.invoice_id,
            block_number: value.transaction_id.block_number,
            position_in_block: value.transaction_id.position_in_block,
            tx_hash: value.transaction_id.tx_hash,
            transaction_type: value.transaction_type,
            asset_name: value.transfer_info.asset_name,
            asset_id: value.transfer_info.asset_id,
            chain: value.transfer_info.chain,
            amount: value.transfer_info.amount,
            source_address: value.transfer_info.source_address,
            destination_address: value.transfer_info.destination_address,
            created_at: value.created_at,
            updated_at: value.updated_at,
            status: value.status,
            transaction_link,
        }
    }
}

#[derive(FromRow)]
pub struct TransactionRow {
    pub id: Uuid,
    pub invoice_id: Uuid,
    #[sqlx(flatten)]
    pub transfer_info: TransferInfoRow,
    #[sqlx(flatten)]
    pub transaction_id: GeneralTransactionId,
    pub origin: Json<TransactionOrigin>,
    pub status: TransactionStatus,
    pub transaction_type: TransactionType,
    pub outgoing_meta: Json<OutgoingTransactionMeta>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<TransactionRow> for Transaction {
    fn from(row: TransactionRow) -> Self {
        Self {
            id: row.id,
            invoice_id: row.invoice_id,
            transfer_info: row.transfer_info.into(),
            transaction_id: row.transaction_id,
            origin: row.origin.0,
            status: row.status,
            transaction_type: row.transaction_type,
            outgoing_meta: row.outgoing_meta.0,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[cfg(test)]
pub fn default_transaction(invoice_id: Uuid) -> Transaction {
    let transfer_info = TransferInfo {
        asset_id: 1984.to_string(),
        asset_name: "USDT".to_string(),
        chain: ChainType::PolkadotAssetHub,
        amount: rust_decimal::Decimal::new(10000, 2),
        source_address: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
        destination_address: "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string(),
    };

    let transaction_id = GeneralTransactionId {
        block_number: Some(1000),
        position_in_block: Some(2),
        tx_hash: Some("0x1234567890abcdef".to_string()),
    };

    let now = Utc::now();

    Transaction {
        id: Uuid::new_v4(),
        invoice_id,
        transfer_info,
        transaction_id,
        origin: TransactionOrigin::default(),
        status: TransactionStatus::Waiting,
        transaction_type: TransactionType::Incoming,
        outgoing_meta: OutgoingTransactionMeta::default(),
        created_at: now,
        updated_at: now,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingTransaction {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub transfer_info: TransferInfo,
    pub tx_hash: String,
    pub transaction_bytes: String,
    pub origin: TransactionOrigin,
}

impl From<OutgoingTransaction> for Transaction {
    fn from(value: OutgoingTransaction) -> Self {
        let now = Utc::now();

        let transaction_id = GeneralTransactionId {
            block_number: None,
            position_in_block: None,
            tx_hash: Some(value.tx_hash),
        };

        Self {
            id: value.id,
            invoice_id: value.invoice_id,
            transfer_info: value.transfer_info,
            transaction_id,
            origin: value.origin,
            status: TransactionStatus::InProgress,
            transaction_type: TransactionType::Outgoing,
            outgoing_meta: OutgoingTransactionMeta {
                extrinsic_bytes: Some(value.transaction_bytes),
                built_at: Some(now),
                sent_at: None,
                confirmed_at: None,
                failed_at: None,
                failure_message: None,
            },
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingTransaction {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub transfer_info: TransferInfo,
    pub transaction_id: GeneralTransactionId,
}

impl IncomingTransaction {
    pub fn from_chain_transfer(
        invoice_id: Uuid,
        transfer: GeneralChainTransfer,
    ) -> Self {
        let id = transfer.id;
        let transaction_id = transfer.general_transaction_id();
        let transfer_info = transfer.into_transfer_info();

        Self {
            id,
            invoice_id,
            transfer_info,
            transaction_id,
        }
    }
}

impl From<IncomingTransaction> for Transaction {
    fn from(value: IncomingTransaction) -> Self {
        let now = Utc::now();

        Self {
            id: value.id,
            invoice_id: value.invoice_id,
            transfer_info: value.transfer_info,
            transaction_id: value.transaction_id,
            origin: TransactionOrigin::default(),
            status: TransactionStatus::Completed,
            transaction_type: TransactionType::Incoming,
            outgoing_meta: OutgoingTransactionMeta::default(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg(test)]
pub fn default_incoming_transaction(invoice_id: Uuid) -> IncomingTransaction {
    let transfer_info = TransferInfo {
        asset_id: 1984.to_string(),
        asset_name: "USDT".to_string(),
        chain: ChainType::PolkadotAssetHub,
        amount: rust_decimal::Decimal::new(10000, 2),
        source_address: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
        destination_address: "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string(),
    };

    let transaction_id = GeneralTransactionId {
        block_number: Some(1000),
        position_in_block: Some(2),
        tx_hash: Some("0x1234567890abcdef".to_string()),
    };

    IncomingTransaction {
        id: Uuid::new_v4(),
        invoice_id,
        transfer_info,
        transaction_id,
    }
}
