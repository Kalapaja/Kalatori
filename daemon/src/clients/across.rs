mod types;

use std::time::Duration;

use serde::{Serialize, Deserialize};
use serde::de::DeserializeOwned;

use types::*;

pub use types::AcrossSwapStatus;

const ACROSS_BASE_URL: &'static str = "https://app.across.to";
const ACROSS_CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossRawTransaction {
    pub transaction: SwapTransaction,
    #[serde(default)]
    pub approval_transactions: Vec<ApprovalTransaction>,
}

#[cfg(test)]
pub fn default_across_raw_transaction() -> AcrossRawTransaction {
    AcrossRawTransaction {
        transaction: SwapTransaction {
            chain_id: 8453,
            contract_address: "".to_string(),
            data: "".to_string(),
            gas: 100,
            max_fee_per_gas: 100,
            max_priority_fee_per_gas: 100,
        },
        approval_transactions: Vec::new(),
    }
}

pub type AcrossQuoteDetails = AcrossRawTransaction;

#[derive(Debug, thiserror::Error)]
pub enum AcrossClientError {
    #[error("Across API Error with code")]
    AcrossError {
        message: String,
    },
    #[error("Request failed")]
    RequestFailed,
}

impl From<AcrossApiError> for AcrossClientError {
    fn from(value: AcrossApiError) -> Self {
        Self::AcrossError {
            message: value.message,
        }
    }
}

impl From<reqwest::Error> for AcrossClientError {
    fn from(_value: reqwest::Error) -> Self {
        Self::RequestFailed
    }
}

#[derive(Clone)]
pub struct AcrossClient {
    client: reqwest::Client,
}

impl AcrossClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    #[tracing::instrument(skip(self))]
    async fn send_request<T, R>(
        &self,
        url: &str,
        params: T,
    ) -> Result<R, AcrossClientError>
      where T: Serialize + std::fmt::Debug,
            R: DeserializeOwned + std::fmt::Debug,
    {
        let full_url = format!("{ACROSS_BASE_URL}{url}");

        let raw_response = self.client
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

            AcrossClientError::RequestFailed
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

    pub async fn get_swap_approval(
        &self,
        data: SwapApprovalRequest,
    ) -> Result<SwapApprovalResponse, AcrossClientError> {
        self.send_request("/api/swap/approval", data).await
    }

    pub async fn get_swap_status(
        &self,
        data: SwapStatusRequest,
    ) -> Result<SwapStatusResponse, AcrossClientError> {
        self.send_request("/api/deposit/status", data).await
    }

    pub async fn get_deposits_by_address(
        &self,
        data: GetDepositsRequest,
    ) -> Result<Vec<GetDepositsResponse>, AcrossClientError> {
        self.send_request("/api/deposits", data).await
    }
}
