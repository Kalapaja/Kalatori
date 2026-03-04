use serde::{Serialize, Deserialize, Deserializer};

use crate::types::CreateSwapData;

fn deserialize_string_to_u128<'de, D>(deserializer: D) -> Result<u128, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    s.parse()
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TradeType {
    ExactInput,
    MinOutput,
    ExactOutput,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapTransaction {
    pub simulation_success: bool,
    pub chain_id: u64,
    pub to: String,
    pub data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapApprovalResponse {
    #[serde(deserialize_with = "deserialize_string_to_u128")]
    pub input_amount: u128,
    #[serde(deserialize_with = "deserialize_string_to_u128")]
    pub max_input_amount: u128,
    #[serde(default)]
    pub approval_txns: Vec<ApprovalTransaction>,
    pub swap_tx: SwapTransaction,
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcrossApiError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: String,
    pub status: u32,
    pub message: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AcrossApiResponse<T> {
    Ok(T),
    Err(AcrossApiError),
}
