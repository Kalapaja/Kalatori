use std::sync::Arc;
use std::time::Duration;

use chrono::{
    DateTime,
    Utc,
};
use kalatori_client::types::ChainType;
use tokio::sync::RwLock;

use crate::chain_client::ClientError;
use crate::configs::ChainsConfig;

const TIMEOUT: u64 = 60; // 1 minute
const HEALTH_CHECK_DELAY: Duration = Duration::from_secs(60);

fn init_chain_endpoints(
    chains_config: &ChainsConfig,
    chain_type: ChainType,
) -> Result<Vec<RpcEndpoint>, ClientError> {
    let config = chains_config
        .chains
        .get(&chain_type)
        .ok_or(ClientError::InvalidConfiguration {
            field: format!("Failed to get {} config", chain_type),
        })?;

    if config.endpoints.is_empty() {
        return Err(ClientError::InvalidConfiguration {
            field: format!(
                "{} endpoints cannot be empty",
                chain_type
            ),
        })
    }

    let endpoints = config
        .endpoints
        .iter()
        .map(|url| RpcEndpoint {
            url: url.clone(),
            attempts: 0,
            status: RpcEndpointStatus::Healthy,
            last_attempt_at: None,
            next_retry_at: None,
        })
        .collect();

    Ok(endpoints)
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum RpcEndpointStatus {
    Healthy,
    Unhealthy,
}

#[derive(Debug)]
pub struct RpcEndpoint {
    url: String,
    status: RpcEndpointStatus,
    attempts: u32,
    last_attempt_at: Option<DateTime<Utc>>,
    next_retry_at: Option<DateTime<Utc>>,
}

impl RpcEndpoint {
    fn calculate_backoff(&self) -> u64 {
        TIMEOUT * 2u64.pow(self.attempts)
    }

    fn has_recovered(&self) -> bool {
        match self.next_retry_at {
            Some(retry_at) => retry_at < Utc::now(),
            None => true,
        }
    }

    fn increment_retry(&mut self) {
        let now = Utc::now();
        self.status = RpcEndpointStatus::Unhealthy;
        self.attempts += 1;
        self.last_attempt_at = Some(now);
        self.next_retry_at = Some(now + Duration::from_secs(self.calculate_backoff()));
    }

    pub fn mark_healthy(&mut self) {
        self.status = RpcEndpointStatus::Healthy;
        self.attempts = 0;
        self.last_attempt_at = None;
        self.next_retry_at = None;
    }
}

#[derive(Debug, Clone)]
pub struct RpcEndpointRotator {
    asset_hub_endpoints: Arc<RwLock<Vec<RpcEndpoint>>>,
    polygon_endpoints: Arc<RwLock<Vec<RpcEndpoint>>>,
}

impl RpcEndpointRotator {
    pub fn new(chains_config: &ChainsConfig) -> Result<RpcEndpointRotator, ClientError> {
        let asset_hub = init_chain_endpoints(
            chains_config,
            ChainType::PolkadotAssetHub,
        )?;

        let polygon = init_chain_endpoints(chains_config, ChainType::Polygon)?;

        Ok(Self {
            asset_hub_endpoints: Arc::new(RwLock::new(asset_hub)),
            polygon_endpoints: Arc::new(RwLock::new(polygon)),
        })
    }

    fn get_chain_endpoints(
        &self,
        chain_type: ChainType,
    ) -> Arc<RwLock<Vec<RpcEndpoint>>> {
        match chain_type {
            ChainType::PolkadotAssetHub => self.asset_hub_endpoints.clone(),
            ChainType::Polygon => self.polygon_endpoints.clone(),
        }
    }

    pub async fn get_endpoint_url(
        &self,
        chain_type: ChainType,
    ) -> String {
        let endpoints = self.get_chain_endpoints(chain_type);
        let lock = endpoints.read().await;

        for endpoint in lock.iter() {
            if matches!(
                endpoint.status,
                RpcEndpointStatus::Healthy
            ) {
                return endpoint.url.clone()
            }
        }

        // we checked that endpoints are not empty during initialization
        // so it's safe to unwrap here
        lock.iter()
            .min_by_key(|endpoint| endpoint.next_retry_at)
            .unwrap()
            .url
            .clone()
    }

    pub async fn mark_unhealthy(
        &self,
        url: &str,
        chain_type: ChainType,
    ) {
        let endpoints = self.get_chain_endpoints(chain_type);
        let mut lock = endpoints.write().await;

        match lock
            .iter_mut()
            .find(|endpoint| endpoint.url == url)
        {
            Some(endpoint) => {
                endpoint.increment_retry();
                tracing::warn!("Marked endpoint {url} as unhealthy");
            },
            None => tracing::warn!("Failed to increment retry. Endpoint {url} not found"),
        }
    }

    async fn heal_chain_endpoints(
        &self,
        chain_type: ChainType,
    ) {
        let endpoints = self.get_chain_endpoints(chain_type);
        let mut lock = endpoints.write().await;

        lock.iter_mut()
            .filter(|endpoint| {
                matches!(
                    endpoint.status,
                    RpcEndpointStatus::Unhealthy
                ) && endpoint.has_recovered()
            })
            .for_each(|endpoint| {
                endpoint.mark_healthy();
                tracing::debug!("Endpoint {} got healthy", endpoint.url);
            });
    }

    async fn heal_all_endpoints(&self) {
        for chain_type in ChainType::iter() {
            self.heal_chain_endpoints(chain_type)
                .await
        }
    }

    async fn perform(
        self,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) {
        tracing::info!("Starting rpc endpoints health checker");
        let mut interval = tokio::time::interval(HEALTH_CHECK_DELAY);

        loop {
            tokio::select! {
                _ = interval.tick() => self.heal_all_endpoints().await,
                () = cancellation_token.cancelled() => {
                    tracing::info!(
                        "Rpc endpoints health checker received cancellation signal, shutting down"
                    );
                    break;
                },
            }
        }
    }

    pub async fn periodic_health_check(
        self,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn(async move { self.perform(cancellation_token).await })
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use crate::configs::ChainConfig;

    use super::*;

    fn rpc_endpoint() -> RpcEndpoint {
        RpcEndpoint {
            url: "http://test.com".to_string(),
            status: RpcEndpointStatus::Healthy,
            attempts: 0,
            last_attempt_at: None,
            next_retry_at: None,
        }
    }

    #[test]
    fn test_calculate_backoff() {
        // Check that backoff grows exponentally
        let mut endpoint = rpc_endpoint();
        let backoff = endpoint.calculate_backoff();
        assert_eq!(backoff, 60);

        endpoint.attempts += 1;
        let backoff = endpoint.calculate_backoff();
        assert_eq!(backoff, 120);

        endpoint.attempts += 1;
        let backoff = endpoint.calculate_backoff();
        assert_eq!(backoff, 240);

        endpoint.attempts += 1;
        let backoff = endpoint.calculate_backoff();
        assert_eq!(backoff, 480);
    }

    #[test]
    fn test_is_get_healthy() {
        let mut endpoint = rpc_endpoint();
        assert!(endpoint.has_recovered());

        endpoint.next_retry_at = Some(Utc::now() + Duration::from_hours(1));
        assert!(!endpoint.has_recovered());

        endpoint.next_retry_at = Some(Utc::now() - Duration::from_hours(1));
        assert!(endpoint.has_recovered());
    }

    #[test]
    fn test_increment_retry_and_mark_healthy() {
        let mut endpoint = rpc_endpoint();
        endpoint.increment_retry();

        assert_eq!(
            endpoint.status,
            RpcEndpointStatus::Unhealthy
        );
        assert_eq!(endpoint.attempts, 1);
        assert!(endpoint.last_attempt_at.is_some());
        assert!(endpoint.next_retry_at.is_some());

        endpoint.mark_healthy();

        assert_eq!(
            endpoint.status,
            RpcEndpointStatus::Healthy
        );
        assert_eq!(endpoint.attempts, 0);
        assert!(endpoint.last_attempt_at.is_none());
        assert!(endpoint.next_retry_at.is_none());
    }

    #[tokio::test]
    async fn test_get_endpoint_url() {
        let endpoint1 = "http://test1.com".to_string();
        let endpoint2 = "http://test2.com".to_string();
        let endpoint3 = "http://test3.com".to_string();

        let endpoints = vec![endpoint1.clone(), endpoint2.clone(), endpoint3.clone()];

        let chain_config = ChainConfig {
            endpoints,
            assets: Vec::new(),
            allow_insecure_endpoints: true,
        };

        let chains_config = ChainsConfig {
            chains: HashMap::from([
                (ChainType::Polygon, chain_config.clone()),
                (
                    ChainType::PolkadotAssetHub,
                    chain_config,
                ),
            ]),
        };

        // Test case: first endpoint is not healthy, but others are
        let mut rotator = RpcEndpointRotator::new(&chains_config).unwrap();
        rotator
            .mark_unhealthy(&endpoint1, ChainType::Polygon)
            .await;

        let result = rotator
            .get_endpoint_url(ChainType::Polygon)
            .await;
        // HashMap is not sorted, so any healthy endpoint may be returned
        assert!(result == endpoint2 || result == endpoint3);

        // Test case: all endpoints are unhealthy
        let endpoint1 = RpcEndpoint {
            url: "http://test1.com".to_string(),
            next_retry_at: Some(Utc::now() + Duration::from_hours(3)),
            status: RpcEndpointStatus::Unhealthy,
            ..rpc_endpoint()
        };

        let endpoint2 = RpcEndpoint {
            url: "http://test2.com".to_string(),
            next_retry_at: Some(Utc::now() + Duration::from_hours(2)),
            status: RpcEndpointStatus::Unhealthy,
            ..rpc_endpoint()
        };

        let endpoint3 = RpcEndpoint {
            url: "http://test3.com".to_string(),
            next_retry_at: Some(Utc::now() + Duration::from_hours(1)),
            status: RpcEndpointStatus::Unhealthy,
            ..rpc_endpoint()
        };

        let endpoints = vec![endpoint1, endpoint2, endpoint3];
        rotator.polygon_endpoints = Arc::new(RwLock::new(endpoints));

        let result = rotator
            .get_endpoint_url(ChainType::Polygon)
            .await;
        assert_eq!("http://test3.com", &result);

        // Test case: check that expected endpoints are returned
        // when some of them got healthy
        let endpoint1 = RpcEndpoint {
            url: "http://test1.com".to_string(),
            next_retry_at: Some(Utc::now() - Duration::from_hours(3)),
            status: RpcEndpointStatus::Unhealthy,
            ..rpc_endpoint()
        };

        let endpoint2 = RpcEndpoint {
            url: "http://test2.com".to_string(),
            next_retry_at: Some(Utc::now() + Duration::from_hours(2)),
            status: RpcEndpointStatus::Unhealthy,
            ..rpc_endpoint()
        };

        let endpoint3 = RpcEndpoint {
            url: "http://test3.com".to_string(),
            next_retry_at: Some(Utc::now() + Duration::from_hours(1)),
            status: RpcEndpointStatus::Unhealthy,
            ..rpc_endpoint()
        };

        let endpoints = vec![endpoint1, endpoint2, endpoint3];
        rotator.polygon_endpoints = Arc::new(RwLock::new(endpoints));
        rotator.heal_all_endpoints().await;

        let result = rotator
            .get_endpoint_url(ChainType::Polygon)
            .await;
        assert_eq!("http://test1.com", &result);
    }
}
