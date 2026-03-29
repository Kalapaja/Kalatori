mod types;

use std::time::Duration;

use kalatori_client::types::ChainType;
use serde::de::DeserializeOwned;
use serde::{
    Deserialize,
    Serialize,
};

use types::*;

pub use types::AcrossSwapStatus;

use crate::clients::swaps::{
    SwapsClient,
    SwapsClientError,
};
use crate::configs::{
    IntegratorFees,
    SwapsConfig,
};
use crate::types::{
    SwapChainType,
    SwapDetails,
    SwapExecutorType,
};

use super::RawSwapDetails;

const ACROSS_BASE_URL: &str = "https://app.across.to";
const ACROSS_CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossRawTransaction {
    pub transaction: SwapTransaction,
    #[serde(default)]
    pub approval_transactions: Vec<ApprovalTransaction>,
}

impl TryFrom<RawSwapDetails> for AcrossRawTransaction {
    type Error = SwapsClientError;

    fn try_from(value: RawSwapDetails) -> Result<Self, Self::Error> {
        let RawSwapDetails::Across(raw_transaction) = value else {
            return Err(SwapsClientError::WrongRawTransaction)
        };

        Ok(raw_transaction)
    }
}

#[cfg(test)]
pub fn default_across_raw_transaction() -> AcrossRawTransaction {
    AcrossRawTransaction {
        transaction: SwapTransaction {
            chain_id: 8453,
            contract_address: "".to_string(),
            data: "".to_string(),
            value: 100,
            gas: 100,
            max_fee_per_gas: 100,
            max_priority_fee_per_gas: 100,
        },
        approval_transactions: Vec::new(),
    }
}

pub type AcrossQuoteDetails = AcrossRawTransaction;

impl From<AcrossApiError> for SwapsClientError {
    fn from(_value: AcrossApiError) -> Self {
        Self::UnknownApiError
    }
}

#[derive(Clone)]
pub struct AcrossClient {
    client: reqwest::Client,
    #[expect(dead_code)]
    fees: Option<IntegratorFees>,
}

impl AcrossClient {
    pub fn new(swaps_config: &SwapsConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            fees: swaps_config.fees.clone(),
        }
    }

    #[tracing::instrument(skip(self))]
    async fn send_request<T, R>(
        &self,
        url: &str,
        params: T,
    ) -> Result<R, SwapsClientError>
    where
        T: Serialize + std::fmt::Debug,
        R: DeserializeOwned + std::fmt::Debug,
    {
        let full_url = format!("{ACROSS_BASE_URL}{url}");

        let raw_response = self
            .client
            .get(full_url)
            .query(&params)
            .timeout(ACROSS_CLIENT_REQUEST_TIMEOUT)
            .send()
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    error.source = ?e,
                    "Across request failed"
                )
            })?
            .text()
            .await?;

        tracing::trace!(
            text = %raw_response,
            "Got raw response text from across"
        );

        let response = serde_json::from_str(&raw_response).map_err(|e| {
            tracing::error!(
                text = %raw_response,
                error.source = ?e,
                "Error while trying to deserialize response from across"
            );

            SwapsClientError::UnknownApiError
        })?;

        tracing::trace!(
            ?response,
            "Got parsed response from across"
        );

        match response {
            AcrossApiResponse::Ok(data) => Ok(data),
            AcrossApiResponse::Err(e) => Err(e.into()),
        }
    }
}

impl SwapsClient for AcrossClient {
    type GetQuoteParams = SwapApprovalRequest;
    type GetQuoteResponse = SwapApprovalResponse;
    type RawTransactionDetails = AcrossRawTransaction;
    type SwapStatus = AcrossSwapStatus;

    const CROSS_CHAIN_SUPPORTED: bool = true;
    const EXECUTOR: SwapExecutorType = SwapExecutorType::Across;
    const GASLESS: bool = false;
    const SINGLE_CHAIN_SUPPORTED: bool = false;
    const SUPPORTED_CHAINS: &[ChainType] = &[ChainType::Polygon];
    const SUPPORTED_SWAP_CHAINS: &[SwapChainType] = &[];

    async fn get_quote_internal(
        &self,
        data: Self::GetQuoteParams,
    ) -> Result<Self::GetQuoteResponse, SwapsClientError> {
        self.send_request("/api/swap/approval", data)
            .await
    }

    async fn submit_transaction_internal(
        &self,
        _params: &SwapDetails,
    ) -> Result<super::TransactionHash, SwapsClientError> {
        // TODO: add more explicit error or implement server-side submission
        Err(SwapsClientError::UnknownApiError)
    }

    async fn get_transaction_status_internal(
        &self,
        data: &SwapDetails,
    ) -> Result<Self::SwapStatus, SwapsClientError> {
        let result: SwapStatusResponse = self
            .send_request("/api/deposits", data)
            .await?;

        Ok(result.status)
    }
}
