mod types;

use alloy::primitives::B256;
use alloy::providers::fillers::FillProvider;
use alloy::providers::utils::JoinedRecommendedFillers;
use alloy::providers::{
    Provider,
    ProviderBuilder,
    RootProvider,
};
use secrecy::{
    ExposeSecret,
    SecretString,
};
use serde::{
    Deserialize,
    Serialize,
};
use serde_with::{
    DisplayFromStr,
    serde_as,
};

use crate::configs::{
    IntegratorFees,
    SwapsConfig,
};

use types::*;

type ChainProvider = FillProvider<JoinedRecommendedFillers, RootProvider>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroExTransactionStatus {
    NotFound,
    Executed,
    FailedOnChain,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawTransactionData {
    to: String,
    data: String,
    #[serde_as(as = "DisplayFromStr")]
    gas: u64,
    #[serde_as(as = "DisplayFromStr")]
    gas_price: u128,
    #[serde_as(as = "DisplayFromStr")]
    value: u128,
}

impl From<ZeroExTransaction> for RawTransactionData {
    fn from(value: ZeroExTransaction) -> Self {
        Self {
            to: value.to,
            data: value.data,
            gas: value.gas,
            gas_price: value.gas_price,
            value: value.value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZeroExRawTransaction {
    pub allowance_target: String,
    pub permit_hash: String,
    pub permit_data: serde_json::Value,
    pub raw_transaction: RawTransactionData,
}

pub type ZeroExQuoteDetails = ZeroExRawTransaction;

#[derive(Debug, thiserror::Error)]
pub enum ZeroExClientError {
    #[error("0x API Error")]
    ZeroExError { code: String, message: String },
    #[error("Request failed")]
    RequestFailed,
}

impl From<ZeroExErrorResponse> for ZeroExClientError {
    fn from(value: ZeroExErrorResponse) -> Self {
        Self::ZeroExError {
            code: value.name,
            message: value.message,
        }
    }
}

#[derive(Clone)]
pub struct ZeroExClient {
    client: reqwest::Client,
    chain_client: ChainProvider,
    #[expect(dead_code)]
    fees: Option<IntegratorFees>,
    api_key: SecretString,
}

impl ZeroExClient {
    pub async fn new(config: &SwapsConfig) -> Self {
        let chain_client = ProviderBuilder::new()
            .connect(&config.zero_ex.rpc_url)
            .await
            .expect("Failed to connect to RPC endpoint for 0x client");

        Self {
            client: reqwest::Client::new(),
            chain_client,
            fees: config.fees.clone(),
            api_key: config.zero_ex.api_key.clone(),
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_quote(
        &self,
        data: ZeroExGetQuoteRequest,
    ) -> Result<ZeroExGetQuoteResponse, ZeroExClientError> {
        let response = self
            .client
            .get("https://api.0x.org/swap/permit2/quote")
            .header(
                "0x-api-key",
                self.api_key.expose_secret(),
            )
            .header("0x-version", "v2")
            .query(&data)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = ?e, "Error while send request to 0x API");
                ZeroExClientError::RequestFailed
            })?;

        let text = response.text().await.map_err(|e| {
            tracing::warn!(error = ?e, "Failed to extract response text from 0x response");
            ZeroExClientError::RequestFailed
        })?;

        tracing::trace!(%text, "Got raw text response from 0x API");

        let result = serde_json::from_str(&text).map_err(|e| {
            tracing::warn!(error = ?e, "Failed to deserialize response from 0x API");
            ZeroExClientError::RequestFailed
        })?;

        match result {
            ZeroExResponse::Ok(resp) => Ok(resp),
            ZeroExResponse::Err(e) => Err(e.into()),
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn submit_transaction(
        &self,
        transaction: String,
    ) -> Result<String, ZeroExClientError> {
        let transaction_bytes = const_hex::decode(&transaction).map_err(|e| {
            tracing::warn!(error = ?e, "Failed to decode 0x transaction");
            ZeroExClientError::RequestFailed
        })?;

        let submitted = self
            .chain_client
            .send_raw_transaction(&transaction_bytes)
            .await
            .map_err(|e| {
                tracing::warn!(error = ?e, "Failed to send 0x transaction to chain");
                ZeroExClientError::RequestFailed
            })?;

        let tx_hash = const_hex::encode_prefixed(submitted.tx_hash());

        Ok(tx_hash)
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_transaction_status(
        &self,
        transaction_hash: &str,
    ) -> Result<ZeroExTransactionStatus, ZeroExClientError> {
        let bytes = const_hex::decode(transaction_hash).map_err(|e| {
            tracing::warn!(error = ?e, "Failed to decode 0x transaction");
            ZeroExClientError::RequestFailed
        })?;

        let tx_hash = B256::try_from(bytes.as_slice()).map_err(|e| {
            tracing::warn!(error = ?e, "Failed to decode 0x transaction");
            ZeroExClientError::RequestFailed
        })?;

        let result = self
            .chain_client
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|e| {
                tracing::warn!(error = ?e, "Chain request to get 0x transaction status failed");
                ZeroExClientError::RequestFailed
            })?
            .map(|receipt| receipt.status());

        let status = match result {
            None => ZeroExTransactionStatus::NotFound,
            Some(true) => ZeroExTransactionStatus::Executed,
            Some(false) => ZeroExTransactionStatus::FailedOnChain,
        };

        Ok(status)
    }
}
