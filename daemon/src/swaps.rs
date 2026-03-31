mod executor;
mod tracker;

pub use executor::SwapsExecutor;
pub use tracker::SwapsTracker;

use crate::clients::{
    AcrossClient,
    BungeeClient,
    ExecutorSwapStatus,
    SwapsClient,
    SwapsClientError,
    ZeroExClient,
};
use crate::configs::SwapsConfig;
use crate::types::{
    CreateSwapData,
    SwapDetails,
    SwapExecutorType,
    SwapQuote,
};

#[derive(Clone)]
pub struct SwapsClients {
    pub across_client: AcrossClient,
    pub bungee_client: BungeeClient,
    pub zero_ex_client: ZeroExClient,
}

impl SwapsClients {
    pub async fn new(config: SwapsConfig) -> Self {
        let across_client = AcrossClient::new(&config);
        let bungee_client = BungeeClient::new(&config);
        let zero_ex_client = ZeroExClient::new(&config).await;

        Self {
            across_client,
            bungee_client,
            zero_ex_client,
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
        }
    }
}
