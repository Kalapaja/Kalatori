//! Fee decision client.
//!
//! Resolves per-payout fee parameters (fee wallet address + rate) from
//! an optional external service, with a config-based fallback.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use alloy::primitives::Address;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::configs::FeeConfig;
use crate::types::ChainType;
use crate::utils::logging::category;

/// Hard cap on fee_bps returned by the fee service (1%).
pub const MAX_FEE_BPS: u16 = 100;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Where the fee decision originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeeSource {
    /// Resolved by the external fee service.
    Service,
    /// Fallback: taken directly from local config.
    Config,
}

impl FeeSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Service => "Service",
            Self::Config => "Config",
        }
    }
}

impl std::fmt::Display for FeeSource {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for FeeSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Service" => Ok(Self::Service),
            "Config" => Ok(Self::Config),
            _ => Err(format!("Unknown fee source: {s}")),
        }
    }
}

/// Fee parameters to apply to a payout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeDecision {
    pub fee_wallet: Address,
    pub fee_bps: u16,
    pub source: FeeSource,
}

#[derive(Debug)]
struct CachedDecision {
    decision: Option<FeeDecision>,
    fetched_at: Instant,
    /// Server-specified expiry converted from the response's `valid_until` Unix timestamp.
    valid_until: SystemTime,
}

/// Resolves fee parameters for each payout.
///
/// Resolution order per chain:
/// 1. Return cached service response if within TTL.
/// 2. Call the fee service if a URL is configured for that chain.
/// 3. On service failure (or no URL), use the chain's config `fee_wallet`/`fee_bps`.
/// 4. If the chain has no entry, return `None` (no fee).
#[derive(Clone, Debug)]
pub struct FeeClient {
    http_client: reqwest::Client,
    client_id: Option<String>,
    config: FeeConfig,
    cache: Arc<RwLock<HashMap<ChainType, CachedDecision>>>,
}

#[derive(Serialize)]
struct FeeRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    client_id: Option<&'a str>,
    chain: ChainType,
    amount: String,
}

#[derive(Deserialize)]
struct FeeResponse {
    fee_wallet: Address,
    fee_bps: u16,
    /// Unix timestamp (seconds) until which this response may be cached.
    valid_until: u64,
}

fn cap_fee_bps(fee_bps: u16, source: FeeSource) -> u16 {
    if fee_bps <= MAX_FEE_BPS {
        return fee_bps;
    }
    tracing::error!(
        error.category = category::FEE,
        fee_bps,
        max = MAX_FEE_BPS,
        ?source,
        "fee_bps exceeds hard limit, capping to max"
    );
    MAX_FEE_BPS
}

impl FeeClient {
    pub fn new(config: FeeConfig, client_id: Option<String>) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http_client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
            client_id,
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Decide fee for a payout. Returns `None` if no fee applies for this chain.
    pub async fn decide(
        &self,
        chain: ChainType,
        amount: Decimal,
    ) -> Option<FeeDecision> {
        let chain_cfg = self.config.get(&chain)?;

        if let Some(url) = &chain_cfg.fee_service_url {
            // Check cache first — both the client TTL and the server-specified
            // valid_until must still be in the future.
            {
                let now = SystemTime::now();
                let cache = self.cache.read().await;
                if let Some(entry) = cache.get(&chain)
                    && entry.fetched_at.elapsed() < CACHE_TTL
                    && entry.valid_until > now
                {
                    return entry.decision.clone();
                }
            }

            // Cache miss or expired — call service.
            match self.call_service(url, chain, amount).await {
                Ok((decision, valid_until)) => {
                    self.cache.write().await.insert(chain, CachedDecision {
                        decision: decision.clone(),
                        fetched_at: Instant::now(),
                        valid_until,
                    });
                    return decision;
                }
                Err(e) => {
                    tracing::warn!(
                        error.category = category::FEE,
                        error.source = ?e,
                        "Fee service call failed, falling back to config values"
                    );
                }
            }
        }

        Some(FeeDecision {
            fee_wallet: chain_cfg.fee_wallet,
            fee_bps: cap_fee_bps(chain_cfg.fee_bps, FeeSource::Config),
            source: FeeSource::Config,
        })
    }

    async fn call_service(
        &self,
        url: &str,
        chain: ChainType,
        amount: Decimal,
    ) -> Result<(Option<FeeDecision>, SystemTime), reqwest::Error> {
        let request = FeeRequest {
            client_id: self.client_id.as_deref(),
            chain,
            amount: amount.to_string(),
        };

        let response: FeeResponse = self
            .http_client
            .post(url)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let valid_until = SystemTime::UNIX_EPOCH + Duration::from_secs(response.valid_until);
        let fee_bps = cap_fee_bps(response.fee_bps, FeeSource::Service);

        if fee_bps == 0 {
            return Ok((None, valid_until));
        }

        Ok((
            Some(FeeDecision {
                fee_wallet: response.fee_wallet,
                fee_bps,
                source: FeeSource::Service,
            }),
            valid_until,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use alloy::primitives::Address;
    use httpmock::prelude::*;
    use rust_decimal::Decimal;

    use crate::configs::ChainFeeConfig;
    use crate::types::ChainType;

    use super::*;

    /// Far-future Unix timestamp so cache entries never expire during tests.
    const VALID_UNTIL_FAR: u64 = 9_999_999_999;

    fn polygon_config(fee_bps: u16, fee_service_url: Option<String>) -> FeeConfig {
        let mut cfg = HashMap::new();
        cfg.insert(ChainType::Polygon, ChainFeeConfig {
            fee_wallet: Address::ZERO,
            fee_bps,
            fee_service_url,
        });
        cfg
    }

    fn make_client(config: FeeConfig) -> FeeClient {
        FeeClient::new(config, None).expect("failed to build FeeClient")
    }

    // ── FeeSource ─────────────────────────────────────────────────────────────

    #[test]
    fn fee_source_roundtrip() {
        for src in [FeeSource::Service, FeeSource::Config] {
            let s = src.to_string();
            let parsed: FeeSource = s.parse().expect("roundtrip failed");
            assert_eq!(parsed, src);
        }
    }

    #[test]
    fn fee_source_unknown_str_errors() {
        assert!("unknown".parse::<FeeSource>().is_err());
    }

    // ── cap_fee_bps ───────────────────────────────────────────────────────────

    #[test]
    fn cap_fee_bps_within_limit_passes_through() {
        assert_eq!(cap_fee_bps(MAX_FEE_BPS, FeeSource::Config), MAX_FEE_BPS);
        assert_eq!(cap_fee_bps(0, FeeSource::Config), 0);
        assert_eq!(cap_fee_bps(50, FeeSource::Service), 50);
    }

    #[test]
    fn cap_fee_bps_over_limit_clamps_to_max() {
        assert_eq!(cap_fee_bps(101, FeeSource::Config), MAX_FEE_BPS);
        assert_eq!(cap_fee_bps(u16::MAX, FeeSource::Service), MAX_FEE_BPS);
    }

    // ── FeeClient::decide — no service URL ───────────────────────────────────

    #[tokio::test]
    async fn decide_no_service_url_returns_config_fallback() {
        let client = make_client(polygon_config(40, None));
        let decision = client.decide(ChainType::Polygon, Decimal::new(100, 0)).await;
        assert_eq!(decision, Some(FeeDecision {
            fee_wallet: Address::ZERO,
            fee_bps: 40,
            source: FeeSource::Config,
        }));
    }

    #[tokio::test]
    async fn decide_unknown_chain_returns_none() {
        // Config only has Polygon; asking for PolkadotAssetHub returns None.
        let client = make_client(polygon_config(40, None));
        let decision = client
            .decide(ChainType::PolkadotAssetHub, Decimal::new(100, 0))
            .await;
        assert!(decision.is_none());
    }

    // ── FeeClient::decide — with service ─────────────────────────────────────

    #[tokio::test]
    async fn decide_service_success_returns_service_decision() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(200).json_body(serde_json::json!({
                "fee_wallet": "0x0000000000000000000000000000000000000001",
                "fee_bps": 50,
                "valid_until": VALID_UNTIL_FAR,
            }));
        });

        let client = make_client(polygon_config(40, Some(server.base_url())));
        let decision = client.decide(ChainType::Polygon, Decimal::new(100, 0)).await;

        mock.assert_calls(1);
        assert_eq!(decision, Some(FeeDecision {
            fee_wallet: "0x0000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            fee_bps: 50,
            source: FeeSource::Service,
        }));
    }

    #[tokio::test]
    async fn decide_service_fee_bps_zero_returns_none() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(200).json_body(serde_json::json!({
                "fee_wallet": Address::ZERO,
                "fee_bps": 0u16,
                "valid_until": VALID_UNTIL_FAR,
            }));
        });

        let client = make_client(polygon_config(40, Some(server.base_url())));
        let decision = client.decide(ChainType::Polygon, Decimal::new(100, 0)).await;
        assert!(decision.is_none());
    }

    #[tokio::test]
    async fn decide_service_fee_bps_over_cap_is_clamped() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(200).json_body(serde_json::json!({
                "fee_wallet": "0x0000000000000000000000000000000000000001",
                "fee_bps": 200u16,
                "valid_until": VALID_UNTIL_FAR,
            }));
        });

        let client = make_client(polygon_config(40, Some(server.base_url())));
        let decision = client.decide(ChainType::Polygon, Decimal::new(100, 0)).await;
        assert_eq!(decision.map(|d| d.fee_bps), Some(MAX_FEE_BPS));
    }

    #[tokio::test]
    async fn decide_service_failure_falls_back_to_config() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(500);
        });

        let client = make_client(polygon_config(40, Some(server.base_url())));
        let decision = client.decide(ChainType::Polygon, Decimal::new(100, 0)).await;

        mock.assert_calls(1);
        assert_eq!(decision, Some(FeeDecision {
            fee_wallet: Address::ZERO,
            fee_bps: 40,
            source: FeeSource::Config,
        }));
    }

    // ── Cache behaviour ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn decide_caches_service_response() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(200).json_body(serde_json::json!({
                "fee_wallet": "0x0000000000000000000000000000000000000001",
                "fee_bps": 50,
                "valid_until": VALID_UNTIL_FAR,
            }));
        });

        let client = make_client(polygon_config(40, Some(server.base_url())));
        let amount = Decimal::new(100, 0);

        let first = client.decide(ChainType::Polygon, amount).await;
        let second = client.decide(ChainType::Polygon, amount).await;

        // Service should only be called once; second call hits cache.
        mock.assert_calls(1);
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn decide_cache_expired_by_valid_until_refetches() {
        let server = MockServer::start();
        // valid_until = 1 second past the Unix epoch — already expired.
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(200).json_body(serde_json::json!({
                "fee_wallet": "0x0000000000000000000000000000000000000001",
                "fee_bps": 50,
                "valid_until": 1u64,
            }));
        });

        let client = make_client(polygon_config(40, Some(server.base_url())));
        let amount = Decimal::new(100, 0);

        client.decide(ChainType::Polygon, amount).await;
        client.decide(ChainType::Polygon, amount).await;

        // Both calls should hit the service because valid_until is in the past.
        mock.assert_calls(2);
    }
}
