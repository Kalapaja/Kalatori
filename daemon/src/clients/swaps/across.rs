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
            gas: Some(100),
            max_fee_per_gas: Some(100),
            max_priority_fee_per_gas: Some(100),
        },
        approval_transactions: Vec::new(),
    }
}

pub type AcrossQuoteDetails = AcrossRawTransaction;

impl AcrossApiError {
    /// Across usually embeds the status in the error body, but a gateway
    /// failure (502/503) answers with a bare `{"message": …}` and no `status`.
    /// Falling back to the transport status keeps those from reaching the payer
    /// as a rejection they could act on. Mirrors the 0x client, which threads
    /// the HTTP status in for the same reason.
    fn into_client_error(
        self,
        status: reqwest::StatusCode,
    ) -> SwapsClientError {
        tracing::warn!(
            %status,
            error.source = ?self,
            "Across API returned an error response"
        );

        let is_server_error = self.status.map_or_else(
            || status.is_server_error(),
            |embedded| embedded >= 500,
        );

        // Provider-side failures are not the requester's fault
        if is_server_error {
            return SwapsClientError::UnknownApiError;
        }

        SwapsClientError::ProviderRejected {
            message: self.message,
        }
    }
}

#[derive(Clone)]
pub struct AcrossClient {
    client: reqwest::Client,
    #[expect(dead_code)]
    fees: Option<IntegratorFees>,
    base_url: String,
}

impl AcrossClient {
    pub fn new(swaps_config: &SwapsConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            fees: swaps_config.fees.clone(),
            base_url: ACROSS_BASE_URL.to_string(),
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
        let full_url = format!("{}{}", self.base_url, url);

        let response = self
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
            })?;

        // Captured before the body is consumed: an Across gateway failure omits
        // the embedded `status`, leaving the transport status the only signal
        // that this was their outage rather than a bad request.
        let http_status = response.status();
        let raw_response = response.text().await?;

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
            AcrossApiResponse::Err(e) => Err(e.into_client_error(http_status)),
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
    // https://docs.across.to/chains-and-contracts
    const SUPPORTED_SWAP_CHAINS: &[SwapChainType] = &[
        SwapChainType::Ethereum,
        SwapChainType::Arbitrum,
        SwapChainType::Base,
        SwapChainType::Blast,
        SwapChainType::BnbSmartChain,
        SwapChainType::HyperEvm,
        SwapChainType::Ink,
        SwapChainType::Lens,
        SwapChainType::Linea,
        SwapChainType::Lisk,
        SwapChainType::MegaEth,
        SwapChainType::Mode,
        SwapChainType::Monad,
        SwapChainType::Optimism,
        SwapChainType::Plasma,
        SwapChainType::Polygon,
        SwapChainType::Scroll,
        SwapChainType::Solana,
        SwapChainType::Soneium,
        SwapChainType::Tempo,
        SwapChainType::Unichain,
        SwapChainType::WorldChain,
        SwapChainType::ZkSync,
        SwapChainType::Zora,
    ];

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
        let deposit_txn_ref = self.extract_transaction_hash(data)?;

        let params = SwapStatusRequest {
            deposit_txn_ref,
        };

        let result: SwapStatusResponse = self
            .send_request("/api/deposit/status", params)
            .await?;

        Ok(result.status)
    }
}

#[cfg(test)]
mod tests {
    use httpmock::MockServer;
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
    fn test_swap_approval_response_tolerates_null_approval_txns() {
        // Trimmed from the production response that broke 0.9.3 (2026-08-03):
        // Across sends "approvalTxns": null — not an absent key — whenever the
        // payer already holds allowance, which made every ready-to-pay wallet
        // fail the untagged AcrossApiResponse parse and surface a blank 500.
        let raw = r#"{
            "inputAmount": "2010000",
            "maxInputAmount": "2010000",
            "expectedOutputAmount": "2003496",
            "approvalTxns": null,
            "swapTx": {
                "simulationSuccess": true,
                "chainId": 1,
                "to": "0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5",
                "data": "0xad5425c6",
                "value": "0",
                "gas": "571750",
                "maxFeePerGas": "1094950",
                "maxPriorityFeePerGas": "1000000"
            },
            "id": "cx44b-1785763721048-522b47d271d9",
            "quoteExpiryTimestamp": 1785764021
        }"#;

        let AcrossApiResponse::Ok(parsed) =
            serde_json::from_str::<AcrossApiResponse<SwapApprovalResponse>>(raw).unwrap()
        else {
            panic!("null approvalTxns must parse as the Ok variant, not AcrossApiError");
        };
        assert!(parsed.approval_txns.is_none());

        let quote = SwapQuote::try_from(parsed).unwrap();
        let RawSwapDetails::Across(details) = quote.quote_details else {
            panic!("expected Across quote details");
        };
        assert!(details.approval_transactions.is_empty());

        // An absent key must keep working too.
        let raw_absent = raw.replace("\"approvalTxns\": null,", "");
        let AcrossApiResponse::Ok(parsed) =
            serde_json::from_str::<AcrossApiResponse<SwapApprovalResponse>>(&raw_absent).unwrap()
        else {
            panic!("absent approvalTxns must parse as the Ok variant");
        };
        assert!(parsed.approval_txns.is_none());
    }

    #[test]
    fn test_swap_approval_response_passes_through_missing_gas_parameters() {
        for raw in [
            r#"{
                "inputAmount": "1",
                "maxInputAmount": "1",
                "expectedOutputAmount": "1",
                "swapTx": {
                    "simulationSuccess": true,
                    "chainId": 137,
                    "to": "0x1111111111111111111111111111111111111111",
                    "data": "0x1234",
                    "value": null,
                    "gas": null,
                    "maxFeePerGas": null
                },
                "id": "quote-benign-null",
                "quoteExpiryTimestamp": 1893456000
            }"#,
            r#"{
                "inputAmount": "1",
                "maxInputAmount": "1",
                "expectedOutputAmount": "1",
                "swapTx": {
                    "simulationSuccess": true,
                    "chainId": 137,
                    "to": "0x1111111111111111111111111111111111111111",
                    "data": "0x1234",
                    "maxPriorityFeePerGas": null
                },
                "id": "quote-benign-absent",
                "quoteExpiryTimestamp": 1893456000
            }"#,
        ] {
            let AcrossApiResponse::Ok(parsed) =
                serde_json::from_str::<AcrossApiResponse<SwapApprovalResponse>>(raw).unwrap()
            else {
                panic!("nullable swap transaction fields must parse as the success arm");
            };
            assert!(parsed.swap_tx.value.is_none());
            assert!(parsed.swap_tx.gas.is_none());
            assert!(parsed.swap_tx.max_fee_per_gas.is_none());
            assert!(
                parsed
                    .swap_tx
                    .max_priority_fee_per_gas
                    .is_none()
            );

            // Absent gas parameters are published as absent: Kassette
            // estimates them in the payer's wallet. Substituting 0 would
            // publish a transaction that cannot be mined, so the key must be
            // missing from the JSON entirely — not `null`, not `"0"`.
            let quote = SwapQuote::try_from(parsed).unwrap();
            let RawSwapDetails::Across(details) = quote.quote_details else {
                panic!("expected Across quote details");
            };

            assert!(details.transaction.gas.is_none());
            assert!(
                details
                    .transaction
                    .max_fee_per_gas
                    .is_none()
            );
            assert!(
                details
                    .transaction
                    .max_priority_fee_per_gas
                    .is_none()
            );
            // Across documents an absent `value` as zero, and Kassette
            // defaults it to `0n` — unlike gas, it is not omitted.
            assert_eq!(details.transaction.value, 0);

            let serialized = serde_json::to_value(&details.transaction).unwrap();
            assert!(serialized.get("gas").is_none());
            assert!(
                serialized
                    .get("max_fee_per_gas")
                    .is_none()
            );
            assert!(
                serialized
                    .get("max_priority_fee_per_gas")
                    .is_none()
            );
        }
    }

    #[test]
    fn test_across_zero_gas_and_fee_cap_are_omitted() {
        // Based on the production shape returned when an ERC-20 approval is
        // still missing, where Across sends `gas: "0"`. The fee caps and
        // `value` are normally absent from that response; they are supplied
        // here on purpose, so one fixture exercises zero-cap normalization and
        // the genuine-zero `value` alongside the gas sentinel.
        let raw = r#"{
            "inputAmount": "1",
            "maxInputAmount": "1",
            "expectedOutputAmount": "1",
            "swapTx": {
                "simulationSuccess": false,
                "chainId": 137,
                "to": "0x1111111111111111111111111111111111111111",
                "data": "0x1234",
                "value": "0",
                "gas": "0",
                "maxFeePerGas": "0",
                "maxPriorityFeePerGas": "1500000000"
            },
            "id": "quote-zero-sentinels",
            "quoteExpiryTimestamp": 1893456000
        }"#;

        let AcrossApiResponse::Ok(parsed) =
            serde_json::from_str::<AcrossApiResponse<SwapApprovalResponse>>(raw).unwrap()
        else {
            panic!("zero gas sentinels must parse as the success arm");
        };
        assert_eq!(parsed.swap_tx.gas, Some(0));
        assert_eq!(parsed.swap_tx.max_fee_per_gas, Some(0));
        assert_eq!(
            parsed.swap_tx.max_priority_fee_per_gas,
            Some(1_500_000_000)
        );
        assert_eq!(parsed.swap_tx.value, Some(0));

        let quote = SwapQuote::try_from(parsed).unwrap();
        let RawSwapDetails::Across(details) = quote.quote_details else {
            panic!("expected Across quote details");
        };
        assert!(details.transaction.gas.is_none());
        assert!(
            details
                .transaction
                .max_fee_per_gas
                .is_none()
        );
        assert!(
            details
                .transaction
                .max_priority_fee_per_gas
                .is_none()
        );
        assert_eq!(details.transaction.value, 0);

        let serialized = serde_json::to_value(&details.transaction).unwrap();
        assert!(serialized.get("gas").is_none());
        assert!(
            serialized
                .get("max_fee_per_gas")
                .is_none()
        );
        assert!(
            serialized
                .get("max_priority_fee_per_gas")
                .is_none()
        );
        assert_eq!(
            serialized.get("value"),
            Some(&serde_json::json!("0"))
        );
    }

    #[test]
    fn test_across_preserves_non_zero_gas_and_zero_priority_fee() {
        let raw = r#"{
            "inputAmount": "1",
            "maxInputAmount": "1",
            "expectedOutputAmount": "1",
            "swapTx": {
                "simulationSuccess": true,
                "chainId": 137,
                "to": "0x1111111111111111111111111111111111111111",
                "data": "0x1234",
                "gas": "21000",
                "maxFeePerGas": "30000000000",
                "maxPriorityFeePerGas": "0"
            },
            "id": "quote-valid-zero-priority",
            "quoteExpiryTimestamp": 1893456000
        }"#;

        let AcrossApiResponse::Ok(parsed) =
            serde_json::from_str::<AcrossApiResponse<SwapApprovalResponse>>(raw).unwrap()
        else {
            panic!("valid gas parameters must parse as the success arm");
        };
        let quote = SwapQuote::try_from(parsed).unwrap();
        let RawSwapDetails::Across(details) = quote.quote_details else {
            panic!("expected Across quote details");
        };

        assert_eq!(details.transaction.gas, Some(21_000));
        assert_eq!(
            details.transaction.max_fee_per_gas,
            Some(30_000_000_000)
        );
        assert_eq!(
            details
                .transaction
                .max_priority_fee_per_gas,
            Some(0)
        );
        // The same zero value results from an omitted provider field.
        assert_eq!(details.transaction.value, 0);

        let serialized = serde_json::to_value(&details.transaction).unwrap();
        assert_eq!(
            serialized.get("gas"),
            Some(&serde_json::json!("21000"))
        );
        assert_eq!(
            serialized.get("max_fee_per_gas"),
            Some(&serde_json::json!("30000000000"))
        );
        assert_eq!(
            serialized.get("max_priority_fee_per_gas"),
            Some(&serde_json::json!("0"))
        );
        assert_eq!(
            serialized.get("value"),
            Some(&serde_json::json!("0"))
        );
    }

    #[test]
    fn test_across_status_growth_degrades_safely() {
        // The variant is asserted alongside the mapping on purpose: asserting
        // only the `ExecutorSwapStatus` would stay green if the explicit
        // `Received`/`DepositPending` variants were deleted, because
        // `#[serde(other)] Unknown` also maps to `Pending`.
        for (status, expected_variant, expected) in [
            (
                "received",
                AcrossSwapStatus::Received,
                ExecutorSwapStatus::Pending,
            ),
            (
                "deposit-pending",
                AcrossSwapStatus::DepositPending,
                ExecutorSwapStatus::Pending,
            ),
            (
                "deposit-failed",
                AcrossSwapStatus::DepositFailed,
                ExecutorSwapStatus::Failed,
            ),
            (
                "future-provider-status",
                AcrossSwapStatus::Unknown,
                ExecutorSwapStatus::Pending,
            ),
        ] {
            let raw = format!(
                r#"{{
                    "status": "{status}",
                    "originChainId": 137,
                    "depositId": "deposit-benign",
                    "depositTxnRef": "0x1234",
                    "fillTxnRef": null,
                    "destinationChainId": 1,
                    "depositRefundTxnRef": null
                }}"#,
            );
            let response = serde_json::from_str::<SwapStatusResponse>(&raw).unwrap();

            assert_eq!(response.status, expected_variant);
            assert_eq!(
                ExecutorSwapStatus::from(response.status),
                expected
            );
        }
    }

    #[test]
    fn test_status_response_tolerates_null_identifiers() {
        // Across returns `"depositId": null` for CCTP/OFT deposits (it routes
        // USDC over CCTP), and the early states have no proven contract for the
        // other identifiers. A required field here would break status polling
        // for that swap permanently.
        let raw = r#"{
            "status": "pending",
            "originChainId": null,
            "depositId": null,
            "depositTxnRef": null,
            "fillTxnRef": null,
            "destinationChainId": null,
            "depositRefundTxnRef": null
        }"#;
        let response = serde_json::from_str::<SwapStatusResponse>(raw).unwrap();
        assert_eq!(
            response.status,
            AcrossSwapStatus::Pending
        );

        // Absent keys must work too.
        let response =
            serde_json::from_str::<SwapStatusResponse>(r#"{"status":"filled"}"#).unwrap();
        assert_eq!(
            response.status,
            AcrossSwapStatus::Filled
        );
    }

    #[test]
    fn test_try_from_raw_details() {
        let across_details = RawSwapDetails::Across(default_across_raw_transaction());
        let result = AcrossRawTransaction::try_from(across_details);
        assert_eq!(
            result,
            Ok(default_across_raw_transaction())
        );

        let non_across_details = RawSwapDetails::ZeroEx(default_zero_ex_raw_transaction());
        let result = AcrossRawTransaction::try_from(non_across_details);
        assert_eq!(
            result,
            Err(SwapsClientError::WrongRawTransaction)
        );
    }

    #[tokio::test]
    async fn test_get_quote() {
        let server = MockServer::start();
        let config = SwapsConfig::default();
        let mut client = AcrossClient::new(&config);
        client.base_url = server.base_url();

        let data = CreateSwapData {
            invoice_id: Uuid::new_v4(),
            swap_executor: SwapExecutorType::Across,
            from_chain: SwapChainType::Optimism,
            to_chain: SwapChainType::Polygon,
            from_token_address: "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85".to_string(),
            to_token_address: "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359".to_string(),
            // not important, shouldn't be used in request
            from_amount_units: 0,
            expected_to_amount_units: 1_000_000,
            from_address: "0xA4d353BBc130cbeF1811f27ac70989F9d568CeAB".to_string(),
            to_address: "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9".to_string(),
            direction: SwapDirection::Incoming,
            origin: Default::default(),
        };

        let mock = server.mock(|when, then| {
            when
                .path("/api/swap/approval")
                .query_param("tradeType", "minOutput")
                .query_param("amount", "1000000")
                .query_param("inputToken", "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85")
                .query_param("outputToken", "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359")
                .query_param("originChainId", "10")
                .query_param("destinationChainId", "137")
                .query_param("depositor", "0xA4d353BBc130cbeF1811f27ac70989F9d568CeAB")
                .query_param("recipient", "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9");

            then.json_body(serde_json::json!({
                "crossSwapType": "anyToBridgeable",
                "amountType": "exactInput",
                "checks": {
                    "allowance": {
                        "token": "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
                        "spender": "0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
                        "actual": "115792089237316195423570985008687907853269984665640564039457584007913116539935",
                        "expected": "1000000"
                    },
                    "balance": {
                        "token": "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
                        "actual": "2993387",
                        "expected": "1000000"
                    }
                },
                "steps": {
                    "originSwap": {
                        "tokenIn": {
                            "decimals": 6,
                            "symbol": "USDC",
                            "address": "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
                            "name": "USD Coin",
                            "chainId": 10
                        },
                        "tokenOut": {
                            "address": "0x4200000000000000000000000000000000000006",
                            "decimals": 18,
                            "symbol": "WETH",
                            "chainId": 10
                        },
                        "inputAmount": "1000000",
                        "outputAmount": "472306805062946",
                        "minOutputAmount": "468764504022050",
                        "maxInputAmount": "1000000",
                        "swapProvider": {
                            "name": "0x",
                            "sources": [
                                "woofi_v2"
                            ]
                        },
                        "slippage": 0.0075
                    },
                    "bridge": {
                        "inputAmount": "468764504022050",
                        "outputAmount": "464434354522974",
                        "tokenIn": {
                            "address": "0x4200000000000000000000000000000000000006",
                            "decimals": 18,
                            "symbol": "WETH",
                            "chainId": 10
                        },
                        "tokenOut": {
                            "decimals": 18,
                            "symbol": "WETH",
                            "address": "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
                            "name": "Wrapped Ether",
                            "chainId": 42161
                        },
                        "fees": {
                            "amount": "4330149499076",
                            "pct": "9237366442904769",
                            "token": {
                                "address": "0x4200000000000000000000000000000000000006",
                                "decimals": 18,
                                "symbol": "WETH",
                                "chainId": 10
                            },
                            "details": {
                                "type": "across",
                                "relayerCapital": {
                                    "amount": "46867294561",
                                    "pct": "99980468145248",
                                    "token": {
                                        "address": "0x4200000000000000000000000000000000000006",
                                        "decimals": 18,
                                        "symbol": "WETH",
                                        "chainId": 10
                                    }
                                },
                                "destinationGas": {
                                    "amount": "4260773033213",
                                    "pct": "9089367895086142",
                                    "token": {
                                        "address": "0x4200000000000000000000000000000000000006",
                                        "decimals": 18,
                                        "symbol": "WETH",
                                        "chainId": 10
                                    }
                                },
                                "lp": {
                                    "amount": "22509171302",
                                    "pct": "48018079673379",
                                    "token": {
                                        "address": "0x4200000000000000000000000000000000000006",
                                        "decimals": 18,
                                        "symbol": "WETH",
                                        "chainId": 10
                                    }
                                }
                            }
                        },
                        "provider": "across"
                    }
                },
                "inputToken": {
                    "decimals": 6,
                    "symbol": "USDC",
                    "address": "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
                    "name": "USD Coin",
                    "chainId": 10
                },
                "outputToken": {
                    "decimals": 18,
                    "symbol": "WETH",
                    "address": "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
                    "name": "Wrapped Ether",
                    "chainId": 42161
                },
                "refundToken": {
                    "address": "0x4200000000000000000000000000000000000006",
                    "decimals": 18,
                    "symbol": "WETH",
                    "chainId": 10
                },
                "fees": {
                    "total": {
                        "amount": "6701",
                        "amountUsd": "0.0067",
                        "token": {
                            "decimals": 6,
                            "symbol": "USDC",
                            "address": "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
                            "name": "USD Coin",
                            "chainId": 10
                        },
                        "pct": "6701306754817189",
                        "details": {
                            "type": "total-breakdown",
                            "swapImpact": {
                                "amount": "-2490",
                                "amountUsd": "-0.002490006186373967",
                                "token": {
                                    "decimals": 6,
                                    "symbol": "USDC",
                                    "address": "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
                                    "name": "USD Coin",
                                    "chainId": 10
                                },
                                "pct": "-2490491832281262"
                            },
                            "app": {
                                "amount": "0",
                                "amountUsd": "0.0",
                                "token": {
                                    "decimals": 18,
                                    "symbol": "WETH",
                                    "address": "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
                                    "name": "Wrapped Ether",
                                    "chainId": 42161
                                },
                                "pct": "0"
                            },
                            "bridge": {
                                "amount": "4330149499076",
                                "amountUsd": "0.009190006186373967",
                                "token": {
                                    "address": "0x4200000000000000000000000000000000000006",
                                    "decimals": 18,
                                    "symbol": "WETH",
                                    "chainId": 10
                                },
                                "pct": "9191798587098451",
                                "details": {
                                    "type": "across",
                                    "lp": {
                                        "amount": "22509171302",
                                        "amountUsd": "0.000047771889529374",
                                        "token": {
                                            "address": "0x4200000000000000000000000000000000000006",
                                            "decimals": 18,
                                            "symbol": "WETH",
                                            "chainId": 10
                                        },
                                        "pct": "47781206864712"
                                    },
                                    "relayerCapital": {
                                        "amount": "46867294561",
                                        "amountUsd": "0.000099467865265647",
                                        "token": {
                                            "address": "0x4200000000000000000000000000000000000006",
                                            "decimals": 18,
                                            "symbol": "WETH",
                                            "chainId": 10
                                        },
                                        "pct": "99487265282377"
                                    },
                                    "destinationGas": {
                                        "amount": "4260773033213",
                                        "amountUsd": "0.009042766431578945",
                                        "token": {
                                            "chainId": 42161,
                                            "address": "0x0000000000000000000000000000000000000000",
                                            "decimals": 18,
                                            "symbol": "ETH"
                                        },
                                        "pct": "9044530114951359"
                                    }
                                }
                            }
                        }
                    },
                    "totalMax": {
                        "amount": "14102",
                        "amountUsd": "0.0141",
                        "token": {
                            "decimals": 6,
                            "symbol": "USDC",
                            "address": "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
                            "name": "USD Coin",
                            "chainId": 10
                        },
                        "pct": "14102750036257069",
                        "details": {
                            "type": "max-total-breakdown",
                            "maxSwapImpact": {
                                "amount": "4910",
                                "amountUsd": "0.004909993813626033",
                                "token": {
                                    "decimals": 6,
                                    "symbol": "USDC",
                                    "address": "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
                                    "name": "USD Coin",
                                    "chainId": 10
                                },
                                "pct": "4910951449158618"
                            },
                            "app": {
                                "amount": "0",
                                "amountUsd": "0.0",
                                "token": {
                                    "decimals": 18,
                                    "symbol": "WETH",
                                    "address": "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
                                    "name": "Wrapped Ether",
                                    "chainId": 42161
                                },
                                "pct": "0"
                            },
                            "bridge": {
                                "amount": "4330149499076",
                                "amountUsd": "0.009190006186373967",
                                "token": {
                                    "address": "0x4200000000000000000000000000000000000006",
                                    "decimals": 18,
                                    "symbol": "WETH",
                                    "chainId": 10
                                },
                                "pct": "9191798587098451",
                                "details": {
                                    "type": "across",
                                    "lp": {
                                        "amount": "22509171302",
                                        "amountUsd": "0.000047771889529374",
                                        "token": {
                                            "address": "0x4200000000000000000000000000000000000006",
                                            "decimals": 18,
                                            "symbol": "WETH",
                                            "chainId": 10
                                        },
                                        "pct": "47781206864712"
                                    },
                                    "relayerCapital": {
                                        "amount": "46867294561",
                                        "amountUsd": "0.000099467865265647",
                                        "token": {
                                            "address": "0x4200000000000000000000000000000000000006",
                                            "decimals": 18,
                                            "symbol": "WETH",
                                            "chainId": 10
                                        },
                                        "pct": "99487265282377"
                                    },
                                    "destinationGas": {
                                        "amount": "4260773033213",
                                        "amountUsd": "0.009042766431578945",
                                        "token": {
                                            "chainId": 42161,
                                            "address": "0x0000000000000000000000000000000000000000",
                                            "decimals": 18,
                                            "symbol": "ETH"
                                        },
                                        "pct": "9044530114951359"
                                    }
                                }
                            }
                        }
                    },
                    "originGas": {
                        "amount": "626037662500",
                        "amountUsd": "0.001328658512253625",
                        "token": {
                            "chainId": 10,
                            "address": "0x0000000000000000000000000000000000000000",
                            "decimals": 18,
                            "symbol": "ETH"
                        }
                    }
                },
                "inputAmount": "1000000",
                "maxInputAmount": "1000000",
                "expectedOutputAmount": "467917612181896",
                "minOutputAmount": "464434354522974",
                "expectedFillTime": 2,
                "swapTx": {
                    "simulationSuccess": true,
                    "chainId": 10,
                    "to": "0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
                    "data": "0xad5425c6000000000000000000000000a4d353bbc130cbef1811f27ac70989f9d568ceab0000000000000000000000000e3ca7fd040144900adaa5f9b8917f3933a4f5e90000000000000000000000000b2c639c533813f4aa9d7837caf62653d097ff850000000000000000000000003c499c542cef5e3811e1192ce70d8cc03d5c335900000000000000000000000000000000000000000000000000000000000f55c800000000000000000000000000000000000000000000000000000000000f4d190000000000000000000000000000000000000000000000000000000000000089000000000000000000000000cad97616f91872c02ba3553db315db4015cbe8500000000000000000000000000000000000000000000000000000000069caaf870000000000000000000000000000000000000000000000000000000069cacba700000000000000000000000000000000000000000000000000000000000000050000000000000000000000000000000000000000000000000000000000000180000000000000000000000000000000000000000000000000000000000000000073c0de",
                    "gas": "571750",
                    "maxFeePerGas": "1094950",
                    "maxPriorityFeePerGas": "1000000"
                },
                "quoteExpiryTimestamp": 1770679043,
                "id": "vqhfb-1770675636494-01dc8983f2fc",
                "x-gitbook-description-html": "<p>Swap approval data returned successfully.</p>"
            }));
        });

        let expected_response = SwapQuote {
            swap_executor: SwapExecutorType::Across,
            id: "vqhfb-1770675636494-01dc8983f2fc".to_string(),
            estimated_to_amount_units: 467917612181896,
            estimated_to_amount: rust_decimal::Decimal::ZERO,
            valid_till: chrono::DateTime::parse_from_rfc3339("2026-02-09T23:17:23Z").unwrap().to_utc(),
            quote_details: RawSwapDetails::Across(
                AcrossRawTransaction {
                    transaction: SwapTransaction {
                        chain_id: 10,
                        contract_address: "0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C".to_string(),
                        data: "0xad5425c6000000000000000000000000a4d353bbc130cbef1811f27ac70989f9d568ceab0000000000000000000000000e3ca7fd040144900adaa5f9b8917f3933a4f5e90000000000000000000000000b2c639c533813f4aa9d7837caf62653d097ff850000000000000000000000003c499c542cef5e3811e1192ce70d8cc03d5c335900000000000000000000000000000000000000000000000000000000000f55c800000000000000000000000000000000000000000000000000000000000f4d190000000000000000000000000000000000000000000000000000000000000089000000000000000000000000cad97616f91872c02ba3553db315db4015cbe8500000000000000000000000000000000000000000000000000000000069caaf870000000000000000000000000000000000000000000000000000000069cacba700000000000000000000000000000000000000000000000000000000000000050000000000000000000000000000000000000000000000000000000000000180000000000000000000000000000000000000000000000000000000000000000073c0de".to_string(),
                        value: 0,
                        gas: Some(571750),
                        max_fee_per_gas: Some(1094950),
                        max_priority_fee_per_gas: Some(1000000),
                    },
                    approval_transactions: vec![],
                },
            ),
        };

        let response = client.get_quote(data).await.unwrap();
        assert_eq!(response, expected_response);
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_transaction_status() {
        let server = MockServer::start();
        let config = SwapsConfig::default();
        let mut client = AcrossClient::new(&config);
        client.base_url = server.base_url();

        let data = SwapDetails {
            // only transaction_hash field is used
            id: "test".to_string(),
            raw_transaction: RawSwapDetails::Across(default_across_raw_transaction()),
            signature: None,
            transaction_hash: Some(
                "0x98e81ff0d66222e920881dc956762bd8ef45a42babbf39f837d3b7c89fdd73a8".to_string(),
            ),
        };

        let mock = server.mock(|when, then| {
            when
                .path("/api/deposit/status")
                .query_param("depositTxnRef", "0x98e81ff0d66222e920881dc956762bd8ef45a42babbf39f837d3b7c89fdd73a8");

            then
                .json_body(serde_json::json!({
                    "status": "filled",
                    "originChainId": 8453,
                    "depositId": "5524933",
                    "depositTxHash": "0x98e81ff0d66222e920881dc956762bd8ef45a42babbf39f837d3b7c89fdd73a8",
                    "depositTxnRef": "0x98e81ff0d66222e920881dc956762bd8ef45a42babbf39f837d3b7c89fdd73a8",
                    "fillTx": "0x08897d29b4ef00a65282c4c67a0c75fc826512927b3c95ae763c79ce9baef107",
                    "fillTxnRef": "0x08897d29b4ef00a65282c4c67a0c75fc826512927b3c95ae763c79ce9baef107",
                    "destinationChainId": 137,
                    "depositRefundTxHash": null,
                    "depositRefundTxnRef": null,
                    "actionsSucceeded": true,
                    "pagination": {
                        "currentIndex": 0,
                        "maxIndex": 0
                    }
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
    fn api_error(
        status: Option<u32>,
        message: &str,
    ) -> AcrossApiError {
        AcrossApiError {
            error_type: None,
            error: None,
            code: None,
            status,
            message: message.to_string(),
            id: None,
        }
    }

    #[test]
    fn test_error_response_classification() {
        let rejection = AcrossApiError {
            error_type: Some("InvalidParamError".to_string()),
            error: None,
            code: Some("AMOUNT_TOO_LOW".to_string()),
            status: Some(400),
            message: "Sent amount is too low relative to fees".to_string(),
            id: None,
        };
        assert_eq!(
            rejection.into_client_error(reqwest::StatusCode::BAD_REQUEST),
            SwapsClientError::ProviderRejected {
                message: "Sent amount is too low relative to fees".to_string(),
            }
        );

        assert_eq!(
            api_error(Some(500), "Internal server error")
                .into_client_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            SwapsClientError::UnknownApiError
        );
    }

    /// A gateway failure answers with a bare `{"message": …}` and no embedded
    /// status. Before the transport status was threaded through, this was
    /// classified as a rejection and surfaced to the payer as a 422 they could
    /// do nothing about.
    #[test]
    fn server_error_without_embedded_status_is_not_a_rejection() {
        assert_eq!(
            api_error(None, "Bad gateway").into_client_error(reqwest::StatusCode::BAD_GATEWAY),
            SwapsClientError::UnknownApiError
        );
        assert_eq!(
            api_error(None, "Service unavailable")
                .into_client_error(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            SwapsClientError::UnknownApiError
        );
    }

    /// The body stays authoritative when it does carry a status: a 400-shaped
    /// rejection delivered over an unexpected transport status is still the
    /// requester's problem to fix.
    #[test]
    fn embedded_status_wins_over_transport_status() {
        assert_eq!(
            api_error(Some(400), "Amount too low")
                .into_client_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            SwapsClientError::ProviderRejected {
                message: "Amount too low".to_string(),
            }
        );
        assert_eq!(
            api_error(Some(503), "Upstream unavailable").into_client_error(reqwest::StatusCode::OK),
            SwapsClientError::UnknownApiError
        );
    }

    /// A genuine client-side rejection with no embedded status must stay a
    /// rejection — the fallback must not sweep every statusless body into
    /// "our fault".
    #[test]
    fn client_error_without_embedded_status_stays_a_rejection() {
        assert_eq!(
            api_error(None, "Unsupported route")
                .into_client_error(reqwest::StatusCode::BAD_REQUEST),
            SwapsClientError::ProviderRejected {
                message: "Unsupported route".to_string(),
            }
        );
    }
}
