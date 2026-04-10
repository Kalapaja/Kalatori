use alloy::primitives::{
    Address,
    Log,
    U256,
};
use serde::de::DeserializeOwned;
use serde::{
    Deserialize,
    Serialize,
};

use super::consts::{
    ACCOUNT_IMPL,
    BUNDLER_RPC,
    CHAIN_ID,
    ENTRYPOINT,
    PAYMASTER,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Eip7702Auth {
    pub chain_id: U256,
    pub address: Address,
    pub nonce: U256,
    pub y_parity: U256,
    pub r: U256,
    pub s: U256,
}

impl Eip7702Auth {
    fn dummy(nonce: U256) -> Self {
        Self {
            nonce,
            chain_id: U256::from(CHAIN_ID),
            address: ACCOUNT_IMPL,
            y_parity: U256::from(1),
            r: U256::MAX,
            s: U256::MAX,
        }
    }
}

impl From<alloy::eips::eip7702::SignedAuthorization> for Eip7702Auth {
    fn from(value: alloy::eips::eip7702::SignedAuthorization) -> Self {
        Self {
            chain_id: *value.chain_id(),
            address: *value.address(),
            nonce: U256::from(value.nonce()),
            y_parity: U256::from(value.y_parity()),
            r: value.r(),
            s: value.s(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasParams {
    pub pre_verification_gas: U256,
    pub verification_gas_limit: U256,
    pub call_gas_limit: U256,
    pub paymaster_verification_gas_limit: U256,
    pub paymaster_post_op_gas_limit: U256,
}

impl GasParams {
    pub fn dummy() -> Self {
        Self {
            pre_verification_gas: U256::from(10_000),
            verification_gas_limit: U256::from(50_000),
            call_gas_limit: U256::from(10_000),
            paymaster_verification_gas_limit: U256::from(30_000),
            paymaster_post_op_gas_limit: U256::from(15_000),
        }
    }
}

// This structure is pretty similar to `PackedUserOperation` from `alloy` but
// has some differences:
// 1. Contains `eip7702_auth` field which is missing in alloy's version
// 2. Some of fields are not optional (they are not optional for our specific
//    implementation)
// 3. Some optional fields which we don't need is not presented here (like
//    `factory` and `factory_data`)
// 4. Some fields are wrapped into structures which are returned from other
//    calls in order to simplify the code
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserOperationParams {
    pub sender: Address,
    pub nonce: U256,
    pub call_data: String,
    pub paymaster: Address,
    pub paymaster_data: String,
    pub signature: String,
    #[serde(flatten)]
    pub gas_params: GasParams,
    #[serde(flatten)]
    pub gas_price: GasPrice,
    #[serde(rename = "eip7702Auth")]
    pub eip7702_auth: Eip7702Auth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasPrice {
    pub max_fee_per_gas: U256,
    pub max_priority_fee_per_gas: U256,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasPrices {
    pub slow: GasPrice,
    pub standard: GasPrice,
    pub fast: GasPrice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserOperationReceipt {
    pub transaction_hash: String,
    pub transaction_index: U256,
    pub block_hash: String,
    pub block_number: U256,
    pub from: Address,
    pub to: Address,
    pub cumulative_gas_used: U256,
    pub gas_used: U256,
    pub logs: Vec<Log>,
    pub logs_bloom: String,
    pub status: U256,
    pub effective_gas_price: U256,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserOperationReceiptResult {
    pub user_op_hash: String,
    pub sender: Address,
    pub nonce: U256,
    pub actual_gas_used: U256,
    pub actual_gas_cost: U256,
    pub success: bool,
    pub logs: Vec<Log>,
    pub receipt: UserOperationReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenQuoteRequestParams {
    tokens: Vec<Address>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenQuote {
    pub token: Address,
    pub paymaster: Address,
    pub exchange_rate: U256, /* Price of 1 full native token in ERC20 smallest units, scaled by
                              * 10^18 */
    pub post_op_gas: U256, // Extra gas for paymaster postOp
    pub exchange_rate_native_to_usd: U256,
    pub balance_slot: U256,
    pub allowance_slot: U256,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenQuotesResponse {
    pub quotes: Vec<TokenQuote>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest<T> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: T,
}

impl<T> JsonRpcRequest<T> {
    fn new(
        method: &'static str,
        params: T,
    ) -> Self {
        JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcSuccessfulResponse<T> {
    id: u64,
    result: T,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcErrorResponse {
    id: u64,
    error: JsonRpcError,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum JsonRpcResponse<T> {
    Ok(JsonRpcSuccessfulResponse<T>),
    Err(JsonRpcErrorResponse),
}

#[derive(Debug, thiserror::Error)]
pub enum PimlicoClientError {
    #[error("Network error")]
    Reqwest(#[from] reqwest::Error),
    #[error("Pimlico bundler error")]
    Bundler(JsonRpcError),
    #[error("Invalid JSON response")]
    InvalidJson(#[from] serde_json::Error),
}

type PimlicoResult<T> = Result<T, PimlicoClientError>;

#[derive(Clone)]
pub struct PimlicoClient {
    client: reqwest::Client,
}

impl PimlicoClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    async fn send_request<T: Serialize, R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: T,
    ) -> PimlicoResult<R> {
        let params = JsonRpcRequest::new(method, params);

        let response = self
            .client
            .post(BUNDLER_RPC)
            .json(&params)
            .send()
            .await?
            .json::<JsonRpcResponse<R>>()
            .await?;

        match response {
            JsonRpcResponse::Ok(resp) => Ok(resp.result),
            JsonRpcResponse::Err(resp) => Err(PimlicoClientError::Bundler(resp.error)),
        }
    }

    pub async fn get_gas_prices(&self) -> PimlicoResult<GasPrices> {
        self.send_request(
            "pimlico_getUserOperationGasPrice",
            [(); 0],
        )
        .await
    }

    pub async fn get_estimate_gas(
        &self,
        sender: Address,
        nonce: U256,
        call_data: Vec<u8>,
        paymaster_data: String,
        gas_price: GasPrice,
    ) -> PimlicoResult<GasParams> {
        let req = UserOperationParams {
            sender,
            nonce,
            call_data: const_hex::encode_prefixed(call_data),
            gas_price,
            paymaster: PAYMASTER,
            paymaster_data,
            gas_params: GasParams::dummy(),
            eip7702_auth: Eip7702Auth::dummy(nonce),
            signature: "0xfffffffffffffffffffffffffffffff0000000000000000000000000000000007aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1c".to_string(),
        };

        self.send_request(
            "eth_estimateUserOperationGas",
            (req, ENTRYPOINT),
        )
        .await
    }

    pub async fn send_user_operation(
        &self,
        user_operation_params: UserOperationParams,
    ) -> PimlicoResult<String> {
        self.send_request(
            "eth_sendUserOperation",
            (user_operation_params, ENTRYPOINT),
        )
        .await
    }

    pub async fn get_operation_receipt(
        &self,
        op_hash: &str,
    ) -> PimlicoResult<Option<UserOperationReceiptResult>> {
        self.send_request(
            "eth_getUserOperationReceipt",
            (op_hash,),
        )
        .await
    }

    pub async fn get_token_quotes(
        &self,
        tokens: &[Address],
    ) -> PimlicoResult<TokenQuotesResponse> {
        self.send_request(
            "pimlico_getTokenQuotes",
            (
                TokenQuoteRequestParams {
                    tokens: tokens.to_vec(),
                },
                ENTRYPOINT,
                U256::from(CHAIN_ID),
            ),
        )
        .await
    }
}
