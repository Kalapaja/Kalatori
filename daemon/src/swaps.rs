mod executor;
mod tracker;

pub use executor::SwapsExecutor;
pub use tracker::SwapsTracker;

use crate::clients::{
    AcrossClient,
    BungeeClient,
    ZeroExClient,
};
use crate::configs::SwapsConfig;

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
}
