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
    #[expect(clippy::cast_possible_truncation)]
    pub fn into_incoming_transaction(
        self,
        invoice_id: Uuid,
    ) -> IncomingTransaction {
        let transfer_info = TransferInfo {
            chain: ChainType::Polygon,
            asset_id: self.contract_address,
            asset_name: self.token_symbol,
            amount: Decimal::new(self.value as i64, self.token_decimal),
            source_address: self.from,
            destination_address: self.to,
        };

        let transaction_id = GeneralTransactionId {
            block_number: Some(self.block_number),
            position_in_block: Some(self.transaction_index),
            tx_hash: Some(self.hash),
        };

        IncomingTransaction {
            id: Uuid::new_v4(),
            invoice_id,
            transfer_info,
            transaction_id,
        }
    }
}
