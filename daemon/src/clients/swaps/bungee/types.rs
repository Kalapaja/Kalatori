use alloy::dyn_abi::Eip712Domain;
use chrono::DateTime;
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use serde_with::{
    DisplayFromStr,
    TryFromInto,
    serde_as,
};

use crate::clients::swaps::SwapsClientError;
use crate::types::{
    CreateSwapData,
    SwapDetails,
    SwapExecutorType,
    SwapQuote,
};

use super::BungeeQuoteDetails;

use super::super::{
    ExecutorSwapStatus,
    RawSwapDetails,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    pub user_address: String,
    pub receiver_address: String,
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    pub input_token: String,
    pub output_token: String,
    pub input_amount: String,
}

impl From<CreateSwapData> for QuoteRequest {
    fn from(value: CreateSwapData) -> Self {
        Self {
            input_amount: value.from_amount_units.to_string(),
            input_token: value.from_token_address,
            output_token: value.to_token_address,
            origin_chain_id: value.from_chain.chain_id(),
            destination_chain_id: value.to_chain.chain_id(),
            user_address: value.from_address,
            receiver_address: value.to_address,
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BasicRequest {
    pub bungee_gateway: String,
    // TODO: as long as we use it only for incoming swaps
    // on the same chain it's fine, but actually it can return either
    // `chain_id` or `originChainId` + `destinationChainId` for cross swaps
    pub chain_id: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub deadline: i64,
    #[serde_as(as = "DisplayFromStr")]
    pub input_amount: u128,
    pub input_token: String,
    #[serde_as(as = "DisplayFromStr")]
    pub min_output_amount: u128,
    #[serde_as(as = "DisplayFromStr")]
    pub nonce: u64,
    pub output_token: String,
    pub receiver: String,
    pub sender: String,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Witness {
    pub affiliate_fees: String,
    pub basic_req: BasicRequest,
    pub destination_payload: String,
    pub exclusive_transmitter: String,
    pub metadata: String,
    #[serde_as(as = "DisplayFromStr")]
    pub min_dest_gas: u128,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permitted {
    #[serde_as(as = "DisplayFromStr")]
    pub amount: u128,
    pub token: String,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignQuoteDataValues {
    #[serde_as(as = "DisplayFromStr")]
    pub deadline: i64,
    #[serde_as(as = "DisplayFromStr")]
    pub nonce: u64,
    pub permitted: Permitted,
    pub spender: String,
    pub witness: Witness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignTypedData {
    pub domain: Eip712Domain,
    pub types: serde_json::Value,
    pub values: SignQuoteDataValues,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalData {
    pub token_address: String,
    pub spender_address: String,
    pub user_address: String,
    pub amount: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteAutoRoute {
    pub quote_id: String,
    pub request_type: String,
    pub sign_typed_data: SignTypedData,
    pub approval_data: ApprovalData,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteResponse {
    pub auto_route: QuoteAutoRoute,
}

impl From<QuoteResponse> for SwapQuote {
    fn from(value: QuoteResponse) -> Self {
        let route = value.auto_route;

        let valid_till =
            DateTime::from_timestamp_secs(route.sign_typed_data.values.deadline).unwrap();
        let estimated_to_amount_units = route
            .sign_typed_data
            .values
            .witness
            .basic_req
            .min_output_amount;

        let details = BungeeQuoteDetails {
            quote_id: route.quote_id.clone(),
            request_type: route.request_type,
            approval_data: route.approval_data,
            sign_typed_data: route.sign_typed_data,
        };

        Self {
            swap_executor: SwapExecutorType::Bungee,
            id: route.quote_id,
            estimated_to_amount_units,
            // TODO: in response there's output token with it's params (decimals), so we can
            // calculate it
            estimated_to_amount: Decimal::ZERO,
            // TODO: ensure unwrap is safe here?
            valid_till,
            quote_details: RawSwapDetails::Bungee(details),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitOrderRequest {
    pub request_type: String,
    pub request: Witness,
    pub user_signature: String,
    pub quote_id: String,
}

impl TryFrom<SwapDetails> for SubmitOrderRequest {
    type Error = SwapsClientError;

    fn try_from(value: SwapDetails) -> Result<Self, Self::Error> {
        let RawSwapDetails::Bungee(raw_transaction) = value.raw_transaction else {
            return Err(SwapsClientError::WrongRawTransaction)
        };

        let signature = value
            .signature
            .ok_or(SwapsClientError::SignatureIsNotSet)?;

        Ok(Self {
            request_type: raw_transaction.request_type,
            request: raw_transaction
                .sign_typed_data
                .values
                .witness,
            // TODO: check if it's safe, at least add tests for that
            user_signature: signature,
            quote_id: value.id,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitOrderResponse {
    pub request_hash: String,
}

pub type GetSwapStatusRequest = SubmitOrderResponse;

impl From<&str> for GetSwapStatusRequest {
    fn from(value: &str) -> Self {
        Self {
            request_hash: value.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum BungeeSwapStatus {
    Pending,
    Assigned,
    Extracted,
    Fulfilled,
    Settled,
    Expired,
    Cancelled,
    Refunded,
}

impl TryFrom<u8> for BungeeSwapStatus {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Pending),
            1 => Ok(Self::Assigned),
            2 => Ok(Self::Extracted),
            3 => Ok(Self::Fulfilled),
            4 => Ok(Self::Settled),
            5 => Ok(Self::Expired),
            6 => Ok(Self::Cancelled),
            7 => Ok(Self::Refunded),
            _ => Err(format!("Invalid status code: {value}")),
        }
    }
}

impl From<BungeeSwapStatus> for ExecutorSwapStatus {
    fn from(value: BungeeSwapStatus) -> Self {
        match value {
            BungeeSwapStatus::Pending
            | BungeeSwapStatus::Assigned
            | BungeeSwapStatus::Extracted => Self::Pending,
            BungeeSwapStatus::Settled | BungeeSwapStatus::Fulfilled => Self::Executed,
            BungeeSwapStatus::Refunded
            | BungeeSwapStatus::Expired
            | BungeeSwapStatus::Cancelled => Self::Failed,
        }
    }
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetSwapStatusResponse {
    #[serde_as(as = "TryFromInto<u8>")]
    pub bungee_status_code: BungeeSwapStatus,
}
