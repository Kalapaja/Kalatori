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
use crate::utils::logging::{
    category,
    operation,
};

use super::super::{
    ExecutorSwapStatus,
    RawSwapDetails,
    SwapsClientError,
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
    Received,
    #[serde(rename = "deposit-pending")]
    DepositPending,
    // Deposit has not been filled yet.
    Pending,
    // Deposit has expired and will not be filled. Expired deposits will be
    // refunded to the depositor on the originChainId in the next batch of
    // repayments.
    Expired,
    // Deposit has expired and the depositor has been successfully refunded
    // on the originChain.
    Refunded,
    #[serde(rename = "deposit-failed")]
    DepositFailed,
    #[serde(other)]
    Unknown,
}

impl From<AcrossSwapStatus> for ExecutorSwapStatus {
    fn from(value: AcrossSwapStatus) -> Self {
        match value {
            AcrossSwapStatus::Filled => Self::Executed,
            AcrossSwapStatus::Received => Self::Pending,
            AcrossSwapStatus::DepositPending => Self::Pending,
            AcrossSwapStatus::Pending => Self::Pending,
            AcrossSwapStatus::Expired => Self::Failed,
            AcrossSwapStatus::Refunded => Self::Failed,
            AcrossSwapStatus::DepositFailed => Self::Failed,
            AcrossSwapStatus::Unknown => Self::Pending,
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
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub value: Option<u128>,
    // Not presented in Solana transactions
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub gas: Option<u128>,
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub max_fee_per_gas: Option<u128>,
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default)]
    pub max_priority_fee_per_gas: Option<u128>,
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

impl TryFrom<SwapTransactionInternal> for SwapTransaction {
    type Error = SwapsClientError;

    fn try_from(value: SwapTransactionInternal) -> Result<Self, Self::Error> {
        // `value` genuinely defaults to zero — Across omits the key for
        // zero-value (ERC-20) transfers and documents `value ? BigInt(v) : 0n`
        // as the caller's handling.
        //
        // The gas parameters do not: Across documents them as omit-and-estimate
        // (`gas ? BigInt(gas) : undefined`), and Kassette submits them
        // unguarded via `BigInt(swapTx.gas)`. Substituting `0` would publish a
        // transaction with a zero gas limit and zero fee caps, which cannot be
        // mined; omitting the field would throw in the payer's browser. Reject
        // the quote instead of handing over either. Once Kassette accepts
        // absent gas (Kalapaja/Kassette#49) these can be passed through as
        // optional instead of rejected.
        let (Some(gas), Some(max_fee_per_gas), Some(max_priority_fee_per_gas)) = (
            value.gas,
            value.max_fee_per_gas,
            value.max_priority_fee_per_gas,
        ) else {
            tracing::warn!(
                error.category = category::SWAPS_CLIENT,
                error.operation = operation::GET_QUOTE,
                gas = ?value.gas,
                max_fee_per_gas = ?value.max_fee_per_gas,
                max_priority_fee_per_gas = ?value.max_priority_fee_per_gas,
                "Across quote is missing gas parameters, rejecting"
            );

            return Err(SwapsClientError::UnusableQuote)
        };

        Ok(Self {
            chain_id: value.chain_id,
            contract_address: value.to,
            data: value.data,
            value: value.value.unwrap_or_default(),
            gas,
            max_fee_per_gas,
            max_priority_fee_per_gas,
        })
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
    // Across's docs describe the no-approval-needed case as an empty array and
    // its own examples omit the key, but production responses on 2026-08-03
    // carried an explicit `"approvalTxns": null`, which a bare `Vec` +
    // `#[serde(default)]` rejects. Accept all three shapes.
    #[serde(default)]
    pub approval_txns: Option<Vec<ApprovalTransaction>>,
    pub swap_tx: SwapTransactionInternal,
    pub id: String,
    pub quote_expiry_timestamp: i64,
}

impl TryFrom<SwapApprovalResponse> for SwapQuote {
    type Error = SwapsClientError;

    fn try_from(value: SwapApprovalResponse) -> Result<Self, Self::Error> {
        let details = AcrossQuoteDetails {
            transaction: value.swap_tx.try_into()?,
            approval_transactions: value.approval_txns.unwrap_or_default(),
        };

        // Provider-controlled timestamp: any `i64` deserializes, but only a
        // subset is representable, and `None` here used to panic the daemon.
        let valid_till =
            DateTime::from_timestamp_secs(value.quote_expiry_timestamp).ok_or_else(|| {
                tracing::warn!(
                    error.category = category::SWAPS_CLIENT,
                    error.operation = operation::GET_QUOTE,
                    quote_expiry_timestamp = value.quote_expiry_timestamp,
                    "Across quote expiry is not a representable timestamp, rejecting"
                );

                SwapsClientError::UnusableQuote
            })?;

        Ok(Self {
            swap_executor: SwapExecutorType::Across,
            id: value.id,
            estimated_to_amount_units: value.expected_output_amount,
            // TODO: in response there's output token with it's params (decimals), so we can
            // calculate it
            estimated_to_amount: Decimal::ZERO,
            valid_till,
            quote_details: RawSwapDetails::Across(details),
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapStatusRequest<'a> {
    pub deposit_txn_ref: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
// Only `status` is read. Every other field is optional on purpose: Across's own
// `/deposit` example returns `"depositId": null` for non-intent deposit types
// (CCTP `DepositForBurn`, OFT `OFTSent`) — and Across routes USDC over CCTP —
// while the early `received` / `deposit-pending` states have no proven contract
// for the identifiers at all. A required field here breaks status polling for
// the swap permanently, so nothing that is never read may be required.
pub struct SwapStatusResponse {
    pub status: AcrossSwapStatus,
    #[expect(dead_code)]
    #[serde(default)]
    pub origin_chain_id: Option<u64>,
    #[expect(dead_code)]
    #[serde(default)]
    pub deposit_id: Option<String>,
    #[expect(dead_code)]
    #[serde(default)]
    pub deposit_txn_ref: Option<String>,
    #[expect(dead_code)]
    #[serde(default)]
    pub fill_txn_ref: Option<String>,
    #[expect(dead_code)]
    #[serde(default)]
    pub destination_chain_id: Option<u64>,
    #[expect(dead_code)]
    #[serde(default)]
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
