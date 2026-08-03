use alloy::dyn_abi::Eip712Domain;
use chrono::DateTime;
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use serde_with::{
    DisplayFromStr,
    PickFirst,
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
use crate::utils::logging::{
    category,
    operation,
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

/// Chain reference inside a Bungee `basicReq`: single-chain quotes carry
/// `chainId` while cross-chain quotes carry `originChainId` +
/// `destinationChainId` instead. Untagged + flattened so both shapes
/// round-trip with their exact field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BungeeRequestChains {
    SingleChain {
        #[serde(rename = "chainId")]
        chain_id: u64,
    },
    CrossChain {
        #[serde(rename = "originChainId")]
        origin_chain_id: u64,
        #[serde(rename = "destinationChainId")]
        destination_chain_id: u64,
    },
}

// Bungee encodes numbers as strings in single-chain responses but as plain
// JSON numbers in cross-chain ones (e.g. `deadline`). `PickFirst` accepts
// both and serializes back as strings — matching the single-chain flow, the
// only one the daemon re-submits today.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BasicRequest {
    pub bungee_gateway: String,
    #[serde(flatten)]
    pub chains: BungeeRequestChains,
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
    pub deadline: i64,
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
    pub input_amount: u128,
    pub input_token: String,
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
    pub min_output_amount: u128,
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
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
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
    pub min_dest_gas: u128,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permitted {
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
    pub amount: u128,
    pub token: String,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignQuoteDataValues {
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
    pub deadline: i64,
    #[serde_as(as = "PickFirst<(DisplayFromStr, _)>")]
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
    // `nullable: true` in Bungee's OpenAPI spec, and null in its own
    // `onchainExample` for this endpoint: Bungee returns an on-chain auto route
    // (`userOp: "tx"`, populated `txData`) instead of a Permit2 one. This
    // integration can only execute the Permit2 shape, so a null here is "no
    // usable route", not a parse failure — a required field made it one.
    //
    // NB: `#[serde(default)]` on `auto_route` below does not cover this. A
    // default fires only for an absent key, never for a present-but-null value.
    #[serde(default)]
    pub sign_typed_data: Option<SignTypedData>,
    #[serde(default)]
    pub approval_data: Option<ApprovalData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteResponse {
    #[serde(default)]
    pub auto_route: Option<QuoteAutoRoute>,
}

impl TryFrom<QuoteAutoRoute> for SwapQuote {
    type Error = SwapsClientError;

    fn try_from(route: QuoteAutoRoute) -> Result<Self, Self::Error> {
        let Some(sign_typed_data) = route.sign_typed_data else {
            tracing::debug!(
                error.category = category::SWAPS_CLIENT,
                error.operation = operation::GET_QUOTE,
                request_type = %route.request_type,
                "Bungee returned an on-chain auto route with no signTypedData"
            );

            return Err(SwapsClientError::NoRouteAvailable)
        };

        // Provider-controlled deadline: any `i64` deserializes, but only a
        // subset is representable, and `None` here used to panic the daemon.
        let valid_till = DateTime::from_timestamp_secs(sign_typed_data.values.deadline)
            .ok_or_else(|| {
                tracing::warn!(
                    error.category = category::SWAPS_CLIENT,
                    error.operation = operation::GET_QUOTE,
                    deadline = sign_typed_data.values.deadline,
                    "Bungee quote deadline is not a representable timestamp, rejecting"
                );

                SwapsClientError::UnusableQuote
            })?;

        let estimated_to_amount_units = sign_typed_data
            .values
            .witness
            .basic_req
            .min_output_amount;

        let details = BungeeQuoteDetails {
            quote_id: route.quote_id.clone(),
            request_type: route.request_type,
            approval_data: route.approval_data,
            sign_typed_data,
        };

        Ok(Self {
            swap_executor: SwapExecutorType::Bungee,
            id: route.quote_id,
            estimated_to_amount_units,
            // TODO: in response there's output token with it's params (decimals), so we can
            // calculate it
            estimated_to_amount: Decimal::ZERO,
            valid_till,
            quote_details: RawSwapDetails::Bungee(details),
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_request_single_chain_roundtrip() {
        let json = serde_json::json!({
            "bungeeGateway": "0x3a23F943181408EAC424116Af7b7790c94Cb97a5",
            "chainId": 137,
            "deadline": "1770679043",
            "inputAmount": "1000000",
            "inputToken": "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
            "minOutputAmount": "990000",
            "nonce": "42",
            "outputToken": "0xc2132D05D31c914a87C6611C10748AEb04B58e8F",
            "receiver": "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9",
            "sender": "0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7"
        });

        let request: BasicRequest = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(
            request.chains,
            BungeeRequestChains::SingleChain {
                chain_id: 137
            }
        );
        assert_eq!(request.deadline, 1_770_679_043);
        assert_eq!(request.input_amount, 1_000_000);

        // Re-serialization (used when submitting the signed witness back to
        // Bungee) must reproduce the exact single-chain shape
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json
        );
    }

    #[test]
    fn test_basic_request_cross_chain_deserializes() {
        // Cross-chain quotes replace `chainId` with origin/destination and
        // encode `deadline` as a plain number
        let json = serde_json::json!({
            "bungeeGateway": "0x3a23F943181408EAC424116Af7b7790c94Cb97a5",
            "originChainId": 8453,
            "destinationChainId": 137,
            "deadline": 1770679043,
            "inputAmount": "1000000",
            "inputToken": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "minOutputAmount": "990000",
            "nonce": "42",
            "outputToken": "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
            "receiver": "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9",
            "sender": "0x45f077823C8d036a1a9f7Cd28e86Bd98191dF2b7"
        });

        let request: BasicRequest = serde_json::from_value(json).unwrap();
        assert_eq!(
            request.chains,
            BungeeRequestChains::CrossChain {
                origin_chain_id: 8453,
                destination_chain_id: 137,
            }
        );
        assert_eq!(request.deadline, 1_770_679_043);
    }
}
