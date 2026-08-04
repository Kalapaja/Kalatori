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
    // Across simulates against the depositor's CURRENT state, so this is
    // legitimately `false` before the user has granted approvals. We no longer
    // act on it — unusable gas sentinels are normalized during transaction
    // conversion — but it must still never be *required*: Across's schema marks
    // it optional and `/swap/gasless` omits it altogether, so a bare `bool`
    // would fail the entire quote the first time it is absent.
    #[expect(dead_code)]
    #[serde(default = "default_simulation_success")]
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

fn default_simulation_success() -> bool {
    // Only reached when Across omits the field entirely. The value is never
    // read; it exists so an absent `simulationSuccess` doesn't fail the quote.
    true
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
    // Omit-and-estimate, not zero: with `simulationSuccess: false`, Across sends
    // `gas: "0"` while genuinely omitting the fee caps and `value`. We normalize
    // a zero gas limit or max fee cap to absent; dropping the cap also drops the
    // priority fee. A zero priority fee remains valid alongside a non-zero cap.
    // Kassette turns absent gas fields into `undefined`, so the payer's wallet
    // estimates them instead of publishing an unmineable zero-gas transaction.
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gas: Option<u128>,
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fee_per_gas: Option<u128>,
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_priority_fee_per_gas: Option<u128>,
}

impl From<SwapTransactionInternal> for SwapTransaction {
    fn from(value: SwapTransactionInternal) -> Self {
        // `value` genuinely defaults to zero — Across omits the key for
        // zero-value (ERC-20) transfers and documents `value ? BigInt(v) : 0n`
        // as the caller's handling. Across uses zero as the unusable sentinel
        // for `gas` and `maxFeePerGas`; without a usable fee cap, its priority
        // fee must be dropped too. See the field comments above.
        let gas = value.gas.filter(|gas| *gas != 0);
        let max_fee_per_gas = value
            .max_fee_per_gas
            .filter(|fee| *fee != 0);
        let max_priority_fee_per_gas = max_fee_per_gas.and(value.max_priority_fee_per_gas);

        Self {
            chain_id: value.chain_id,
            contract_address: value.to,
            data: value.data,
            value: value.value.unwrap_or_default(),
            gas,
            max_fee_per_gas,
            max_priority_fee_per_gas,
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
            transaction: value.swap_tx.into(),
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
