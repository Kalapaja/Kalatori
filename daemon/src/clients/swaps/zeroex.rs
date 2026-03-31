mod types;

use alloy::primitives::B256;
use alloy::providers::fillers::FillProvider;
use alloy::providers::utils::JoinedRecommendedFillers;
use alloy::providers::{
    Provider,
    ProviderBuilder,
    RootProvider,
};
use kalatori_client::types::ChainType;
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
use crate::types::{
    SwapChainType,
    SwapDetails,
    SwapExecutorType,
};

use super::{
    ExecutorSwapStatus,
    RawSwapDetails,
    SwapsClient,
    SwapsClientError,
};

use types::*;

type ChainProvider = FillProvider<JoinedRecommendedFillers, RootProvider>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroExTransactionStatus {
    NotFound,
    Executed,
    FailedOnChain,
}

impl From<ZeroExTransactionStatus> for ExecutorSwapStatus {
    fn from(value: ZeroExTransactionStatus) -> Self {
        match value {
            ZeroExTransactionStatus::NotFound => Self::Pending,
            ZeroExTransactionStatus::Executed => Self::Executed,
            ZeroExTransactionStatus::FailedOnChain => Self::Failed,
        }
    }
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
    // pub permit_hash: String,
    // pub permit_data: serde_json::Value,
    pub raw_transaction: RawTransactionData,
}

#[cfg(test)]
pub fn default_zero_ex_raw_transaction() -> ZeroExRawTransaction {
    ZeroExRawTransaction {
        allowance_target: "0x0000000000001ff3684f28c67538d4d072c22734".to_string(),
        raw_transaction: RawTransactionData {
            to: "0x0000000000001ff3684f28c67538d4d072c22734".to_string(),
            data: "0x2213bc0b000000000000000000000000b0873c46937d34e98615e8c868bd3580bc6dcd4700000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ea5ed2ad6f9c5fa000000000000000000000000b0873c46937d34e98615e8c868bd3580bc6dcd4700000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000006641fff991f0000000000000000000000005151537093e68f61a48cb98ba56922abcd2bc5e40000000000000000000000003c499c542cef5e3811e1192ce70d8cc03d5c3359000000000000000000000000000000000000000000000000000000000001821600000000000000000000000000000000000000000000000000000000000000a0f5303154d2e14bb0f960c9410000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000500000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000001200000000000000000000000000000000000000000000000000000000000000260000000000000000000000000000000000000000000000000000000000000038000000000000000000000000000000000000000000000000000000000000004400000000000000000000000000000000000000000000000000000000000000044bd01c2260000000000000000000000000000000000000000000000000000000069c2e3d00000000000000000000000000000000000000000000000000ea5ed2ad6f9c5fa00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000010438c9c147000000000000000000000000eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee00000000000000000000000000000000000000000000000000000000000027100000000000000000000000000d500b1d8e8ef31e21c99d1db9a6444d3adf1270000000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000024d0e30db00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e48d68a156000000000000000000000000b0873c46937d34e98615e8c868bd3580bc6dcd4700000000000000000000000000000000000000000000000000000000000027100000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000400d500b1d8e8ef31e21c99d1db9a6444d3adf1270000001f400000000000000000000000000000001000276a43c499c542cef5e3811e1192ce70d8cc03d5c335900000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008434ee90ca000000000000000000000000f5c4f3dc02c3fb9279495a8fef7b0741da9561570000000000000000000000003c499c542cef5e3811e1192ce70d8cc03d5c335900000000000000000000000000000000000000000000000000000000000187a1000000000000000000000000000000000000000000000000000000000000271000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000012438c9c1470000000000000000000000003c499c542cef5e3811e1192ce70d8cc03d5c3359000000000000000000000000000000000000000000000000000000000000000f0000000000000000000000003c499c542cef5e3811e1192ce70d8cc03d5c3359000000000000000000000000000000000000000000000000000000000000002400000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000044a9059cbb000000000000000000000000ad01c20d5886137e056775af56915de824c8fce50000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000".to_string(), gas: 302525, gas_price: 198773677713, value: 1055510455939352058 }
    }
}

impl TryFrom<RawSwapDetails> for ZeroExRawTransaction {
    type Error = SwapsClientError;

    fn try_from(value: RawSwapDetails) -> Result<Self, Self::Error> {
        let RawSwapDetails::ZeroEx(raw_transaction) = value else {
            return Err(SwapsClientError::WrongRawTransaction)
        };

        Ok(raw_transaction)
    }
}

pub type ZeroExQuoteDetails = ZeroExRawTransaction;

impl From<ZeroExErrorResponse> for SwapsClientError {
    fn from(_value: ZeroExErrorResponse) -> Self {
        Self::UnknownApiError
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
}

impl SwapsClient for ZeroExClient {
    type GetQuoteParams = ZeroExGetQuoteRequest;
    type GetQuoteResponse = ZeroExGetQuoteResponse;
    type RawTransactionDetails = ZeroExRawTransaction;
    type SwapStatus = ZeroExTransactionStatus;

    const CROSS_CHAIN_SUPPORTED: bool = false;
    const EXECUTOR: SwapExecutorType = SwapExecutorType::ZeroEx;
    const GASLESS: bool = false;
    const SINGLE_CHAIN_SUPPORTED: bool = true;
    const SUPPORTED_CHAINS: &[ChainType] = &[ChainType::Polygon];
    // https://docs.0x.org/docs/introduction/supported-chains
    const SUPPORTED_SWAP_CHAINS: &[SwapChainType] = &[
        SwapChainType::Ethereum,
        SwapChainType::Abstract,
        SwapChainType::Arbitrum,
        SwapChainType::Avalanche,
        SwapChainType::Base,
        SwapChainType::Berachain,
        SwapChainType::Blast,
        SwapChainType::BnbSmartChain,
        SwapChainType::HyperEvm,
        SwapChainType::Ink,
        SwapChainType::Linea,
        SwapChainType::Mantle,
        SwapChainType::Mode,
        SwapChainType::Monad,
        SwapChainType::Optimism,
        SwapChainType::Plasma,
        SwapChainType::Polygon,
        SwapChainType::Scroll,
        SwapChainType::Sonic,
        SwapChainType::Tempo,
        SwapChainType::Unichain,
        SwapChainType::WorldChain,
    ];

    async fn get_quote_internal(
        &self,
        data: Self::GetQuoteParams,
    ) -> Result<Self::GetQuoteResponse, SwapsClientError> {
        let response = self
            .client
            .get("https://api.0x.org/swap/allowance-holder/quote")
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
                SwapsClientError::UnknownApiError
            })?;

        let text = response.text().await.map_err(|e| {
            tracing::warn!(error = ?e, "Failed to extract response text from 0x response");
            SwapsClientError::UnknownApiError
        })?;

        tracing::trace!(%text, "Got raw text response from 0x API");

        let result = serde_json::from_str(&text).map_err(|e| {
            tracing::warn!(error = ?e, "Failed to deserialize response from 0x API");
            SwapsClientError::UnknownApiError
        })?;

        match result {
            ZeroExResponse::Ok(resp) => Ok(resp),
            ZeroExResponse::Err(e) => Err(e.into()),
        }
    }

    async fn submit_transaction_internal(
        &self,
        data: &SwapDetails,
    ) -> Result<super::TransactionHash, SwapsClientError> {
        let transaction = self.extract_signature(data)?;

        let transaction_bytes = const_hex::decode(transaction).map_err(|e| {
            tracing::warn!(error = ?e, "Failed to decode 0x transaction");
            SwapsClientError::UnknownApiError
        })?;

        let submitted = self
            .chain_client
            .send_raw_transaction(&transaction_bytes)
            .await
            .map_err(|e| {
                tracing::warn!(error = ?e, "Failed to send 0x transaction to chain");
                SwapsClientError::UnknownApiError
            })?;

        let tx_hash = const_hex::encode_prefixed(submitted.tx_hash());

        Ok(tx_hash)
    }

    async fn get_transaction_status_internal(
        &self,
        data: &SwapDetails,
    ) -> Result<Self::SwapStatus, SwapsClientError> {
        let transaction_hash = self.extract_transaction_hash(data)?;

        let bytes = const_hex::decode(transaction_hash).map_err(|e| {
            tracing::warn!(error = ?e, "Failed to decode 0x transaction");
            SwapsClientError::UnknownApiError
        })?;

        let tx_hash = B256::try_from(bytes.as_slice()).map_err(|e| {
            tracing::warn!(error = ?e, "Failed to decode 0x transaction");
            SwapsClientError::UnknownApiError
        })?;

        let result = self
            .chain_client
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|e| {
                tracing::warn!(error = ?e, "Chain request to get 0x transaction status failed");
                SwapsClientError::UnknownApiError
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
