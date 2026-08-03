use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use serde_with::{
    DisplayFromStr,
    serde_as,
};
use uuid::Uuid;

use crate::types::{
    ChainType,
    GeneralTransactionId,
    IncomingTransaction,
    TransferInfo,
};

#[serde_as]
#[expect(dead_code)]
#[derive(Debug, Deserialize)]
pub struct EtherscanResponseData<T> {
    #[serde_as(as = "DisplayFromStr")]
    pub status: u32,
    pub message: String,
    pub result: T,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EtherscanResponse<T> {
    Ok(EtherscanResponseData<T>),
    Err(EtherscanResponseData<String>),
}

// TODO: hide `api_key` field in logs
#[derive(Debug, Serialize)]
pub struct GetAccountTokenTransactionsParams<'a> {
    pub module: &'a str,
    pub action: &'a str,
    pub address: &'a str,
    #[serde(rename = "contractaddress")]
    pub contract_address: &'a str,
    #[serde(rename = "chainid")]
    pub chain_id: u32,
    #[serde(rename = "apikey")]
    pub api_key: &'a str,
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EtherscanTransaction {
    #[serde_as(as = "DisplayFromStr")]
    pub block_number: u32,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub time_stamp: u64,
    pub hash: String,
    // #[serde(deserialize_with = "deserialize_string_to_u32")]
    // pub nonce: u32,
    // pub block_hash: String,
    pub from: String,
    pub contract_address: String,
    pub to: String,
    #[serde_as(as = "DisplayFromStr")]
    pub value: u128,
    // pub token_name: String,
    pub token_symbol: String,
    #[serde_as(as = "DisplayFromStr")]
    pub token_decimal: u32,
    #[serde_as(as = "DisplayFromStr")]
    pub transaction_index: u32,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub gas: u64,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub gas_price: u64,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub gas_used: u64,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub cumulative_gas_used: u64,
    // #[serde(deserialize_with = "deserialize_string_to_u64")]
    // pub confirmations: u64,
}

impl EtherscanTransaction {
    /// Returns `None` (with an error log) when the reported value or token
    /// decimals don't fit into `Decimal`: recording a wrapped amount would
    /// corrupt the invoice, while skipping keeps the balance mismatch
    /// visible to the balance checker.
    pub fn into_incoming_transaction(
        self,
        invoice_id: Uuid,
    ) -> Option<IncomingTransaction> {
        let Ok(value) = i64::try_from(self.value) else {
            tracing::error!(
                tx_hash = %self.hash,
                value = self.value,
                "Transfer value exceeds the supported range, skipping transaction"
            );
            return None;
        };

        let Ok(amount) = Decimal::try_new(value, self.token_decimal) else {
            tracing::error!(
                tx_hash = %self.hash,
                token_decimal = self.token_decimal,
                "Token decimals exceed the supported range, skipping transaction"
            );
            return None;
        };

        let transfer_info = TransferInfo {
            chain: ChainType::Polygon,
            asset_id: self.contract_address,
            asset_name: self.token_symbol,
            amount,
            source_address: self.from,
            destination_address: self.to,
        };

        let transaction_id = GeneralTransactionId {
            block_number: Some(self.block_number),
            position_in_block: Some(self.transaction_index),
            tx_hash: Some(self.hash),
        };

        Some(IncomingTransaction {
            id: Uuid::new_v4(),
            invoice_id,
            transfer_info,
            transaction_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transaction(
        value: u128,
        token_decimal: u32,
    ) -> EtherscanTransaction {
        EtherscanTransaction {
            block_number: 100,
            hash: "0xdead".to_string(),
            from: "0xfrom".to_string(),
            contract_address: "0xtoken".to_string(),
            to: "0xto".to_string(),
            value,
            token_symbol: "USDC".to_string(),
            token_decimal,
            transaction_index: 2,
        }
    }

    #[test]
    fn test_into_incoming_transaction() {
        let invoice_id = Uuid::new_v4();
        let result = transaction(1_500_000, 6)
            .into_incoming_transaction(invoice_id)
            .unwrap();

        assert_eq!(
            result.transfer_info.amount,
            Decimal::new(15, 1)
        );
        assert_eq!(result.invoice_id, invoice_id);
        assert_eq!(
            result.transaction_id.tx_hash,
            Some("0xdead".to_string())
        );
    }

    #[test]
    fn test_into_incoming_transaction_rejects_oversized_value() {
        // Above i64::MAX base units: the old `as i64` cast silently wrapped
        let oversized = u128::try_from(i64::MAX).unwrap() + 1;
        assert!(
            transaction(oversized, 6)
                .into_incoming_transaction(Uuid::new_v4())
                .is_none()
        );
    }

    #[test]
    fn test_into_incoming_transaction_rejects_oversized_decimals() {
        // Decimal supports at most 28 decimal places; the old Decimal::new
        // panicked here
        assert!(
            transaction(1_000_000, 77)
                .into_incoming_transaction(Uuid::new_v4())
                .is_none()
        );
    }
}
