mod executor;
mod tracker;

pub use executor::SwapsExecutor;
pub use tracker::SwapsTracker;

use crate::clients::{
    AcrossClient,
    BungeeClient,
};
use crate::configs::SwapsConfig;

#[derive(Clone)]
pub struct SwapsClients {
    pub across_client: AcrossClient,
    pub bungee_client: BungeeClient,
}

impl SwapsClients {
    pub fn new(config: SwapsConfig) -> Self {
        let across_client = AcrossClient::new(&config);
        let bungee_client = BungeeClient::new(&config);

        Self {
            across_client,
            bungee_client,
        }
    }
}
