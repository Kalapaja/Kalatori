use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{
    DateTime,
    Utc,
};
use tokio::sync::RwLock;

use crate::chain_client::ClientError;

const TIMEOUT: u64 = 60; // 1 minute
const HEALTH_CHECK_DELAY: Duration = Duration::from_secs(60);

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

    fn is_get_healthy(&self) -> bool {
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

#[derive(Debug)]
pub struct RpcEndpointRotator {
    endpoints: HashMap<String, RpcEndpoint>,
}

impl RpcEndpointRotator {
    pub fn new(endpoints: Vec<String>) -> Result<RpcEndpointRotator, ClientError> {
        if endpoints.is_empty() {
            return Err(ClientError::InvalidConfiguration {
                field: "Endpoints cannot be empty".to_string(),
            })
        }

        let endpoints = endpoints
            .into_iter()
            .map(|url| {
                let endpoint = RpcEndpoint {
                    url: url.clone(),
                    attempts: 0,
                    status: RpcEndpointStatus::Healthy,
                    last_attempt_at: None,
                    next_retry_at: None,
                };

                (url, endpoint)
            })
            .collect();

        Ok(Self {
            endpoints,
        })
    }

    pub fn get_endpoint_url(&self) -> String {
        for (url, endpoint) in &self.endpoints {
            if matches!(
                endpoint.status,
                RpcEndpointStatus::Healthy
            ) {
                return url.clone()
            }
        }

        // we checked that endpoints are not empty during initialization
        // so it's safe to unwrap here
        self.endpoints
            .values()
            .min_by_key(|endpoint| endpoint.next_retry_at)
            .unwrap()
            .url
            .clone()
    }

    pub fn mark_unhealthy(
        &mut self,
        url: &str,
    ) {
        match self.endpoints.get_mut(url) {
            Some(endpoint) => endpoint.increment_retry(),
            None => tracing::warn!("Failed to increment retry. Endpoint {url} not found"),
        }
    }

    pub fn heal_endpoints(&mut self) {
        self.endpoints
            .values_mut()
            .filter(|endpoint| endpoint.is_get_healthy())
            .for_each(|endpoint| endpoint.mark_healthy());
    }
}

pub async fn rpc_endpoints_health_check(
    rotators: Vec<Arc<RwLock<RpcEndpointRotator>>>,
    cancellation_token: tokio_util::sync::CancellationToken,
) {
    let mut interval = tokio::time::interval(HEALTH_CHECK_DELAY);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                for rotator in &rotators {
                    let mut lock = rotator.write().await;
                    lock.heal_endpoints();
                }
            },
            () = cancellation_token.cancelled() => {
                tracing::info!(
                    "Rpc endpoints health checker received cancellation signal, shutting down"
                );
                break;
            },
        }
    }
}

#[cfg(test)]
mod test {
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
        assert!(endpoint.is_get_healthy());

        endpoint.next_retry_at = Some(Utc::now() + Duration::from_hours(1));
        assert!(!endpoint.is_get_healthy());

        endpoint.next_retry_at = Some(Utc::now() - Duration::from_hours(1));
        assert!(endpoint.is_get_healthy());
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

    #[test]
    fn test_get_endpoint_url() {
        // Test case: first endpoint is not healthy, but others are
        let endpoint1 = "http://test1.com".to_string();
        let endpoint2 = "http://test2.com".to_string();
        let endpoint3 = "http://test3.com".to_string();

        let endpoints = vec![endpoint1.clone(), endpoint2.clone(), endpoint3.clone()];
        let mut rotator = RpcEndpointRotator::new(endpoints).unwrap();
        rotator.mark_unhealthy(&endpoint1);

        let result = rotator.get_endpoint_url();
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

        let endpoints = vec![endpoint1, endpoint2, endpoint3]
            .into_iter()
            .map(|item| (item.url.clone(), item))
            .collect();
        rotator.endpoints = endpoints;

        let result = rotator.get_endpoint_url();
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

        let endpoints = vec![endpoint1, endpoint2, endpoint3]
            .into_iter()
            .map(|item| (item.url.clone(), item))
            .collect();
        rotator.endpoints = endpoints;
        rotator.heal_endpoints();

        let result = rotator.get_endpoint_url();
        assert_eq!("http://test1.com", &result);
    }
}
