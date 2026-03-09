mod types;

use std::time::Duration;

use serde::{Serialize, Deserialize};
use serde::de::DeserializeOwned;

use types::*;

pub use types::BungeeSwapStatus;

const BUNGEE_BASE_URL: &'static str = "https://public-backend.bungee.exchange";
const BUNGEE_CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

// Although it's a copy of `QuoteAutoRoute` structure, it's better
// to leave it as is. Otherwise we'll have to implement different
// `rename_all` for serialize and deserialize + this structure can be modified in future
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BungeeRawTransaction {
    pub quote_id: String,
    pub request_type: String,
    pub approval_data: ApprovalData,
    pub sign_typed_data: SignTypedData,
}

pub type BungeeQuoteDetails = BungeeRawTransaction;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BungeeApiResponse<T> {
    // result is empty in case of error
    result: Option<T>,
    success: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    status_code: Option<u32>,
}

impl<T> From<BungeeApiResponse<T>> for BungeeClientError {
    fn from(value: BungeeApiResponse<T>) -> Self {
        Self::BungeeError {
            message: value.message.unwrap_or("Error message is not provided in response".to_string()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BungeeClientError {
    #[error("Bungee API Error")]
    BungeeError {
        message: String,
    },
    #[error("Request failed")]
    RequestFailed,
}

impl From<reqwest::Error> for BungeeClientError {
    fn from(_value: reqwest::Error) -> Self {
        Self::RequestFailed
    }
}

#[derive(Clone)]
pub struct BungeeClient {
    client: reqwest::Client,
}

impl BungeeClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    #[tracing::instrument(skip(self))]
    async fn send_request<T, R>(
        &self,
        url: &str,
        method: reqwest::Method,
        params: T,
    ) -> Result<R, BungeeClientError>
      where T: Serialize + std::fmt::Debug,
            R: DeserializeOwned + std::fmt::Debug,
    {
        let full_url = format!("{BUNGEE_BASE_URL}{url}");

        let request = self.client
            .request(method.clone(), full_url)
            .timeout(BUNGEE_CLIENT_REQUEST_TIMEOUT);

        let request = if let reqwest::Method::POST = method {
            request.json(&params)
        } else {
            request.query(&params)
        };

        let raw_response = request
            .send()
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    error.source = ?e,
                    "Bungee request failed"
                )
            })?
            .text()
            .await?;

        tracing::trace!(
            text = %raw_response,
            "Got raw response text from Bungee"
        );

        let response: BungeeApiResponse<R> = serde_json::from_str(&raw_response).map_err(|e| {
            tracing::error!(
                text = %raw_response,
                error.source = ?e,
                "Error while trying to deserialize response from Bungee"
            );

            BungeeClientError::RequestFailed
        })?;

        tracing::trace!(
            ?response,
            "Got parsed response from Bungee"
        );

        if response.success && response.result.is_some() {
            Ok(response.result.unwrap())
        } else {
            Err(response.into())
        }
    }

    pub async fn get_swap_quote(
        &self,
        data: QuoteRequest,
    ) -> Result<QuoteResponse, BungeeClientError> {
        self.send_request(
            "/api/v1/bungee/quote",
            reqwest::Method::GET,
            data
        ).await
        // TODO: check if `auto_quote` is empty, if so return an error
    }

    pub async fn submit_signed_request(
        &self,
        data: SubmitOrderRequest,
    ) -> Result<SubmitOrderResponse, BungeeClientError> {
        self.send_request(
            "/api/v1/bungee/submit",
            reqwest::Method::POST,
            data,
        ).await
    }

    pub async fn get_swap_status(
        &self,
        data: GetSwapStatusRequest,
    ) -> Result<Vec<GetSwapStatusResponse>, BungeeClientError> {
        self.send_request(
            "/api/v1/bungee/status",
            reqwest::Method::GET,
            data
        ).await
    }
}
