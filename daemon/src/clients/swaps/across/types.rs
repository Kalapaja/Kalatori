use chrono::DateTime;
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

use super::super::{
    ExecutorSwapStatus,
    RawSwapDetails,
};
use super::AcrossQuoteDetails;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TradeType {
    ExactInput,
    MinOutput,
    ExactOutput,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AcrossSwapStatus {
    // Deposits with this status have been filled on the destination chain
    // and the recipient should have received funds. A FilledRelay event
    // was emitted on the destination chain SpokePool.
    Filled,
    // Deposit has not been filled yet.
    Pending,
    // Deposit has expired and will not be filled. Expired deposits will be
    // refunded to the depositor on the originChainId in the next batch of
    // repayments.
    Expired,
    // Deposit has expired and the depositor has been successfully refunded
    // on the originChain.
    Refunded,
}

impl From<AcrossSwapStatus> for ExecutorSwapStatus {
    fn from(value: AcrossSwapStatus) -> Self {
        match value {
            AcrossSwapStatus::Filled => Self::Executed,
            AcrossSwapStatus::Pending => Self::Pending,
            AcrossSwapStatus::Expired => Self::Failed,
            AcrossSwapStatus::Refunded => Self::Failed,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapApprovalRequest {
    pub trade_type: TradeType,
    pub amount: u128,
    pub input_token: String,
    pub output_token: String,
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    pub depositor: String,
    pub recipient: String,
}

impl From<CreateSwapData> for SwapApprovalRequest {
    fn from(value: CreateSwapData) -> Self {
        Self {
            trade_type: TradeType::MinOutput,
            amount: value.expected_to_amount_units,
            input_token: value.from_token_address,
            output_token: value.to_token_address,
            origin_chain_id: value.from_chain.chain_id(),
            destination_chain_id: value.to_chain.chain_id(),
            depositor: value.from_address,
            recipient: value.to_address,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalTransaction {
    pub chain_id: u64,
    pub to: String,
    pub data: String,
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapTransactionInternal {
    // TODO: check if it's true? But also probably if it's false API should return us an error?
    #[expect(dead_code)]
    pub simulation_success: bool,
    pub chain_id: u64,
    pub to: String,
    pub data: String,
    #[serde(default)]
    #[serde_as(as = "DisplayFromStr")]
    pub value: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub gas: u128,
    #[serde(default)]
    #[serde_as(as = "DisplayFromStr")]
    pub max_fee_per_gas: u128,
    #[serde(default)]
    #[serde_as(as = "DisplayFromStr")]
    pub max_priority_fee_per_gas: u128,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapTransaction {
    pub chain_id: u64,
    pub contract_address: String,
    pub data: String,
    #[serde(default)]
    #[serde_as(as = "DisplayFromStr")]
    pub value: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub gas: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub max_fee_per_gas: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub max_priority_fee_per_gas: u128,
}

impl From<SwapTransactionInternal> for SwapTransaction {
    fn from(value: SwapTransactionInternal) -> Self {
        Self {
            chain_id: value.chain_id,
            contract_address: value.to,
            data: value.data,
            value: value.value,
            gas: value.gas,
            max_fee_per_gas: value.max_fee_per_gas,
            max_priority_fee_per_gas: value.max_priority_fee_per_gas,
        }
    }
}

#[expect(dead_code)]
#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapApprovalResponse {
    #[serde_as(as = "DisplayFromStr")]
    pub input_amount: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub max_input_amount: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub expected_output_amount: u128,
    #[serde(default)]
    pub approval_txns: Vec<ApprovalTransaction>,
    pub swap_tx: SwapTransactionInternal,
    pub id: String,
    pub quote_expiry_timestamp: i64,
}

impl From<SwapApprovalResponse> for SwapQuote {
    fn from(value: SwapApprovalResponse) -> Self {
        let details = AcrossQuoteDetails {
            transaction: value.swap_tx.into(),
            approval_transactions: value.approval_txns,
        };

        Self {
            swap_executor: SwapExecutorType::Across,
            id: value.id,
            estimated_to_amount_units: value.expected_output_amount,
            // TODO: in response there's output token with it's params (decimals), so we can
            // calculate it
            estimated_to_amount: Decimal::ZERO,
            // TODO: ensure unwrap is safe here?
            valid_till: DateTime::from_timestamp_secs(value.quote_expiry_timestamp).unwrap(),
            quote_details: RawSwapDetails::Across(details),
        }
    }
}

#[expect(dead_code)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapStatusRequest {
    pub deposit_txn_ref: String,
}

impl From<&str> for SwapStatusRequest {
    fn from(value: &str) -> Self {
        Self {
            deposit_txn_ref: value.to_string(),
        }
    }
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapStatusResponse {
    pub status: AcrossSwapStatus,
    pub origin_chain_id: u64,
    pub deposit_id: String,
    pub deposit_txn_ref: String,
    pub fill_txn_ref: Option<String>,
    pub destination_chain_id: u64,
    pub deposit_refund_txn_ref: Option<String>,
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcrossApiError {
    #[serde(default, rename = "type")]
    pub error_type: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub status: Option<u32>,
    pub message: String,
    #[serde(default)]
    pub id: Option<String>,
}

#[expect(dead_code)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetDepositsRequest {
    pub depositor: String,
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetDepositsResponse {
    origin_chain_id: u64,
    destination_chain_id: u64,
    depositor: String,
    recipient: String,
    // input_token: String,
    // #[serde(deserialize_with = "deserialize_string_to_u128")]
    // input_amount: u128,
    // output_token: String,
    // #[serde(deserialize_with = "deserialize_string_to_u128")]
    // output_amount: u128,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AcrossApiResponse<T> {
    Ok(T),
    Err(AcrossApiError),
}
