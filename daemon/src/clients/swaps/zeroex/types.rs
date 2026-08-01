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

use super::super::{
    RawSwapDetails,
    SwapsClientError,
};
use super::{
    ExecutorSwapStatus,
    ZeroExGaslessQuoteDetails,
    ZeroExQuoteDetails,
};

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

        let buy_token = if value.to_token_address == "0x0000000000000000000000000000000000000000" {
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
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub gas: Option<u64>,
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
    #[serde(default)]
    pub allowance_target: Option<String>,
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

impl TryFrom<ZeroExGetQuoteResponse> for SwapQuote {
    type Error = SwapsClientError;

    // Infallible today — an absent gas estimate is passed through rather than
    // rejected — but the trait requires the fallible shape.
    fn try_from(value: ZeroExGetQuoteResponse) -> Result<Self, Self::Error> {
        let details = ZeroExQuoteDetails {
            allowance_target: value.allowance_target,
            // permit_hash: value.permit2.hash,
            // permit_data: value.permit2.eip712,
            raw_transaction: value.transaction.into(),
        };

        Ok(Self {
            // ZeroEx doesn't have swap id specifically, use request id
            id: value.zid,
            swap_executor: SwapExecutorType::ZeroEx,
            estimated_to_amount_units: value.buy_amount,
            // TODO: it's not returned in response, also no data about asset precision
            // leave placeholder for now but in future probably it'll be better
            // to make this field optional
            estimated_to_amount: Decimal::ZERO,
            #[expect(
                clippy::arithmetic_side_effects,
                reason = "`Utc::now()` plus a fixed 5 minutes cannot approach DateTime<Utc>'s bounds"
            )]
            valid_till: Utc::now() + TimeDelta::minutes(5),
            quote_details: RawSwapDetails::ZeroEx(details),
        })
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
    #[serde(default)]
    pub allowance_target: Option<String>,
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

impl TryFrom<ZeroExGaslessGetQuoteResponse> for SwapQuote {
    type Error = SwapsClientError;

    // Infallible today — the gasless quote carries no gas parameters — but the
    // trait requires the fallible shape.
    fn try_from(value: ZeroExGaslessGetQuoteResponse) -> Result<Self, Self::Error> {
        let details = ZeroExGaslessQuoteDetails {
            raw_trade: value.trade,
            approval: value.approval,
        };

        Ok(Self {
            // ZeroEx doesn't have swap id specifically, use request id
            id: value.zid,
            swap_executor: SwapExecutorType::ZeroEx,
            estimated_to_amount_units: value.buy_amount,
            // TODO: it's not returned in response, also no data about asset precision
            // leave placeholder for now but in future probably it'll be better
            // to make this field optional
            estimated_to_amount: Decimal::ZERO,
            #[expect(
                clippy::arithmetic_side_effects,
                reason = "`Utc::now()` plus a fixed 5 minutes cannot approach DateTime<Utc>'s bounds"
            )]
            valid_till: Utc::now() + TimeDelta::minutes(5),
            quote_details: RawSwapDetails::ZeroExGasless(details),
        })
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
    Failed,
}

impl From<ZeroExGaslessTransactionStatus> for ExecutorSwapStatus {
    fn from(value: ZeroExGaslessTransactionStatus) -> Self {
        use ZeroExGaslessTransactionStatus::*;

        match value {
            Pending => ExecutorSwapStatus::Pending,
            Submitted => ExecutorSwapStatus::Pending,
            Succeeded => ExecutorSwapStatus::Pending,
            Confirmed => ExecutorSwapStatus::Executed,
            Failed => ExecutorSwapStatus::Failed,
        }
    }
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactionStatusResponse {
    pub status: ZeroExGaslessTransactionStatus,
    #[serde(default)]
    pub reason: Option<String>,
    pub zid: String,
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ZeroExErrorResponseData {
    pub zid: String,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ZeroExErrorResponse {
    pub name: String,
    pub message: String,
    pub data: ZeroExErrorResponseData,
}

// Deserializes only the literal `false`. Without this the untagged arm below
// matches on field *shape* alone, so any successful `liquidityAvailable: true`
// quote that failed the `Ok` arm for an unrelated reason (a renamed field, a
// newly-nullable one) lands here and is reported to the customer as "no
// liquidity" — a plausible-looking business answer in place of a loud parse
// error. Gating on the value makes the arm self-discriminating.
fn deserialize_liquidity_unavailable<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    if bool::deserialize(deserializer)? {
        return Err(serde::de::Error::custom(
            "liquidityAvailable is true; not a no-liquidity response",
        ))
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeroExNoLiquidityResponse {
    // Never read: it exists so the deserializer above runs and rejects the
    // arm when the flag is `true`.
    #[expect(dead_code)]
    #[serde(deserialize_with = "deserialize_liquidity_unavailable")]
    pub liquidity_available: (),
    pub zid: String,
}

/// Gateway-level errors (HTTP 401 invalid API key, 429 rate limited) don't
/// follow the business-error shape above — they only carry a message and a
/// request id.
#[derive(Debug, Deserialize)]
pub struct ZeroExGatewayError {
    pub message: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

// Order matters: untagged is first-successful-arm-wins in declaration order.
// `GatewayErr` is last because it is the most permissive shape ({message} plus
// an optional id): business errors also carry `message` and would otherwise be
// swallowed by it.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ZeroExResponse<T> {
    Ok(T),
    NoLiquidity(ZeroExNoLiquidityResponse),
    Err(ZeroExErrorResponse),
    GatewayErr(ZeroExGatewayError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_variants_deserialize_in_order() {
        // 401/429 gateway shape (live-verified) lands in the dedicated
        // variant instead of failing to match anything
        let gateway: ZeroExResponse<GetTransactionStatusResponse> =
            serde_json::from_str(r#"{"message":"Rate limit exceeded","request_id":"abc-123"}"#)
                .unwrap();
        assert!(matches!(
            gateway,
            ZeroExResponse::GatewayErr(ref e)
                if e.message == "Rate limit exceeded"
                    && e.request_id.as_deref() == Some("abc-123")
        ));

        // Business errors also contain `message` but must keep matching their
        // own variant (arm order)
        let business: ZeroExResponse<GetTransactionStatusResponse> = serde_json::from_str(
            r#"{"name":"INSUFFICIENT_BALANCE","message":"nope","data":{"zid":"z"}}"#,
        )
        .unwrap();
        assert!(matches!(
            business,
            ZeroExResponse::Err(_)
        ));

        // Success payloads still win
        let ok: ZeroExResponse<GetTransactionStatusResponse> =
            serde_json::from_str(r#"{"status":"confirmed","zid":"z1"}"#).unwrap();
        assert!(matches!(ok, ZeroExResponse::Ok(_)));
    }
}
