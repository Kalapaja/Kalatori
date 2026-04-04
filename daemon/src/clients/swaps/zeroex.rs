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
use serde::de::DeserializeOwned;
use serde_with::{
    DisplayFromStr,
    serde_as,
};
use uuid::Uuid;

use crate::chain_client::{KeyringClient, SignPermitRequestData};
use crate::configs::{
    IntegratorFees,
    SwapsConfig,
};
use crate::types::{
    Swap,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZeroExGaslessRawTransaction {
    pub raw_trade: ZeroExTrade,
    pub approval: Option<ZeroExTrade>,
}

#[cfg(test)]
pub fn default_zero_ex_gasless_raw_transaction() -> ZeroExGaslessRawTransaction {
    ZeroExGaslessRawTransaction {
        raw_trade: ZeroExTrade {
            trade_type: "settler_metatransaction".to_string(),
            hash: "0x3ff032fa3a970a3f2b763afce093fd133ced63c0b097ab12ae1441b42de4a167".to_string(),
            eip712: serde_json::json!({
                "types": {
                    "PermitWitnessTransferFrom": [
                    {
                        "name": "permitted",
                        "type": "TokenPermissions"
                    },
                    {
                        "name": "spender",
                        "type": "address"
                    },
                    {
                        "name": "nonce",
                        "type": "uint256"
                    },
                    {
                        "name": "deadline",
                        "type": "uint256"
                    },
                    {
                        "name": "slippageAndActions",
                        "type": "SlippageAndActions"
                    }
                    ],
                    "EIP712Domain": [
                    {
                        "name": "name",
                        "type": "string"
                    },
                    {
                        "name": "chainId",
                        "type": "uint256"
                    },
                    {
                        "name": "verifyingContract",
                        "type": "address"
                    }
                    ],
                    "TokenPermissions": [
                    {
                        "name": "token",
                        "type": "address"
                    },
                    {
                        "name": "amount",
                        "type": "uint256"
                    }
                    ],
                    "SlippageAndActions": [
                    {
                        "name": "recipient",
                        "type": "address"
                    },
                    {
                        "name": "buyToken",
                        "type": "address"
                    },
                    {
                        "name": "minAmountOut",
                        "type": "uint256"
                    },
                    {
                        "name": "actions",
                        "type": "bytes[]"
                    }
                    ]
                },
                "domain": {
                    "name": "Permit2",
                    "chainId": 1,
                    "verifyingContract": "0x000000000022d473030f116ddee9f6b43ac78ba3"
                },
                "message": {
                    "permitted": {
                    "token": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
                    "amount": "300000000"
                    },
                    "spender": "0x7c39a136ea20b3483e402ea031c1f3c019bab24b",
                    "nonce": "2241959297937691820908574931991567",
                    "deadline": "1718670104",
                    "slippageAndActions": {
                    "recipient": "0x70a9f34f9b34c64957b9c401a97bfed35b95049e",
                    "buyToken": "0xdac17f958d2ee523a2206206994597c13d831ec7",
                    "minAmountOut": "292116101",
                    "actions": [
                        "0x0dfeb4190000000000000000000000007c39a136ea20b3483e402ea031c1f3c019bab24b000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb480000000000000000000000000000000000000000000000000000000011e1a3000000000000000000000000000000000000006e898131631616b1779bad70bc0f000000000000000000000000000000000000000000000000000000006670d318",
                        "0x38c9c147000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb4800000000000000000000000000000000000000000000000000000000000027100000000000000000000000006146be494fee4c73540cb1c5f87536abf1452500000000000000000000000000000000000000000000000000000000000000004400000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000084c31b8d7a0000000000000000000000007c39a136ea20b3483e402ea031c1f3c019bab24b00000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000011e1a30000000000000000000000000000000000000000000000000000000001000276a400000000000000000000000000000000000000000000000000000000",
                        "0x38c9c147000000000000000000000000dac17f958d2ee523a2206206994597c13d831ec700000000000000000000000000000000000000000000000000000000000000ec000000000000000000000000dac17f958d2ee523a2206206994597c13d831ec7000000000000000000000000000000000000000000000000000000000000002400000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000044a9059cbb00000000000000000000000038f5e5b4da37531a6e85161e337e0238bb27aa90000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
                    ]
                    }
                },
                "primaryType": "PermitWitnessTransferFrom"
            }),
        },
        approval: None,
    }
}

impl TryFrom<RawSwapDetails> for ZeroExGaslessRawTransaction {
    type Error = SwapsClientError;

    fn try_from(value: RawSwapDetails) -> Result<Self, Self::Error> {
        let RawSwapDetails::ZeroExGasless(raw_transaction) = value else {
            return Err(SwapsClientError::WrongRawTransaction)
        };

        Ok(raw_transaction)
    }
}

pub type ZeroExGaslessQuoteDetails = ZeroExGaslessRawTransaction;

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

#[derive(Clone)]
pub struct ZeroExGaslessClient {
    client: reqwest::Client,
    #[expect(dead_code)]
    fees: Option<IntegratorFees>,
    api_key: SecretString,
}

impl ZeroExGaslessClient {
    pub fn new(config: &SwapsConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            fees: config.fees.clone(),
            api_key: config.zero_ex.api_key.clone(),
        }
    }

    #[tracing::instrument(skip(self))]
    async fn send_request<T, R>(
        &self,
        url: &str,
        method: reqwest::Method,
        params: T,
    ) -> Result<R, SwapsClientError>
    where
        T: Serialize + std::fmt::Debug,
        R: DeserializeOwned + std::fmt::Debug,
    {
        // let full_url = format!("{}{}", self.base_url, url);
        let full_url = format!("https://api.0x.org{}", url);

        let request = self
            .client
            .request(method.clone(), full_url)
            .header(
                "0x-api-key",
                self.api_key.expose_secret(),
            )
            .header("0x-version", "v2");

        let request = if let reqwest::Method::POST = method {
            request.json(&params)
        } else {
            request.query(&params)
        };

        let response = request
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

        tracing::trace!(%text, "Got raw text response from 0x gasless API");

        let result = serde_json::from_str(&text).map_err(|e| {
            tracing::warn!(error = ?e, "Failed to deserialize response from 0x API");
            SwapsClientError::UnknownApiError
        })?;

        match result {
            ZeroExResponse::Ok(resp) => Ok(resp),
            ZeroExResponse::Err(e) => Err(e.into()),
        }
    }

    async fn sign_hash(
        &self,
        hash: &str,
        invoice_id: Uuid,
        keyring_client: &KeyringClient,
    ) -> Result<String, SwapsClientError> {
        let hash = B256::from_slice(
            &const_hex::decode(hash).map_err(|e| {
                tracing::error!(error = ?e, %hash,  "Failed to decode stored trade hash");
                SwapsClientError::FailedToSignTransaction
            })?
        );

        let data = SignPermitRequestData {
            permit_hash: hash,
            derivation_params: vec![invoice_id.to_string()],
        };

        let signed = keyring_client
            .sign_polygon_permit(data)
            .await
            .map_err(|e| {
                tracing::warn!(error = ?e, "Failed to sign transaction");
                SwapsClientError::FailedToSignTransaction
            })?;

        Ok(const_hex::encode_prefixed(signed.signature.as_bytes()))
    }
}

impl SwapsClient for ZeroExGaslessClient {
    type GetQuoteParams = ZeroExGetQuoteRequest;
    type GetQuoteResponse = ZeroExGaslessGetQuoteResponse;
    type RawTransactionDetails = ZeroExGaslessRawTransaction;
    type SwapStatus = ZeroExGaslessTransactionStatus;

    const CROSS_CHAIN_SUPPORTED: bool = false;
    const EXECUTOR: SwapExecutorType = SwapExecutorType::ZeroExGasless;
    const GASLESS: bool = true;
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

    #[tracing::instrument(skip(self))]
    async fn get_quote_internal(
        &self,
        data: Self::GetQuoteParams,
    ) -> Result<Self::GetQuoteResponse, SwapsClientError> {
        // let price: ZeroExSwapPrice = self.send_request(
        //     "/gasless/price",
        //     reqwest::Method::GET,
        //     data.clone(),
        // ).await?;

        // let sell_amount = data.sell_amount - price.total_fees();
        // tracing::Span::current().record("total_fees_amount", price.total_fees());
        // tracing::Span::current().record("updated_sell_amount", sell_amount);

        // let data = Self::GetQuoteParams {
        //     sell_amount,
        //     ..data
        // };

        self.send_request(
            "/gasless/quote",
            reqwest::Method::GET,
            data,
        ).await
    }

    async fn sign_transaction_internal(
        &self,
        keyring_client: &KeyringClient,
        swap: &Swap,
    ) -> Result<String, SwapsClientError> {
        let details = self.extract_raw_details(swap.swap_details.raw_transaction.clone())?;

        let trade_signature = self
            .sign_hash(
                &details.raw_trade.hash,
                swap.request.invoice_id,
                keyring_client,
            )
            .await?;

        let signature = if let Some(approval) = details.approval.as_ref() {
            let approval_signature = self
                .sign_hash(
                    &approval.hash,
                    swap.request.invoice_id,
                    keyring_client,
                )
                .await?;

            format!("{trade_signature}|{approval_signature}")
        } else {
            trade_signature
        };

        Ok(signature)
    }

    async fn submit_transaction_internal(
        &self,
        data: &SwapDetails,
    ) -> Result<super::TransactionHash, SwapsClientError> {
        let signature = self.extract_signature(&data)?;
        let raw_details: ZeroExGaslessRawTransaction = data.raw_transaction.clone().try_into()?;

        let (approval, signature_bytes) = if let Some(approval) = raw_details.approval {
            // TODO: get rid of unwrap
            let (trade_signature, approval_signature) = signature.split_once("|").unwrap();

            let approval = SignedTrade {
                trade_type: approval.trade_type,
                eip712: approval.eip712,
                signature: TypedSignature {
                    signature_type: 5,
                    signature_bytes: approval_signature.to_string(),
                },
            };

            (Some(approval), trade_signature.to_string())
        } else {
            (None, signature.clone())
        };

        let params = SubmitTransactionRequest {
            chain_id: 137,
            trade: SignedTrade {
                trade_type: raw_details.raw_trade.trade_type,
                eip712: raw_details.raw_trade.eip712,
                signature: TypedSignature {
                    signature_type: 5,
                    signature_bytes,
                },
            },
            approval,
        };

        let result: SubmitTransactionResponse = self.send_request(
            "/gasless/submit",
            reqwest::Method::POST,
            params
        ).await?;

        Ok(result.trade_hash)
    }

    async fn get_transaction_status_internal(
        &self,
        data: &SwapDetails,
    ) -> Result<Self::SwapStatus, SwapsClientError> {
        let tx_hash = self.extract_transaction_hash(data)?;

        let url = format!("/gasless/status/{tx_hash}");

        let params = GetTransactionStatusRequest {
            chain_id: 137
        };

        let response: GetTransactionStatusResponse = self.send_request(
            &url,
            reqwest::Method::GET,
            params,
        ).await?;

        Ok(response.status.into())
    }
}
