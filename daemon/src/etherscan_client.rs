mod types;

use std::sync::Arc;
use std::time::Duration;

use governor::{
    DefaultDirectRateLimiter,
    Quota,
    RateLimiter,
};
use secrecy::{
    ExposeSecret,
    SecretString,
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
    /// Etherscan reported a transfer whose base-unit amount cannot be
    /// represented as a `Decimal`. The item is rejected on its own, so the
    /// other transfers in the same response are still recorded — a poisoned
    /// item must not cost the batch, because Etherscan keeps returning it and
    /// every retry would meet it again.
    #[error("Transfer {tx_hash}: amount {value} with {decimals} decimals is not representable")]
    UnrepresentableAmount {
        value: u128,
        decimals: u32,
        tx_hash: String,
        block_number: u32,
        transaction_index: u32,
    },
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
    api_key: SecretString,
    rate_limiter: Arc<DefaultDirectRateLimiter>,
}

impl EtherscanClient {
    fn convert_incoming_transfers(
        transactions: Vec<EtherscanTransaction>,
        address: &str,
        invoice_id: Uuid,
    ) -> Vec<IncomingTransaction> {
        transactions
            .into_iter()
            .filter(|transaction| transaction.to.eq_ignore_ascii_case(address))
            .filter_map(|transaction| {
                transaction
                    .into_incoming_transaction(invoice_id)
                    .map_err(|error| {
                        match &error {
                            EtherscanClientError::UnrepresentableAmount {
                                value,
                                decimals,
                                tx_hash,
                                block_number,
                                transaction_index,
                            } => tracing::error!(
                                %invoice_id,
                                %tx_hash,
                                block_number,
                                transaction_index,
                                value,
                                decimals,
                                "Rejected unrepresentable Etherscan transfer; valid batch items will continue"
                            ),
                            _ => tracing::error!(
                                %invoice_id,
                                error = ?error,
                                "Rejected Etherscan transfer during conversion"
                            ),
                        }
                    })
                    .ok()
            })
            .collect()
    }

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
            api_key: self.api_key.expose_secret(),
        };

        let raw_response = self
            .client
            .get("https://api.etherscan.io/v2/api")
            .query(&params)
            .timeout(ETHERSCAN_CLIENT_REQUEST_TIMEOUT)
            .send()
            .await
            .inspect_err(|e| {
                // Never format a `reqwest::Error` from this client with `{:?}` or `{}`:
                // it carries the request URL, and the API key is a query parameter.
                // `status()` is deliberately absent: a send() failure is `Kind::Request`
                // and `Error::status()` answers `Some` only for `Kind::Status`.
                tracing::warn!(
                    timeout = e.is_timeout(),
                    connect = e.is_connect(),
                    "Etherscan request failed"
                )
            })?
            .text()
            .await
            .inspect_err(|e| {
                // Same rule as above. Body errors carry no URL at reqwest 0.13.2, but
                // 0.13.4 attaches it in `do_bytes`, so this arm must not print `e` either.
                tracing::warn!(
                    timeout = e.is_timeout(),
                    body = e.is_body(),
                    decode = e.is_decode(),
                    "Etherscan response body failed"
                )
            })?;

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

        let transactions = self
            .get_account_transfers(chain_id, asset_id, address)
            .await?;

        Ok(Self::convert_incoming_transfers(
            transactions,
            address,
            invoice_id,
        ))
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;

    fn transaction(
        hash: &str,
        value: u128,
    ) -> EtherscanTransaction {
        EtherscanTransaction {
            block_number: 1,
            hash: hash.to_string(),
            from: "0xfrom".to_string(),
            contract_address: "0xcontract".to_string(),
            to: "0xrecipient".to_string(),
            value,
            token_symbol: "USDC".to_string(),
            token_decimal: 18,
            transaction_index: 0,
        }
    }

    #[test]
    fn mixed_batch_keeps_valid_transfers_around_rejected_item() {
        let invoice_id = Uuid::new_v4();
        let transfers = EtherscanClient::convert_incoming_transfers(
            vec![
                transaction("0xgood1", 1_000_000_000_000_000_000),
                transaction("0xbad", u128::MAX),
                transaction("0xgood2", 2_000_000_000_000_000_000),
            ],
            "0xRECIPIENT",
            invoice_id,
        );

        assert_eq!(transfers.len(), 2);
        assert_eq!(
            transfers[0].transfer_info.amount,
            Decimal::ONE
        );
        assert_eq!(
            transfers[1].transfer_info.amount,
            Decimal::from(2)
        );
    }
}
