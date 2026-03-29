use chrono::{
    TimeDelta,
    Utc,
};
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use serde_with::{
    DisplayFromStr,
    serde_as,
};

use crate::types::{
    CreateSwapData,
    SwapExecutorType,
    SwapQuote,
};

use super::super::RawSwapDetails;
use super::ZeroExQuoteDetails;

#[serde_as]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroExGetQuoteRequest {
    pub chain_id: u64,
    pub buy_token: String,
    pub sell_token: String,
    #[serde_as(as = "DisplayFromStr")]
    pub sell_amount: u128,
    pub taker: String,
    pub recipient: String,
}

// TODO: we probably should use `TryFrom` here and return an error
// if `from_chain != to_chain` or validate it in some other level
impl From<CreateSwapData> for ZeroExGetQuoteRequest {
    fn from(value: CreateSwapData) -> Self {
        // TODO: move to consts? Ideally will be have some wrapper around the value and
        // detect such "native assets" by method and validate address
        let sell_token = if value.from_token_address == "0x0000000000000000000000000000000000000000"
        {
            "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE".to_string()
        } else {
            value.from_token_address
        };

        Self {
            chain_id: value.from_chain.chain_id(),
            buy_token: value.to_token_address,
            sell_token,
            sell_amount: value.from_amount_units,
            taker: value.from_address,
            recipient: value.to_address,
        }
    }
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroExTransaction {
    pub to: String,
    pub data: String,
    #[serde_as(as = "DisplayFromStr")]
    pub gas: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub gas_price: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub value: u128,
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ZeroExPermit2 {
    pub hash: String,
    pub eip712: serde_json::Value,
}

#[expect(dead_code)]
#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroExGetQuoteResponse {
    pub allowance_target: String,
    #[serde_as(as = "DisplayFromStr")]
    pub buy_amount: u128,
    pub buy_token: String,
    #[serde_as(as = "DisplayFromStr")]
    pub min_buy_amount: u128,
    // pub permit2: ZeroExPermit2,
    #[serde_as(as = "DisplayFromStr")]
    pub sell_amount: u128,
    pub sell_token: String,
    pub transaction: ZeroExTransaction,
    pub zid: String,
}

impl From<ZeroExGetQuoteResponse> for SwapQuote {
    fn from(value: ZeroExGetQuoteResponse) -> Self {
        let details = ZeroExQuoteDetails {
            allowance_target: value.allowance_target,
            // permit_hash: value.permit2.hash,
            // permit_data: value.permit2.eip712,
            raw_transaction: value.transaction.into(),
        };

        Self {
            // ZeroEx doesn't have swap id specifically, use request id
            id: value.zid,
            swap_executor: SwapExecutorType::ZeroEx,
            estimated_to_amount_units: value.buy_amount,
            // TODO: it's not returned in response, also no data about asset precision
            // leave placeholder for now but in future probably it'll be better
            // to make this field optional
            estimated_to_amount: Decimal::ZERO,
            valid_till: Utc::now() + TimeDelta::minutes(5),
            quote_details: RawSwapDetails::ZeroEx(details),
        }
    }
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ZeroExErrorResponseData {
    pub zid: String,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ZeroExErrorResponse {
    pub name: String,
    pub message: String,
    #[expect(dead_code)]
    pub data: ZeroExErrorResponseData,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ZeroExResponse<T> {
    Ok(T),
    Err(ZeroExErrorResponse),
}
