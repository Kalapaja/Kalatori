mod types;

use std::time::Duration;

use kalatori_client::types::ChainType;
use reqwest::header::{
    HeaderMap,
    HeaderName,
    HeaderValue,
};
use secrecy::ExposeSecret;
use serde::de::DeserializeOwned;
use serde::{
    Deserialize,
    Serialize,
};

use types::*;

use crate::clients::RawSwapDetails;
use crate::clients::swaps::{
    SwapsClient,
    SwapsClientError,
};
use crate::configs::{
    BungeeApiConfig,
    IntegratorFees,
    SwapsConfig,
};
use crate::types::{
    SwapChainType,
    SwapDetails,
    SwapExecutorType,
};

// Use without API Key
const BUNGEE_PUBLIC_BASE_URL: &str = "https://public-backend.bungee.exchange";
// Use when have API Key
const BUNGEE_PRIVATE_BASE_URL: &str = "https://dedicated-backend.bungee.exchange";
const BUNGEE_CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

// Although it's a copy of `QuoteAutoRoute` structure, it's better
// to leave it as is. Otherwise we'll have to implement different
// `rename_all` for serialize and deserialize + this structure can be modified
// in future
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BungeeRawTransaction {
    pub quote_id: String,
    pub request_type: String,
    pub approval_data: ApprovalData,
    pub sign_typed_data: SignTypedData,
}

impl TryFrom<RawSwapDetails> for BungeeRawTransaction {
    type Error = SwapsClientError;

    fn try_from(value: RawSwapDetails) -> Result<Self, Self::Error> {
        let RawSwapDetails::Bungee(raw_transaction) = value else {
            return Err(SwapsClientError::WrongRawTransaction)
        };

        Ok(raw_transaction)
    }
}

pub type BungeeQuoteDetails = BungeeRawTransaction;

#[cfg(test)]
pub fn default_bungee_raw_transaction() -> BungeeRawTransaction {
    BungeeRawTransaction {
        quote_id: "68b79aeab92d6307".to_string(),
        request_type: "SWAP_REQUEST".to_string(),
        approval_data: ApprovalData {
            token_address: "0xc2132d05d31c914a87c6611c10748aeb04b58e8f".to_string(),
            spender_address: "0x000000000022D473030F116dDEE9F6B43aC78BA3".to_string(),
            user_address: "0xa4d353bbc130cbef1811f27ac70989f9d568ceab".to_string(),
            amount: "1500000".to_string(),
        },
        sign_typed_data: SignTypedData {
            domain: alloy::sol_types::Eip712Domain {
                name: Some("Permit2".into()),
                version: None,
                chain_id: Some(alloy::primitives::U256::from(137)),
                verifying_contract: Some(alloy::primitives::address!(
                    "0x000000000022d473030f116ddee9f6b43ac78ba3"
                )),
                salt: None,
            },
            types: serde_json::json!({
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
                        "name": "witness",
                        "type": "Request"
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
                "Request": [
                    {
                        "name": "basicReq",
                        "type": "BasicRequest"
                    },
                    {
                        "name": "metadata",
                        "type": "bytes32"
                    },
                    {
                        "name": "affiliateFees",
                        "type": "bytes"
                    },
                    {
                        "name": "minDestGas",
                        "type": "uint256"
                    },
                    {
                        "name": "destinationPayload",
                        "type": "bytes"
                    },
                    {
                        "name": "exclusiveTransmitter",
                        "type": "address"
                    }
                ],
                "BasicRequest": [
                    {
                        "name": "chainId",
                        "type": "uint256"
                    },
                    {
                        "name": "deadline",
                        "type": "uint256"
                    },
                    {
                        "name": "nonce",
                        "type": "uint256"
                    },
                    {
                        "name": "sender",
                        "type": "address"
                    },
                    {
                        "name": "receiver",
                        "type": "address"
                    },
                    {
                        "name": "bungeeGateway",
                        "type": "address"
                    },
                    {
                        "name": "inputToken",
                        "type": "address"
                    },
                    {
                        "name": "inputAmount",
                        "type": "uint256"
                    },
                    {
                        "name": "outputToken",
                        "type": "address"
                    },
                    {
                        "name": "minOutputAmount",
                        "type": "uint256"
                    }
                ]
            }),
            values: SignQuoteDataValues {
                deadline: 1774897293,
                nonce: 1774896693,
                permitted: Permitted {
                    amount: 1500000,
                    token: "0xc2132d05d31c914a87c6611c10748aeb04b58e8f".to_string(),
                },
                spender: "0x6dde7cf4e6a6f53f058bf5d2b4a54afbba11ee54".to_string(),
                witness: Witness {
                    affiliate_fees: "0x".to_string(),
                    basic_req: BasicRequest {
                        bungee_gateway: "0x6dde7cf4e6a6f53f058bf5d2b4a54afbba11ee54".to_string(),
                        chain_id: 137,
                        deadline: 1774897293,
                        input_amount: 1500000,
                        input_token: "0xc2132d05d31c914a87c6611c10748aeb04b58e8f".to_string(),
                        min_output_amount: 1487902,
                        nonce: 1774896693,
                        output_token: "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359".to_string(),
                        receiver: "0x0e3ca7fd040144900adaa5f9b8917f3933a4f5e9".to_string(),
                        sender: "0xa4d353bbc130cbef1811f27ac70989f9d568ceab".to_string(),
                    },
                    destination_payload: "0x".to_string(),
                    exclusive_transmitter: "0x0000000000000000000000000000000000000000".to_string(),
                    metadata: "0x68b79aeab92d6307000000000000000000000000000000000000000000002713"
                        .to_string(),
                    min_dest_gas: 0,
                },
            },
        },
    }
}

#[expect(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BungeeApiResponse<T> {
    // result is empty in case of error
    result: Option<T>,
    success: bool,
    #[serde(default)]
    message: Option<String>,
    #[expect(dead_code)]
    #[serde(default)]
    status_code: Option<u32>,
}

impl<T> From<BungeeApiResponse<T>> for SwapsClientError {
    fn from(_value: BungeeApiResponse<T>) -> Self {
        Self::UnknownApiError
    }
}

#[derive(Clone)]
pub struct BungeeClient {
    client: reqwest::Client,
    #[expect(dead_code)]
    fees: Option<IntegratorFees>,
    api_access: Option<BungeeApiConfig>,
    base_url: String,
}

impl BungeeClient {
    pub fn new(config: &SwapsConfig) -> Self {
        let base_url = if config.bungee.is_some() {
            BUNGEE_PRIVATE_BASE_URL
        } else {
            BUNGEE_PUBLIC_BASE_URL
        }
        .to_string();

        Self {
            client: reqwest::Client::new(),
            fees: config.fees.clone(),
            api_access: config.bungee.clone(),
            base_url,
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
        let full_url = format!("{}{}", self.base_url, url);

        let request = self
            .client
            .request(method.clone(), full_url)
            .timeout(BUNGEE_CLIENT_REQUEST_TIMEOUT);

        let request = if let reqwest::Method::POST = method {
            request.json(&params)
        } else {
            request.query(&params)
        };

        let request = if let Some(api_access) = self.api_access.as_ref() {
            request.headers(HeaderMap::from_iter([
                (
                    HeaderName::from_static("x-api-key"),
                    HeaderValue::from_str(api_access.api_key.expose_secret()).unwrap(),
                ),
                (
                    HeaderName::from_static("affiliate"),
                    HeaderValue::from_str(api_access.affiliate.expose_secret()).unwrap(),
                ),
            ]))
        } else {
            request
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

            SwapsClientError::UnknownApiError
        })?;

        tracing::trace!(
            ?response,
            "Got parsed response from Bungee"
        );

        match response.result {
            Some(result) if response.success => Ok(result),
            _ => Err(response.into()),
        }
    }
}

impl SwapsClient for BungeeClient {
    type GetQuoteParams = QuoteRequest;
    type GetQuoteResponse = QuoteResponse;
    type RawTransactionDetails = BungeeRawTransaction;
    type SwapStatus = BungeeSwapStatus;

    // TODO: bungee actually can make a cross-chain swaps but it requires API our
    // modifications
    const CROSS_CHAIN_SUPPORTED: bool = false;
    const EXECUTOR: SwapExecutorType = SwapExecutorType::Bungee;
    const GASLESS: bool = false;
    const SINGLE_CHAIN_SUPPORTED: bool = true;
    const SUPPORTED_CHAINS: &[ChainType] = &[ChainType::Polygon];
    // https://docs.bungee.exchange/overview/chain-support
    const SUPPORTED_SWAP_CHAINS: &[SwapChainType] = &[
        SwapChainType::Arbitrum,
        SwapChainType::Avalanche,
        SwapChainType::Base,
        SwapChainType::Berachain,
        SwapChainType::BnbSmartChain,
        SwapChainType::Blast,
        SwapChainType::Ethereum,
        // SwapChainType::Fantom,
        // SwapChainType::Gnosis,
        SwapChainType::HyperEvm,
        SwapChainType::Ink,
        // SwapChainType::Katana,
        SwapChainType::Linea,
        SwapChainType::Mantle,
        SwapChainType::MegaEth,
        SwapChainType::Mode,
        SwapChainType::Monad,
        SwapChainType::Optimism,
        SwapChainType::Plasma,
        // SwapChainType::Plume,
        SwapChainType::Polygon,
        SwapChainType::Scroll,
        // SwapChainType::Sei,
        SwapChainType::Solana,
        SwapChainType::Soneium,
        SwapChainType::Sonic,
        // SwapChainType::Tron,
        SwapChainType::Unichain,
        SwapChainType::WorldChain,
    ];

    async fn get_quote_internal(
        &self,
        data: Self::GetQuoteParams,
    ) -> Result<Self::GetQuoteResponse, SwapsClientError> {
        self.send_request(
            "/api/v1/bungee/quote",
            reqwest::Method::GET,
            data,
        )
        .await
        // TODO: check if `auto_quote` is empty, if so return an error
    }

    async fn submit_transaction_internal(
        &self,
        data: &SwapDetails,
    ) -> Result<super::TransactionHash, SwapsClientError> {
        let params: SubmitOrderRequest = data.clone().try_into()?;

        let result: SubmitOrderResponse = self
            .send_request(
                "/api/v1/bungee/submit",
                reqwest::Method::POST,
                params,
            )
            .await?;

        Ok(result.request_hash)
    }

    async fn get_transaction_status_internal(
        &self,
        data: &SwapDetails,
    ) -> Result<Self::SwapStatus, SwapsClientError> {
        let params = GetSwapStatusRequest {
            // TODO: clone can be avoided
            request_hash: self
                .extract_transaction_hash(data)?
                .clone(),
        };

        let result: Vec<GetSwapStatusResponse> = self
            .send_request(
                "/api/v1/bungee/status",
                reqwest::Method::GET,
                params,
            )
            .await?;

        let Some(trans) = result.first() else {
            return Err(SwapsClientError::UnknownApiError)
        };

        Ok(trans.bungee_status_code)
    }
}

#[cfg(test)]
mod tests {
    use httpmock::prelude::*;
    use uuid::Uuid;

    use crate::clients::{
        ExecutorSwapStatus,
        default_zero_ex_raw_transaction,
    };
    use crate::types::{
        CreateSwapData,
        SwapDirection,
        SwapQuote,
    };

    use super::*;

    #[test]
    fn test_try_from_raw_details() {
        let bungee_details = RawSwapDetails::Bungee(default_bungee_raw_transaction());
        let result = BungeeRawTransaction::try_from(bungee_details);
        assert_eq!(
            result,
            Ok(default_bungee_raw_transaction())
        );

        let non_across_details = RawSwapDetails::ZeroEx(default_zero_ex_raw_transaction());
        let result = BungeeRawTransaction::try_from(non_across_details);
        assert_eq!(
            result,
            Err(SwapsClientError::WrongRawTransaction)
        );
    }

    #[tokio::test]
    async fn test_get_quote() {
        let server = MockServer::start();
        let config = SwapsConfig::default();
        let mut client = BungeeClient::new(&config);
        client.base_url = server.base_url();

        let data = CreateSwapData {
            invoice_id: Uuid::new_v4(),
            swap_executor: SwapExecutorType::Bungee,
            from_chain: SwapChainType::Polygon,
            to_chain: SwapChainType::Polygon,
            from_token_address: "0xc2132D05D31c914a87C6611C10748AEb04B58e8F".to_string(),
            to_token_address: "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359".to_string(),
            from_amount_units: 1_500_000,
            // not important, shouldn't be used in request
            expected_to_amount_units: 0,
            from_address: "0xA4d353BBc130cbeF1811f27ac70989F9d568CeAB".to_string(),
            to_address: "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9".to_string(),
            direction: SwapDirection::Incoming,
            origin: Default::default(),
        };

        let mock = server.mock(|when, then| {
            when
                .method(GET)
                .path("/api/v1/bungee/quote")
                .query_param("userAddress", "0xA4d353BBc130cbeF1811f27ac70989F9d568CeAB")
                .query_param("originChainId", "137")
                .query_param("destinationChainId", "137")
                .query_param("inputToken", "0xc2132D05D31c914a87C6611C10748AEb04B58e8F")
                .query_param("inputAmount", "1500000")
                .query_param("receiverAddress", "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9")
                .query_param("outputToken", "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359");

            then
                .json_body(serde_json::json!({
                    "success": true,
                    "statusCode": 200,
                    "result": {
                        "originChainId": 137,
                        "destinationChainId": 137,
                        "userAddress": "0xa4d353bbc130cbef1811f27ac70989f9d568ceab",
                        "receiverAddress": "0x0e3ca7fd040144900adaa5f9b8917f3933a4f5e9",
                        "input": {
                            "token": {
                                "chainId": 137,
                                "address": "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
                                "name": "USDT0",
                                "symbol": "USDT0",
                                "decimals": 6,
                                "logoURI": "https://assets.coingecko.com/coins/images/53705/large/usdt0.jpg?1737086183",
                                "icon": "https://assets.coingecko.com/coins/images/53705/large/usdt0.jpg?1737086183"
                            },
                            "amount": "1500000",
                            "priceInUsd": 0.999205,
                            "valueInUsd": 1.4988075
                        },
                        "autoRoute": {
                            "userOp": "sign",
                            "requestHash": "0xb3f6b5da29e33d67e5f6b8d984783a3253a132889750374ce8da8e30300ef5ac",
                            "output": {
                                "token": {
                                    "chainId": 137,
                                    "address": "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
                                    "name": "USDC",
                                    "symbol": "USDC",
                                    "decimals": 6,
                                    "logoURI": "https://assets.coingecko.com/coins/images/6319/large/USDC.png?1769615602",
                                    "icon": "https://assets.coingecko.com/coins/images/6319/large/USDC.png?1769615602"
                                },
                                "priceInUsd": 1,
                                "valueInUsd": 1.502116,
                                "minAmountOut": "1487902",
                                "amount": "1502116",
                                "effectiveAmount": "1492380",
                                "effectiveValueInUsd": 1.49238,
                                "effectiveReceivedInUsd": 1.502116
                            },
                            "requestType": "SWAP_REQUEST",
                            "approvalData": {
                                "spenderAddress": "0x000000000022D473030F116dDEE9F6B43aC78BA3",
                                "amount": "1500000",
                                "tokenAddress": "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
                                "userAddress": "0xa4d353bbc130cbef1811f27ac70989f9d568ceab"
                            },
                            "affiliateFee": null,
                            "signTypedData": {
                                "domain": {
                                    "name": "Permit2",
                                    "chainId": 137,
                                    "verifyingContract": "0x000000000022D473030F116dDEE9F6B43aC78BA3"
                                },
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
                                            "name": "witness",
                                            "type": "Request"
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
                                    "Request": [
                                        {
                                            "name": "basicReq",
                                            "type": "BasicRequest"
                                        },
                                        {
                                            "name": "metadata",
                                            "type": "bytes32"
                                        },
                                        {
                                            "name": "affiliateFees",
                                            "type": "bytes"
                                        },
                                        {
                                            "name": "minDestGas",
                                            "type": "uint256"
                                        },
                                        {
                                            "name": "destinationPayload",
                                            "type": "bytes"
                                        },
                                        {
                                            "name": "exclusiveTransmitter",
                                            "type": "address"
                                        }
                                    ],
                                    "BasicRequest": [
                                        {
                                            "name": "chainId",
                                            "type": "uint256"
                                        },
                                        {
                                            "name": "deadline",
                                            "type": "uint256"
                                        },
                                        {
                                            "name": "nonce",
                                            "type": "uint256"
                                        },
                                        {
                                            "name": "sender",
                                            "type": "address"
                                        },
                                        {
                                            "name": "receiver",
                                            "type": "address"
                                        },
                                        {
                                            "name": "bungeeGateway",
                                            "type": "address"
                                        },
                                        {
                                            "name": "inputToken",
                                            "type": "address"
                                        },
                                        {
                                            "name": "inputAmount",
                                            "type": "uint256"
                                        },
                                        {
                                            "name": "outputToken",
                                            "type": "address"
                                        },
                                        {
                                            "name": "minOutputAmount",
                                            "type": "uint256"
                                        }
                                    ]
                                },
                                "values": {
                                    "permitted": {
                                        "token": "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
                                        "amount": "1500000"
                                    },
                                    "spender": "0x6dde7cf4e6a6f53f058bf5d2b4a54afbba11ee54",
                                    "nonce": "1774896693",
                                    "deadline": "1774897293",
                                    "witness": {
                                        "basicReq": {
                                            "chainId": 137,
                                            "deadline": "1774897293",
                                            "nonce": "1774896693",
                                            "sender": "0xa4d353bbc130cbef1811f27ac70989f9d568ceab",
                                            "receiver": "0x0e3ca7fd040144900adaa5f9b8917f3933a4f5e9",
                                            "bungeeGateway": "0x6dde7cf4e6a6f53f058bf5d2b4a54afbba11ee54",
                                            "inputToken": "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
                                            "inputAmount": "1500000",
                                            "outputToken": "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
                                            "minOutputAmount": "1487902"
                                        },
                                        "exclusiveTransmitter": "0x0000000000000000000000000000000000000000",
                                        "metadata": "0x68b79aeab92d6307000000000000000000000000000000000000000000002713",
                                        "affiliateFees": "0x",
                                        "minDestGas": "0",
                                        "destinationPayload": "0x"
                                    }
                                }
                            },
                            "gasFee": null,
                            "slippage": 0.3,
                            "suggestedClientSlippage": 0.3,
                            "txData": null,
                            "estimatedTime": 10,
                            "routeDetails": {
                                "name": "Bungee Protocol",
                                "logoURI": "",
                                "routeFee": null,
                                "dexDetails": null
                            },
                            "refuel": null,
                            "quoteId": "68b79aeab92d6307",
                            "quoteExpiry": 1774896753
                        },
                        "depositRoute": null
                    },
                    "message": null
                }));
        });

        let expected_response = SwapQuote {
            swap_executor: SwapExecutorType::Bungee,
            id: "68b79aeab92d6307".to_string(),
            estimated_to_amount_units: 1487902,
            estimated_to_amount: rust_decimal::Decimal::ZERO,
            valid_till: chrono::DateTime::parse_from_rfc3339("2026-03-30T19:01:33Z")
                .unwrap()
                .to_utc(),
            quote_details: RawSwapDetails::Bungee(BungeeRawTransaction {
                quote_id: "68b79aeab92d6307".to_string(),
                request_type: "SWAP_REQUEST".to_string(),
                approval_data: ApprovalData {
                    token_address: "0xc2132d05d31c914a87c6611c10748aeb04b58e8f".to_string(),
                    spender_address: "0x000000000022D473030F116dDEE9F6B43aC78BA3".to_string(),
                    user_address: "0xa4d353bbc130cbef1811f27ac70989f9d568ceab".to_string(),
                    amount: "1500000".to_string(),
                },
                sign_typed_data: SignTypedData {
                    domain: alloy::sol_types::Eip712Domain {
                        name: Some("Permit2".into()),
                        version: None,
                        chain_id: Some(alloy::primitives::U256::from(137)),
                        verifying_contract: Some(alloy::primitives::address!(
                            "0x000000000022d473030f116ddee9f6b43ac78ba3"
                        )),
                        salt: None,
                    },
                    types: serde_json::json!({
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
                                "name": "witness",
                                "type": "Request"
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
                        "Request": [
                            {
                                "name": "basicReq",
                                "type": "BasicRequest"
                            },
                            {
                                "name": "metadata",
                                "type": "bytes32"
                            },
                            {
                                "name": "affiliateFees",
                                "type": "bytes"
                            },
                            {
                                "name": "minDestGas",
                                "type": "uint256"
                            },
                            {
                                "name": "destinationPayload",
                                "type": "bytes"
                            },
                            {
                                "name": "exclusiveTransmitter",
                                "type": "address"
                            }
                        ],
                        "BasicRequest": [
                            {
                                "name": "chainId",
                                "type": "uint256"
                            },
                            {
                                "name": "deadline",
                                "type": "uint256"
                            },
                            {
                                "name": "nonce",
                                "type": "uint256"
                            },
                            {
                                "name": "sender",
                                "type": "address"
                            },
                            {
                                "name": "receiver",
                                "type": "address"
                            },
                            {
                                "name": "bungeeGateway",
                                "type": "address"
                            },
                            {
                                "name": "inputToken",
                                "type": "address"
                            },
                            {
                                "name": "inputAmount",
                                "type": "uint256"
                            },
                            {
                                "name": "outputToken",
                                "type": "address"
                            },
                            {
                                "name": "minOutputAmount",
                                "type": "uint256"
                            }
                        ]
                    }),
                    values: SignQuoteDataValues {
                        deadline: 1774897293,
                        nonce: 1774896693,
                        permitted: Permitted {
                            amount: 1500000,
                            token: "0xc2132d05d31c914a87c6611c10748aeb04b58e8f".to_string(),
                        },
                        spender: "0x6dde7cf4e6a6f53f058bf5d2b4a54afbba11ee54".to_string(),
                        witness: Witness {
                            affiliate_fees: "0x".to_string(),
                            basic_req: BasicRequest {
                                bungee_gateway: "0x6dde7cf4e6a6f53f058bf5d2b4a54afbba11ee54"
                                    .to_string(),
                                chain_id: 137,
                                deadline: 1774897293,
                                input_amount: 1500000,
                                input_token: "0xc2132d05d31c914a87c6611c10748aeb04b58e8f"
                                    .to_string(),
                                min_output_amount: 1487902,
                                nonce: 1774896693,
                                output_token: "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359"
                                    .to_string(),
                                receiver: "0x0e3ca7fd040144900adaa5f9b8917f3933a4f5e9".to_string(),
                                sender: "0xa4d353bbc130cbef1811f27ac70989f9d568ceab".to_string(),
                            },
                            destination_payload: "0x".to_string(),
                            exclusive_transmitter: "0x0000000000000000000000000000000000000000"
                                .to_string(),
                            metadata:
                                "0x68b79aeab92d6307000000000000000000000000000000000000000000002713"
                                    .to_string(),
                            min_dest_gas: 0,
                        },
                    },
                },
            }),
        };

        let response = client.get_quote(data).await.unwrap();
        assert_eq!(response, expected_response);
        mock.assert();
    }

    #[tokio::test]
    async fn test_submit_transaction() {
        let server = MockServer::start();
        let config = SwapsConfig::default();
        let mut client = BungeeClient::new(&config);
        client.base_url = server.base_url();

        let data = SwapDetails {
            id: "68b79aeab92d6307".to_string(),
            raw_transaction: RawSwapDetails::Bungee(default_bungee_raw_transaction()),
            signature: Some("SIGNATURE".to_string()),
            transaction_hash: None,
        };

        let mock = server.mock(|when, then| {
            when
                .method(POST)
                .path("/api/v1/bungee/submit")
                .json_body(serde_json::json!({
                    "requestType": "SWAP_REQUEST",
                    "request": {
                        "affiliateFees": "0x",
                        "basicReq": {
                            "bungeeGateway": "0x6dde7cf4e6a6f53f058bf5d2b4a54afbba11ee54",
                            "chainId": 137,
                            "deadline": "1774897293",
                            "inputAmount": "1500000",
                            "inputToken": "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
                            "minOutputAmount": "1487902",
                            "nonce": "1774896693",
                            "outputToken": "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
                            "receiver": "0x0e3ca7fd040144900adaa5f9b8917f3933a4f5e9",
                            "sender": "0xa4d353bbc130cbef1811f27ac70989f9d568ceab"
                        },
                        "destinationPayload": "0x",
                        "exclusiveTransmitter": "0x0000000000000000000000000000000000000000",
                        "metadata": "0x68b79aeab92d6307000000000000000000000000000000000000000000002713",
                        "minDestGas": "0"
                    },
                    "userSignature": "SIGNATURE",
                    "quoteId": "68b79aeab92d6307"
                }));

            then
                .json_body(serde_json::json!({
                    "success": true,
                    "statusCode": 200,
                    // TODO: most of this data isn't used and was copied from example.
                    // But example doesn't seem to be valid and has differences with specification
                    "result": {
                        "requestHash": "0x1f4b45dbb7adba26d723ff1e19af9bcbbb047e29e34fa8e39c1c4d92abbfe3a2",
                        "originData": {
                            "input": [
                                {
                                    "token": {
                                        "chainId": 137,
                                        "address": "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
                                        "name": "USDT0",
                                        "symbol": "USDT0",
                                        "decimals": 6,
                                        "logoURI": "https://assets.coingecko.com/coins/images/53705/large/usdt0.jpg?1737086183",
                                        "icon": "https://assets.coingecko.com/coins/images/53705/large/usdt0.jpg?1737086183"
                                    },
                                    "amount": "1500000",
                                    "priceInUsd": 0.999205,
                                    "valueInUsd": 1.4988075
                                }
                            ],
                            "originChainId": 137,
                            "txHash": null,
                            "status": "PENDING",
                            "userAddress": "0xa4d353bbc130cbef1811f27ac70989f9d568ceab"
                        },
                        "destinationData": {
                            "output": [
                                {
                                    "token": {
                                        "chainId": 137,
                                        "address": "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
                                        "name": "USDC",
                                        "symbol": "USDC",
                                        "decimals": 6,
                                        "logoURI": "https://assets.coingecko.com/coins/images/6319/large/USDC.png?1769615602",
                                        "icon": "https://assets.coingecko.com/coins/images/6319/large/USDC.png?1769615602"
                                    },
                                    "priceInUsd": 1,
                                    "valueInUsd": 1.502116,
                                    "minAmountOut": "1487902",
                                    "amount": "1502116",
                                    "effectiveAmount": "1492380",
                                    "effectiveValueInUsd": 1.49238,
                                    "effectiveReceivedInUsd": 1.502116
                                }
                            ],
                            "txHash": null,
                            "destinationChainId": 137,
                            "receiverAddress": "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9",
                            "status": "PENDING"
                        },
                        "routeDetails": {
                            "name": "bungee-protocol",
                            "logoURI": "https://media.socket.tech/bungee.svg"
                        },
                        "bungeeStatusCode": 0
                    }
                }));
        });

        let expected_response =
            "0x1f4b45dbb7adba26d723ff1e19af9bcbbb047e29e34fa8e39c1c4d92abbfe3a2".to_string();

        let response = client
            .submit_transaction(&data)
            .await
            .unwrap();
        assert_eq!(response, expected_response);
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_transaction_status() {
        let server = MockServer::start();
        let config = SwapsConfig::default();
        let mut client = BungeeClient::new(&config);
        client.base_url = server.base_url();

        let data = SwapDetails {
            id: "68b79aeab92d6307".to_string(),
            raw_transaction: RawSwapDetails::Bungee(default_bungee_raw_transaction()),
            signature: Some("SIGNATURE".to_string()),
            transaction_hash: Some(
                "0x1f4b45dbb7adba26d723ff1e19af9bcbbb047e29e34fa8e39c1c4d92abbfe3a2".to_string(),
            ),
        };

        let mock = server.mock(|when, then| {
            when
                .method(GET)
                .path("/api/v1/bungee/status")
                .query_param("requestHash", "0x1f4b45dbb7adba26d723ff1e19af9bcbbb047e29e34fa8e39c1c4d92abbfe3a2");

            then
                .json_body(serde_json::json!({
                    "success": true,
                    "statusCode": 200,
                    "result": [
                        {
                            "hash": "0x1f4b45dbb7adba26d723ff1e19af9bcbbb047e29e34fa8e39c1c4d92abbfe3a2",
                            "originData": {
                                "input": [
                                    {
                                        "token": {
                                            "chainId": 137,
                                            "address": "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
                                            "name": "USDT0",
                                            "symbol": "USDT0",
                                            "decimals": 6,
                                            "logoURI": "https://assets.coingecko.com/coins/images/53705/large/usdt0.jpg?1737086183",
                                            "icon": "https://assets.coingecko.com/coins/images/53705/large/usdt0.jpg?1737086183"
                                        },
                                        "amount": "1500000",
                                        "priceInUsd": 0.999205,
                                        "valueInUsd": 1.4988075
                                    }
                                ],
                                "originChainId": 137,
                                "txHash": null,
                                "status": "PENDING",
                                "userAddress": "0xa4d353bbc130cbef1811f27ac70989f9d568ceab"
                            },
                            "destinationData": {
                                "output": [
                                    {
                                        "token": {
                                            "chainId": 137,
                                            "address": "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
                                            "name": "USDC",
                                            "symbol": "USDC",
                                            "decimals": 6,
                                            "logoURI": "https://assets.coingecko.com/coins/images/6319/large/USDC.png?1769615602",
                                            "icon": "https://assets.coingecko.com/coins/images/6319/large/USDC.png?1769615602"
                                        },
                                        "priceInUsd": 1,
                                        "valueInUsd": 1.502116,
                                        "minAmountOut": "1487902",
                                        "amount": "1502116",
                                        "effectiveAmount": "1492380",
                                        "effectiveValueInUsd": 1.49238,
                                        "effectiveReceivedInUsd": 1.502116
                                    }
                                ],
                                "txHash": null,
                                "destinationChainId": 10,
                                "receiverAddress": "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9",
                                "status": "PENDING"
                            },
                            "routeDetails": {
                                "name": "bungee-protocol",
                                "logoURI": "https://media.socket.tech/bungee.svg"
                            },
                            "bungeeStatusCode": 3,
                            "refund": null
                        }
                    ]
                }));
        });

        let expected_response = ExecutorSwapStatus::Executed;

        let response = client
            .get_transaction_status(&data)
            .await
            .unwrap();
        assert_eq!(response, expected_response);
        mock.assert();
    }
}
