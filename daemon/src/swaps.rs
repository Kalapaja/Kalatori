mod executor;
mod tracker;

#[cfg_attr(test, mockall_double::double)]
pub use executor::SwapsExecutor;
#[cfg_attr(not(test), expect(unused_imports))]
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
