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
use super::{ZeroExQuoteDetails, ZeroExGaslessQuoteDetails, ExecutorSwapStatus};

#[serde_as]
#[derive(Debug, Clone, Serialize)]
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

        let buy_token = if value.to_token_address == "0x0000000000000000000000000000000000000000"
        {
            "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE".to_string()
        } else {
            value.to_token_address
        };

        Self {
            chain_id: value.from_chain.chain_id(),
            buy_token,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroExTrade {
    #[serde(rename = "type")]
    pub trade_type: String,
    pub hash: String,
    pub eip712: serde_json::Value,
}

#[expect(dead_code)]
#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroExGaslessGetQuoteResponse {
    pub allowance_target: String,
    #[serde(default)]
    pub approval: Option<ZeroExTrade>,
    #[serde_as(as = "DisplayFromStr")]
    pub buy_amount: u128,
    pub buy_token: String,
    #[serde_as(as = "DisplayFromStr")]
    pub min_buy_amount: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub sell_amount: u128,
    pub sell_token: String,
    pub trade: ZeroExTrade,
    pub zid: String,
}

impl From<ZeroExGaslessGetQuoteResponse> for SwapQuote {
    fn from(value: ZeroExGaslessGetQuoteResponse) -> Self {
        let details = ZeroExGaslessQuoteDetails {
            raw_trade: value.trade,
            approval: value.approval,
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
            quote_details: RawSwapDetails::ZeroExGasless(details),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypedSignature {
    pub signature_type: u8,
    pub signature_bytes: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedTrade {
    #[serde(rename = "type")]
    pub trade_type: String,
    pub eip712: serde_json::Value,
    pub signature: TypedSignature,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitTransactionRequest {
    pub chain_id: u64,
    pub trade: SignedTrade,
    pub approval: Option<SignedTrade>,
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitTransactionResponse {
    pub trade_hash: String,
    #[serde(rename = "type")]
    pub trade_type: String,
    pub zid: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactionStatusRequest {
    pub chain_id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZeroExGaslessTransactionStatus {
    Pending,
    Submitted,
    Succeeded,
    Confirmed,
}

impl From<ZeroExGaslessTransactionStatus> for ExecutorSwapStatus {
    fn from(value: ZeroExGaslessTransactionStatus) -> Self {
        use ZeroExGaslessTransactionStatus::*;

        match value {
            Pending => ExecutorSwapStatus::Pending,
            Submitted => ExecutorSwapStatus::Pending,
            Succeeded => ExecutorSwapStatus::Pending,
            Confirmed => ExecutorSwapStatus::Executed,
        }
    }
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactionStatusResponse {
    pub status: ZeroExGaslessTransactionStatus,
    pub zid: String,
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroExFee {
    pub token: String,
    #[serde_as(as = "DisplayFromStr")]
    pub amount: u128,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroExFeesTable {
    // #[serde(default)]
    // pub integrator_fees: Vec<ZeroExFee>,
    pub zero_ex_fee: ZeroExFee,
    pub gas_fee: ZeroExFee,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroExSwapPrice {
    pub fees: ZeroExFeesTable,
}

impl ZeroExSwapPrice {
    // TODO: it'll be best to check that all fees are in the same asset.
    // For gasless API it shouldn't be a problem but if it will be used in common
    // it might have fees in different assets
    pub fn total_fees(&self) -> u128 {
        // self.fees.integrator_fees
        //     .iter()
        //     .map(|fee| fee.amount)
        //     .sum::<u128>()
        self.fees.zero_ex_fee.amount
            + self.fees.gas_fee.amount
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
