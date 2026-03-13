mod types;

use std::sync::Arc;
use std::time::Duration;

use governor::{
    DefaultDirectRateLimiter,
    Quota,
    RateLimiter,
};
use uuid::Uuid;

use crate::configs::EtherscanClientConfig;
use crate::types::{
    ChainType,
    IncomingTransaction,
};

use types::*;

const ETHERSCAN_CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum EtherscanClientError {
    #[error("Unsupported chain: {chain}")]
    UnsupportedChain { chain: ChainType },
    #[error("Etherscan API error. Message: {message}, result: {result}")]
    EtherscanError { message: String, result: String },
    #[error("Request failed")]
    RequestFailed,
}

impl From<EtherscanResponseData<String>> for EtherscanClientError {
    fn from(value: EtherscanResponseData<String>) -> EtherscanClientError {
        // TODO: match message/result and try to find some common errors
        // like invalid API key, parameters etc
        EtherscanClientError::EtherscanError {
            message: value.message,
            result: value.result,
        }
    }
}

impl From<reqwest::Error> for EtherscanClientError {
    fn from(_value: reqwest::Error) -> Self {
        Self::RequestFailed
    }
}

#[derive(Clone)]
pub struct EtherscanClient {
    client: reqwest::Client,
    api_key: String,
    rate_limiter: Arc<DefaultDirectRateLimiter>,
}

impl EtherscanClient {
    pub fn new(config: EtherscanClientConfig) -> Self {
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            config.requests_per_second,
        )));

        Self {
            client: reqwest::Client::new(),
            api_key: config.api_key,
            rate_limiter,
        }
    }

    #[tracing::instrument(skip(self))]
    async fn get_account_transfers(
        &self,
        chain_id: u32,
        contract_address: &str,
        address: &str,
    ) -> Result<Vec<EtherscanTransaction>, EtherscanClientError> {
        self.rate_limiter.until_ready().await;

        let params = GetAccountTokenTransactionsParams {
            module: "account",
            action: "tokentx",
            chain_id,
            contract_address,
            address,
            api_key: &self.api_key,
        };

        let raw_response = self
            .client
            .get("https://api.etherscan.io/v2/api")
            .query(&params)
            .timeout(ETHERSCAN_CLIENT_REQUEST_TIMEOUT)
            .send()
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    error.source = ?e,
                    "Etherscan request failed"
                )
            })?
            .text()
            .await
            .unwrap();

        tracing::trace!(
            text = %raw_response,
            "Got raw response text from etherscan",
        );

        let response = serde_json::from_str(&raw_response).map_err(|e| {
            tracing::error!(
                text = %raw_response,
                error.source = ?e,
                "Error while trying to deserialize response from etherscan"
            );

            EtherscanClientError::RequestFailed
        })?;

        tracing::trace!(
            ?response,
            "Got parsed response from etherscan"
        );

        match response {
            EtherscanResponse::Ok(data) => Ok(data.result),
            EtherscanResponse::Err(error) => Err(error.into()),
        }
    }

    #[tracing::instrument(skip(self), fields(category = "etherscan_client"))]
    pub async fn get_account_incoming_transfers(
        &self,
        chain: ChainType,
        asset_id: &str,
        address: &str,
        invoice_id: Uuid,
    ) -> Result<Vec<IncomingTransaction>, EtherscanClientError> {
        let chain_id = match chain {
            ChainType::Polygon => 137,
            ChainType::PolkadotAssetHub => {
                return Err(EtherscanClientError::UnsupportedChain {
                    chain,
                })
            },
        };

        let result = self
            .get_account_transfers(chain_id, asset_id, address)
            .await?
            .into_iter()
            .filter_map(|trans| {
                (trans.to.to_lowercase() == address.to_lowercase())
                    .then(|| trans.into_incoming_transaction(invoice_id))
            })
            .collect();

        Ok(result)
    }
}
