mod executor;
mod tracker;

#[cfg_attr(test, mockall_double::double)]
pub use executor::SwapsExecutor;
pub use executor::SwapsExecutorError;
pub use tracker::SwapsTracker;

use crate::chain_client::KeyringClient;
use crate::clients::{
    AcrossClient,
    BungeeClient,
    ExecutorSwapStatus,
    SwapsClient,
    SwapsClientError,
    ZeroExClient,
    ZeroExGaslessClient,
};
use crate::configs::SwapsConfig;
use crate::types::{
    CreateSwapData,
    Swap,
    SwapDetails,
    SwapExecutorType,
    SwapQuote,
};

#[derive(Clone)]
pub struct SwapsClients {
    pub across_client: AcrossClient,
    pub bungee_client: BungeeClient,
    pub zero_ex_client: ZeroExClient,
    pub zero_ex_gasless_client: ZeroExGaslessClient,
}

impl SwapsClients {
    pub async fn new(config: SwapsConfig) -> Self {
        let across_client = AcrossClient::new(&config);
        let bungee_client = BungeeClient::new(&config);
        let zero_ex_client = ZeroExClient::new(&config).await;
        let zero_ex_gasless_client = ZeroExGaslessClient::new(&config);

        Self {
            across_client,
            bungee_client,
            zero_ex_client,
            zero_ex_gasless_client,
        }
    }

    pub async fn get_quote(
        &self,
        executor: SwapExecutorType,
        data: CreateSwapData,
    ) -> Result<SwapQuote, SwapsClientError> {
        match executor {
            SwapExecutorType::Across => self.across_client.get_quote(data).await,
            SwapExecutorType::Bungee => self.bungee_client.get_quote(data).await,
            SwapExecutorType::ZeroEx => {
                self.zero_ex_client
                    .get_quote(data)
                    .await
            },
            SwapExecutorType::ZeroExGasless => {
                self.zero_ex_gasless_client
                    .get_quote(data)
                    .await
            },
        }
    }

    pub async fn sign_transaction(
        &self,
        keyring_client: &KeyringClient,
        swap: &Swap,
    ) -> Result<String, SwapsClientError> {
        match swap.request.swap_executor {
            SwapExecutorType::Across => {
                self.across_client
                    .sign_transaction(keyring_client, swap)
                    .await
            },
            SwapExecutorType::Bungee => {
                self.bungee_client
                    .sign_transaction(keyring_client, swap)
                    .await
            },
            SwapExecutorType::ZeroEx => {
                self.zero_ex_client
                    .sign_transaction(keyring_client, swap)
                    .await
            },
            SwapExecutorType::ZeroExGasless => {
                self.zero_ex_gasless_client
                    .sign_transaction(keyring_client, swap)
                    .await
            },
        }
    }

    /// Check a caller-supplied signature against the stored quote before it is
    /// written to the database. Rejecting here keeps a payload that the
    /// submission path cannot parse from ever reaching the swap row.
    pub fn validate_signature(
        &self,
        executor: SwapExecutorType,
        details: &SwapDetails,
        signature: &str,
    ) -> Result<(), SwapsClientError> {
        match executor {
            SwapExecutorType::Across => self
                .across_client
                .validate_signature(details, signature),
            SwapExecutorType::Bungee => self
                .bungee_client
                .validate_signature(details, signature),
            SwapExecutorType::ZeroEx => self
                .zero_ex_client
                .validate_signature(details, signature),
            SwapExecutorType::ZeroExGasless => self
                .zero_ex_gasless_client
                .validate_signature(details, signature),
        }
    }

    pub async fn submit_transaction(
        &self,
        executor: SwapExecutorType,
        data: &SwapDetails,
    ) -> Result<String, SwapsClientError> {
        match executor {
            SwapExecutorType::Across => {
                self.across_client
                    .submit_transaction(data)
                    .await
            },
            SwapExecutorType::Bungee => {
                self.bungee_client
                    .submit_transaction(data)
                    .await
            },
            SwapExecutorType::ZeroEx => {
                self.zero_ex_client
                    .submit_transaction(data)
                    .await
            },
            SwapExecutorType::ZeroExGasless => {
                self.zero_ex_gasless_client
                    .submit_transaction(data)
                    .await
            },
        }
    }

    pub async fn get_transaction_status(
        &self,
        executor: SwapExecutorType,
        data: &SwapDetails,
    ) -> Result<ExecutorSwapStatus, SwapsClientError> {
        match executor {
            SwapExecutorType::Across => {
                self.across_client
                    .get_transaction_status(data)
                    .await
            },
            SwapExecutorType::Bungee => {
                self.bungee_client
                    .get_transaction_status(data)
                    .await
            },
            SwapExecutorType::ZeroEx => {
                self.zero_ex_client
                    .get_transaction_status(data)
                    .await
            },
            SwapExecutorType::ZeroExGasless => {
                self.zero_ex_gasless_client
                    .get_transaction_status(data)
                    .await
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::clients::{
        RawSwapDetails,
        default_zero_ex_gasless_raw_transaction,
    };

    use super::*;

    #[tokio::test]
    async fn signature_validation_dispatches_to_each_executor_client() {
        let clients = SwapsClients::new(SwapsConfig::default()).await;
        let mut raw_transaction = default_zero_ex_gasless_raw_transaction();
        raw_transaction.approval = Some(raw_transaction.raw_trade.clone());
        let details = SwapDetails {
            id: "quote-with-approval".to_string(),
            raw_transaction: RawSwapDetails::ZeroExGasless(raw_transaction),
            signature: None,
            transaction_hash: None,
        };

        for executor in [
            SwapExecutorType::Across,
            SwapExecutorType::Bungee,
            SwapExecutorType::ZeroEx,
        ] {
            assert_eq!(
                clients.validate_signature(executor, &details, "0xdeadbeef"),
                Ok(()),
                "{executor} accepts an opaque signature"
            );
        }

        assert_eq!(
            clients.validate_signature(
                SwapExecutorType::ZeroExGasless,
                &details,
                "0xdeadbeef"
            ),
            Err(SwapsClientError::InvalidSignaturePayload)
        );
    }
}
