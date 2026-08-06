use std::collections::{
    HashMap,
    HashSet,
};
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::str::FromStr;

use kalatori_client::strum::IntoEnumIterator;
use rand::prelude::*;
use rust_decimal::Decimal;
use secrecy::SecretString;
use serde::{
    Deserialize,
    Serialize,
};

use crate::chain::utils::to_base58_string;
use crate::types::{
    ChainType,
    DetectedShopPlatform,
};

use super::consts::{
    DEFAULT_ALLOW_INSECURE_ENDPOINTS,
    DEFAULT_ASSET_HUB_ASSET_ID,
    DEFAULT_AUTH_CLOCK_TOLERANCE_SECS,
    DEFAULT_CHAIN,
    DEFAULT_DATABASE_DIR,
    DEFAULT_ETHERSCAN_LIMIT_PER_SECOND,
    DEFAULT_HOST,
    DEFAULT_INVOICE_LIFETIME_MILLIS,
    DEFAULT_LOG_DIRECTIVES,
    DEFAULT_OVERPAYMENT_TOLERANCE,
    DEFAULT_POLKADOT_ASSET_HUB_ENDPOINTS,
    DEFAULT_POLYGON_CONFIRMATIONS,
    DEFAULT_POLYGON_ENDPOINTS,
    DEFAULT_POLYGON_USDC_ADDRESS,
    DEFAULT_PORT,
    DEFAULT_SIGNATURE_MAX_AGE_SECS,
    DEFAULT_UNDERPAYMENT_TOLERANCE,
    DEFAULT_ZERO_EX_RPC_URL,
};

#[derive(Deserialize)]
pub struct SecretsConfig {
    /// IMPORTANT: we use the same seed for all chains for simplicity
    pub seed: SecretString,
    /// API secret key for securing API endpoints. Should be the same as in the
    /// e-commerce platform
    pub api_secret_key: SecretString,
}

fn default_allow_insecure_endpoints() -> bool {
    DEFAULT_ALLOW_INSECURE_ENDPOINTS
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum EndpointAllowedOperation {
    Subscriptions,
    Requests,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum ChainEndpoint {
    Universal(String),
    Specific {
        url: String,
        operations: Vec<EndpointAllowedOperation>,
    },
}

fn default_confirmations() -> u64 {
    DEFAULT_POLYGON_CONFIRMATIONS
}

/// Normalize an EVM address to its EIP-55 checksummed form. Chain clients
/// report addresses in this form, so configured addresses must match it
/// byte-for-byte to be comparable as strings.
fn checksummed_evm_address(address: &str) -> Result<String, String> {
    address
        .parse::<alloy::primitives::Address>()
        .map(|addr| addr.to_checksum(None))
        .map_err(|_| format!("Invalid EVM asset address: {address}"))
}

// TODO: add some docs for fields, their purpose might be not obvious
#[derive(Deserialize, Clone, Debug)]
pub struct ChainConfig {
    /// RPC endpoints for the chain node. Can be left empty to use defaults.
    #[serde(default)]
    pub endpoints: Vec<ChainEndpoint>,
    /// List of asset IDs to monitor on this chain. Can be left empty. By
    /// default the default asset ID for the chain will be added. If the
    /// default asset ID is changed in PaymentsConfig but in database there
    /// are not finished invoices, the old asset ID will be also added
    /// automatically.
    #[serde(default)]
    pub assets: Vec<String>,
    /// Allow endpoints which starts from `http://` and `ws://` instead of `https://` and `wss://`
    #[serde(default = "default_allow_insecure_endpoints")]
    pub allow_insecure_endpoints: bool,
    /// How many blocks an incoming transfer must stay on-chain before it's
    /// treated as received. Only used for chains with probabilistic finality
    /// (Polygon); Asset Hub tracks finalized blocks and ignores this. Set to 0
    /// to act on transfers as soon as the next block arrives.
    #[serde(default = "default_confirmations")]
    pub confirmations: u64,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            endpoints: Vec::new(),
            assets: Vec::new(),
            allow_insecure_endpoints: DEFAULT_ALLOW_INSECURE_ENDPOINTS,
            confirmations: DEFAULT_POLYGON_CONFIRMATIONS,
        }
    }
}

impl ChainConfig {
    fn get_endpoints_with_allowed_operation(
        &self,
        op: EndpointAllowedOperation,
    ) -> impl Iterator<Item = &String> {
        self.endpoints
            .iter()
            .flat_map(move |ep| match ep {
                ChainEndpoint::Universal(url) => Some(url),
                ChainEndpoint::Specific {
                    url,
                    operations,
                } if operations.contains(&op) => Some(url),
                _ => None,
            })
    }

    pub fn get_random_requests_endpoint(&self) -> Option<String> {
        let mut rng = rand::rng();

        self.get_endpoints_with_allowed_operation(EndpointAllowedOperation::Requests)
            .choose(&mut rng)
            .cloned()
    }

    pub fn get_random_subscriptions_endpoint(&self) -> Option<String> {
        let mut rng = rand::rng();

        self.get_endpoints_with_allowed_operation(EndpointAllowedOperation::Subscriptions)
            .choose(&mut rng)
            .cloned()
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct ChainsConfig {
    /// Configuration per supported chain. See `ChainConfig` for details.
    #[serde(default)]
    pub chains: HashMap<ChainType, ChainConfig>,
}

impl ChainsConfig {
    /// Extend chains config with default asset IDs from payments config
    #[expect(
        clippy::unwrap_used,
        reason = "startup config validation: a chain without a default asset id or without a chain config is an unusable configuration, and refusing to start is the intended outcome"
    )]
    pub fn add_default_asset_ids(
        &mut self,
        default_asset_ids: &HashMap<ChainType, String>,
    ) {
        for chain_type in ChainType::iter() {
            let default_asset_id = default_asset_ids
                .get(&chain_type)
                .unwrap();
            let chain_config = self
                .chains
                .get_mut(&chain_type)
                .unwrap();

            if !chain_config
                .assets
                .contains(default_asset_id)
            {
                chain_config
                    .assets
                    .push(default_asset_id.clone());
            }
        }
    }

    /// Extend chains config with asset IDs of restored invoices from the
    /// database
    #[expect(
        clippy::unwrap_used,
        reason = "startup: restored invoices reference a chain that is no longer configured, which the operator has to resolve before the daemon can run"
    )]
    pub fn add_restored_asset_ids(
        &mut self,
        restored_asset_ids: HashMap<ChainType, HashSet<String>>,
    ) {
        for (chain_type, asset_ids) in restored_asset_ids {
            let chain_config = self
                .chains
                .get_mut(&chain_type)
                .unwrap();

            for asset_id in asset_ids {
                // Old invoices may store the address as it was cased in the
                // config back then; canonicalize so it doesn't duplicate the
                // configured entry
                let asset_id = if chain_type == ChainType::Polygon {
                    match checksummed_evm_address(&asset_id) {
                        Ok(canonical) => canonical,
                        Err(error) => {
                            tracing::warn!(
                                %asset_id,
                                %error,
                                "Restored asset id is not a valid EVM address, keeping it as-is"
                            );
                            asset_id
                        },
                    }
                } else {
                    asset_id
                };

                if !chain_config.assets.contains(&asset_id) {
                    chain_config.assets.push(asset_id);
                }
            }
        }
    }

    /// Canonicalize configured EVM asset addresses to their checksummed form
    /// so a differently-cased config entry still matches addresses reported
    /// by the chain clients.
    pub fn canonicalize_evm_asset_ids(&mut self) -> Result<(), String> {
        let Some(chain_config) = self.chains.get_mut(&ChainType::Polygon) else {
            return Ok(());
        };

        for asset_id in &mut chain_config.assets {
            *asset_id = checksummed_evm_address(asset_id)?;
        }

        // Canonicalization can collapse entries that differed only in casing
        let mut seen = HashSet::new();
        chain_config
            .assets
            .retain(|asset_id| seen.insert(asset_id.clone()));

        Ok(())
    }

    pub(super) fn set_default_chains_if_missing(&mut self) {
        for chain in ChainType::iter() {
            let chain_config = self.chains.entry(chain).or_default();

            if chain_config.endpoints.is_empty() {
                let endpoints = match chain {
                    ChainType::PolkadotAssetHub => DEFAULT_POLKADOT_ASSET_HUB_ENDPOINTS,
                    ChainType::Polygon => DEFAULT_POLYGON_ENDPOINTS,
                };

                // These are free public endpoints with no availability guarantee.
                // An operator who never configured any has no other way to find
                // out which node their payments depend on.
                tracing::warn!(
                    chain = %chain,
                    endpoints = ?endpoints,
                    "No endpoints configured, falling back to free public defaults. \
                     Configure your own endpoints for production use."
                );

                chain_config.endpoints = endpoints
                    .iter()
                    .map(|s| ChainEndpoint::Universal(s.to_string()))
                    .collect();
            }
        }
    }
}

fn default_chain() -> ChainType {
    DEFAULT_CHAIN
}

fn default_invoice_lifetime_millis() -> u64 {
    DEFAULT_INVOICE_LIFETIME_MILLIS
}

// TODO: add validations for that params. At least we have to ensure that they
// are not negative. Ideally, we have to also validate their estimate price and
// don't allow to exceed it some constant amount like 5 dollars or something
// similar. Also, later we'll probably add some minimal invoice amount. We'll
// have to ensure that tolerance doesn't allow to avoid invoice payment at all
// (or pay just the very minimal amount).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct SlippageParams {
    /// Maximum amount below the expected payment that will still be accepted.
    /// If set to 0, will require exact amount or more. By default is 0.
    #[serde(default)]
    pub underpayment_tolerance: Decimal,
    /// Maximum acceptable overpayment before triggering a partial refund of the
    /// excess amount. If set to 0, will trigger partial refund for any
    /// overpayment. By default is 0.
    #[serde(default)]
    pub overpayment_tolerance: Decimal,
}

// TODO: add some docs for fields, their purpose might be not obvious
#[derive(Deserialize, Clone, Debug)]
pub struct PaymentsConfig {
    /// Address to which payments will be sent after invoice paid,
    /// separate address per chain. Should always be set for default chain.
    /// If default chain is changed but there are not finished invoices in the
    /// database, the old default chain's recipient address will be also
    /// required.
    pub recipient: HashMap<ChainType, String>,
    /// Invoice lifetime in milliseconds. Default is 24 hours.
    #[serde(default = "default_invoice_lifetime_millis")]
    pub invoice_lifetime_millis: u64,
    /// Default chain to use for invoices. Default is Polkadot Asset Hub.
    #[serde(default = "default_chain")]
    pub default_chain: ChainType,
    /// Default asset IDs per chain. Can be left empty to use built-in defaults.
    #[serde(default)]
    pub default_asset_id: HashMap<ChainType, String>,
    /// Base URL for payment links, e.g. "https://shop.example.com". Should be an address of Kalatori instance.
    pub payment_url_base: String,
    /// Slippage parameters can be configured for each specific asset. If not
    /// set, default settings will be used.
    #[serde(default)]
    pub slippage_params: HashMap<ChainType, HashMap<String, SlippageParams>>,
}

impl PaymentsConfig {
    pub(super) fn set_default_asset_id_if_missing(&mut self) {
        for chain in ChainType::iter() {
            let default = match chain {
                ChainType::PolkadotAssetHub => DEFAULT_ASSET_HUB_ASSET_ID,
                ChainType::Polygon => DEFAULT_POLYGON_USDC_ADDRESS,
            };

            self.default_asset_id
                .entry(chain)
                .or_insert(default.to_string());

            self.slippage_params
                .entry(chain)
                .or_default()
                .entry(default.to_string())
                .or_insert(SlippageParams {
                    underpayment_tolerance: DEFAULT_UNDERPAYMENT_TOLERANCE,
                    overpayment_tolerance: DEFAULT_OVERPAYMENT_TOLERANCE,
                });
        }
    }

    /// Validate that all recipient addresses are valid for their respective
    /// chains
    pub fn validate_recipients(
        &mut self,
        chains: &[ChainType],
    ) -> Result<(), String> {
        for chain in chains {
            let recipient = self
                .recipient
                .get(chain)
                .ok_or_else(|| {
                    format!(
                        "Recipient address for chain {:?} is missing",
                        chain
                    )
                })?;

            match chain {
                ChainType::PolkadotAssetHub => {
                    // Validate Polkadot address (prefix 0)
                    let account_id =
                        subxt::utils::AccountId32::from_str(recipient).map_err(|_| {
                            format!(
                                "Invalid Polkadot address: {}",
                                recipient
                            )
                        })?;

                    // Re-encode to ensure correct format
                    self.recipient.insert(
                        *chain,
                        to_base58_string(account_id.0, 0),
                    );
                },
                ChainType::Polygon => {
                    // Validate Ethereum/Polygon address (0x-prefixed hex, 20 bytes)
                    let address = recipient
                        .parse::<alloy::primitives::Address>()
                        .map_err(|_| format!("Invalid Polygon address: {}", recipient))?;

                    // Store checksummed version for consistency
                    self.recipient
                        .insert(*chain, address.to_checksum(None));
                },
            }
        }

        Ok(())
    }

    /// Canonicalize configured EVM asset addresses (default asset and
    /// slippage keys) to their checksummed form so lookups by chain-reported
    /// addresses match regardless of how the config file cased them.
    pub fn canonicalize_evm_asset_ids(&mut self) -> Result<(), String> {
        if let Some(asset_id) = self
            .default_asset_id
            .get_mut(&ChainType::Polygon)
        {
            *asset_id = checksummed_evm_address(asset_id)?;
        }

        if let Some(params) = self
            .slippage_params
            .remove(&ChainType::Polygon)
        {
            let mut canonical = HashMap::with_capacity(params.len());

            for (asset_id, slippage) in params {
                canonical.insert(
                    checksummed_evm_address(&asset_id)?,
                    slippage,
                );
            }

            self.slippage_params
                .insert(ChainType::Polygon, canonical);
        }

        Ok(())
    }

    pub fn get_asset_slippage_params(
        &self,
        chain: ChainType,
        asset_id: &str,
    ) -> SlippageParams {
        self.slippage_params
            .get(&chain)
            .and_then(|map| {
                map.get(asset_id).or_else(|| {
                    // EVM hex addresses are case-insensitive (casing is only
                    // a checksum) and invoices restored from older databases
                    // may carry a differently-cased asset id
                    (chain == ChainType::Polygon)
                        .then(|| {
                            map.iter()
                                .find(|(key, _)| key.eq_ignore_ascii_case(asset_id))
                                .map(|(_, params)| params)
                        })
                        .flatten()
                })
            })
            .copied()
            .unwrap_or_default()
    }

    pub fn get_asset_underpayment_tolerance(
        &self,
        chain: ChainType,
        asset_id: &str,
    ) -> Decimal {
        self.get_asset_slippage_params(chain, asset_id)
            .underpayment_tolerance
    }

    pub fn get_asset_overpayment_tolerance(
        &self,
        chain: ChainType,
        asset_id: &str,
    ) -> Decimal {
        self.get_asset_slippage_params(chain, asset_id)
            .overpayment_tolerance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USDC_LOWER: &str = "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359";
    const USDC_CHECKSUMMED: &str = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";
    const USDT_LOWER: &str = "0xc2132d05d31c914a87c6611c10748aeb04b58e8f";
    const USDT_CHECKSUMMED: &str = "0xc2132D05D31c914a87C6611C10748AEb04B58e8F";

    #[test]
    fn test_canonicalize_payments_evm_asset_ids() {
        let slippage = SlippageParams {
            underpayment_tolerance: Decimal::new(5, 1),
            overpayment_tolerance: Decimal::ZERO,
        };

        let mut config = PaymentsConfig {
            recipient: HashMap::new(),
            invoice_lifetime_millis: 1,
            default_chain: ChainType::Polygon,
            default_asset_id: HashMap::from([
                (
                    ChainType::Polygon,
                    USDC_LOWER.to_string(),
                ),
                (
                    ChainType::PolkadotAssetHub,
                    "1337".to_string(),
                ),
            ]),
            payment_url_base: String::new(),
            slippage_params: HashMap::from([(
                ChainType::Polygon,
                HashMap::from([(USDT_LOWER.to_string(), slippage)]),
            )]),
        };

        config
            .canonicalize_evm_asset_ids()
            .unwrap();

        assert_eq!(
            config.default_asset_id[&ChainType::Polygon],
            USDC_CHECKSUMMED
        );
        // Non-EVM chains are untouched
        assert_eq!(
            config.default_asset_id[&ChainType::PolkadotAssetHub],
            "1337"
        );
        assert!(config.slippage_params[&ChainType::Polygon].contains_key(USDT_CHECKSUMMED));

        // Lookups keep working for historic lowercase asset ids
        let found = config.get_asset_slippage_params(ChainType::Polygon, USDT_LOWER);
        assert_eq!(
            found.underpayment_tolerance,
            slippage.underpayment_tolerance
        );

        // Invalid EVM addresses are rejected
        config.default_asset_id.insert(
            ChainType::Polygon,
            "not-an-address".to_string(),
        );
        assert!(
            config
                .canonicalize_evm_asset_ids()
                .is_err()
        );
    }

    #[test]
    fn test_canonicalize_chains_evm_asset_ids() {
        let mut config = ChainsConfig {
            chains: HashMap::from([(
                ChainType::Polygon,
                ChainConfig {
                    // The same asset cased two ways plus another asset
                    assets: vec![
                        USDC_LOWER.to_string(),
                        USDC_CHECKSUMMED.to_string(),
                        USDT_LOWER.to_string(),
                    ],
                    ..Default::default()
                },
            )]),
        };

        config
            .canonicalize_evm_asset_ids()
            .unwrap();

        assert_eq!(
            config.chains[&ChainType::Polygon].assets,
            vec![USDC_CHECKSUMMED.to_string(), USDT_CHECKSUMMED.to_string(),]
        );
    }

    // --- Public-default endpoint visibility ---

    const FALLBACK_MESSAGE: &str = "falling back to free public defaults";
    const SWAPS_FALLBACK_MESSAGE: &str = "Swaps are using a free public RPC endpoint";

    /// Count fallback warnings naming `chain`. Matching on level, message and
    /// the `chain` field together is deliberate: a substring search over the
    /// whole buffer would be satisfied by a single ambiguous INFO event, and
    /// would not notice a warning fired for the wrong chain.
    fn fallback_warnings_for(
        logs: &[&str],
        chain: ChainType,
    ) -> usize {
        let field = format!("chain={chain}");

        logs.iter()
            .filter(|log| {
                log.contains(" WARN ") && log.contains(FALLBACK_MESSAGE) && log.contains(&field)
            })
            .count()
    }

    fn assert_warned_once(
        logs: &[&str],
        chain: ChainType,
    ) -> Result<(), String> {
        match fallback_warnings_for(logs, chain) {
            1 => Ok(()),
            other => Err(format!(
                "expected exactly one fallback WARN naming {chain}, found {other}"
            )),
        }
    }

    fn assert_not_warned(
        logs: &[&str],
        chain: ChainType,
    ) -> Result<(), String> {
        match fallback_warnings_for(logs, chain) {
            0 => Ok(()),
            other => Err(format!(
                "expected no fallback WARN naming {chain}, found {other}"
            )),
        }
    }

    fn chains_config_from_json(json: &str) -> ChainsConfig {
        serde_json::from_str(json).unwrap()
    }

    fn configured(url: &str) -> ChainConfig {
        ChainConfig {
            endpoints: vec![ChainEndpoint::Universal(url.to_string())],
            ..ChainConfig::default()
        }
    }

    #[test]
    #[tracing_test::traced_test]
    fn every_unconfigured_chain_is_announced_exactly_once() {
        let mut config = chains_config_from_json(r#"{"chains":{}}"#);

        config.set_default_chains_if_missing();

        for chain in ChainType::iter() {
            assert!(
                config.chains[&chain]
                    .get_random_requests_endpoint()
                    .is_some(),
                "{chain} was left without endpoints"
            );
        }

        logs_assert(|logs| {
            assert_warned_once(logs, ChainType::Polygon)?;
            assert_warned_once(logs, ChainType::PolkadotAssetHub)
        });

        // The endpoints themselves have to be in the line — naming the chain
        // without naming the node leaves the operator no better off.
        assert!(logs_contain(
            "wss://polygon-bor-rpc.publicnode.com"
        ));
        assert!(logs_contain(
            "wss://asset-hub-polkadot-rpc.n.dwellir.com"
        ));
    }

    #[test]
    #[tracing_test::traced_test]
    fn configuring_polygon_silences_polygon_only() {
        let url = "wss://polygon.example.internal";
        let mut config = chains_config_from_json(r#"{"chains":{}}"#);
        config
            .chains
            .insert(ChainType::Polygon, configured(url));

        config.set_default_chains_if_missing();

        assert_eq!(
            config.chains[&ChainType::Polygon].get_random_requests_endpoint(),
            Some(url.to_string()),
        );
        logs_assert(|logs| {
            assert_not_warned(logs, ChainType::Polygon)?;
            assert_warned_once(logs, ChainType::PolkadotAssetHub)
        });
    }

    /// The mirror of the above. Asymmetric bugs — a loop that only ever reports
    /// the first chain, say — pass one direction and fail the other.
    #[test]
    #[tracing_test::traced_test]
    fn configuring_asset_hub_silences_asset_hub_only() {
        let url = "wss://asset-hub.example.internal";
        let mut config = chains_config_from_json(r#"{"chains":{}}"#);
        config.chains.insert(
            ChainType::PolkadotAssetHub,
            configured(url),
        );

        config.set_default_chains_if_missing();

        assert_eq!(
            config.chains[&ChainType::PolkadotAssetHub].get_random_requests_endpoint(),
            Some(url.to_string()),
        );
        logs_assert(|logs| {
            assert_not_warned(logs, ChainType::PolkadotAssetHub)?;
            assert_warned_once(logs, ChainType::Polygon)
        });
    }

    /// `endpoints: []` and an absent `endpoints` key reach `ChainConfig`
    /// through different serde paths — an explicit empty sequence versus the
    /// `#[serde(default)]` — but both mean "unconfigured" and must both warn.
    #[test]
    #[tracing_test::traced_test]
    fn an_explicitly_empty_endpoint_list_counts_as_unconfigured() {
        let mut config =
            chains_config_from_json(r#"{"chains":{"Polygon":{"endpoints":[],"assets":[]}}}"#);

        assert!(
            config.chains[&ChainType::Polygon]
                .endpoints
                .is_empty()
        );

        config.set_default_chains_if_missing();

        assert!(
            config.chains[&ChainType::Polygon]
                .get_random_requests_endpoint()
                .is_some()
        );
        logs_assert(|logs| assert_warned_once(logs, ChainType::Polygon));
    }

    /// Exercise the loader rather than the method directly, so the file/env
    /// precedence layer is covered too. The prefix is unique to this test and
    /// the directory does not exist, so nothing external can influence it.
    #[test]
    #[tracing_test::traced_test]
    fn the_loader_announces_defaults_too() {
        let config = crate::configs::chains_config_with_prefix(
            "no-such-config-dir",
            "KALATORI_TEST_ENDPOINT_FALLBACK",
        );

        for chain in ChainType::iter() {
            assert!(
                config.chains[&chain]
                    .get_random_requests_endpoint()
                    .is_some(),
                "{chain} was left without endpoints"
            );
        }

        logs_assert(|logs| {
            assert_warned_once(logs, ChainType::Polygon)?;
            assert_warned_once(logs, ChainType::PolkadotAssetHub)
        });
    }

    /// Swaps are the sibling fallback: `SwapsClients` is always constructed, so
    /// an operator who configured both payment chains and left `swaps.zero_ex`
    /// alone is still on a free public node.
    #[test]
    #[tracing_test::traced_test]
    fn the_default_swaps_rpc_is_announced() {
        SwapsConfig::default().warn_if_zero_ex_rpc_is_public_default();

        logs_assert(|logs| {
            let count = logs
                .iter()
                .filter(|log| log.contains(" WARN ") && log.contains(SWAPS_FALLBACK_MESSAGE))
                .count();

            if count == 1 {
                Ok(())
            } else {
                Err(format!(
                    "expected exactly one swaps fallback WARN, found {count}"
                ))
            }
        });
        assert!(logs_contain(DEFAULT_ZERO_EX_RPC_URL));
    }

    #[test]
    #[tracing_test::traced_test]
    fn a_configured_swaps_rpc_is_not_announced() {
        let config = SwapsConfig {
            zero_ex: ZeroExApiConfig {
                api_key: "not-a-real-key".into(),
                rpc_url: "https://polygon.example.internal".to_string(),
            },
            ..SwapsConfig::default()
        };

        config.warn_if_zero_ex_rpc_is_public_default();

        assert!(!logs_contain(SWAPS_FALLBACK_MESSAGE));
    }
}

fn default_host() -> IpAddr {
    DEFAULT_HOST
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

// TODO: configure enable/disable health/metrics/etc handlers?
#[derive(Deserialize, Debug)]
pub struct WebServerConfig {
    /// By default use 0.0.0.0
    #[serde(default = "default_host")]
    pub host: IpAddr,
    /// By default use port 8080
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_database_dir() -> String {
    DEFAULT_DATABASE_DIR.to_string()
}

#[derive(Deserialize, Clone)]
pub struct DatabaseConfig {
    #[serde(default = "default_database_dir")]
    pub dir: String,
    #[serde(default)]
    pub temporary: bool,
    #[serde(default)]
    pub require_existing: bool,
}

fn default_signature_max_age_secs() -> u64 {
    DEFAULT_SIGNATURE_MAX_AGE_SECS
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShopMetaConfig {
    pub shop_name: String,
    pub shop_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    pub reown_project_id: String,
    pub ankr_api_token: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct ShopConfig {
    #[serde(default)]
    pub invoices_webhook_url: Option<String>,
    #[serde(default = "default_signature_max_age_secs")]
    pub signature_max_age_secs: u64,
    #[serde(default)]
    pub private_api_base_url: Option<String>,
    #[serde(flatten)]
    pub meta: ShopMetaConfig,
    #[serde(default)]
    pub shop_platform: DetectedShopPlatform,
}

fn default_log_directives() -> String {
    DEFAULT_LOG_DIRECTIVES.to_string()
}

#[derive(Deserialize, Clone, Debug)]
pub struct LoggerConfig {
    #[serde(default = "default_log_directives")]
    pub directives: String,
    #[serde(default)]
    pub loki_url: Option<String>,
}

fn default_etherscan_limit_per_second() -> NonZeroU32 {
    DEFAULT_ETHERSCAN_LIMIT_PER_SECOND
}

#[derive(Deserialize, Clone, Debug)]
pub struct EtherscanClientConfig {
    #[serde(default = "default_etherscan_limit_per_second")]
    pub requests_per_second: NonZeroU32,
    pub api_key: SecretString,
}

// --- Auth config ---

fn default_auth_clock_tolerance_secs() -> u64 {
    DEFAULT_AUTH_CLOCK_TOLERANCE_SECS
}

/// OAuth configuration for the daemon's admin API.
///
/// If `auth.json` exists (or `KALATORI_AUTH_*` env vars are set), auth is
/// enabled. If not, auth is disabled and admin paths return 404.
#[derive(Deserialize)]
pub struct OAuthConfigRaw {
    /// Authorization server base URL (e.g. `https://app.kalatori.org`).
    pub auth_server_url: String,
    /// OAuth client identifier, assigned at daemon provisioning.
    pub client_id: String,
    /// Per-daemon shared secret for authenticating s2s calls.
    pub client_secret: SecretString,
    /// Previous secret, accepted during rotation window (see spec §10.3).
    #[serde(default)]
    pub previous_client_secret: Option<SecretString>,
    /// Ed25519 public keys in PASERK format (`k4.public.<data>`), max 2.
    pub token_public_keys: Vec<String>,
    /// Seconds of clock skew tolerance for exp/iat validation. Default: 30.
    #[serde(default = "default_auth_clock_tolerance_secs")]
    pub clock_tolerance: u64,
    /// Daemon's own public base URL (e.g. `https://bel-fantasy-01.kalatori.store`).
    /// Used to construct the redirect URI for the OAuth callback.
    pub base_url: String,
}

/// Validated OAuth configuration. All fields are guaranteed present and valid.
#[derive(Clone, Debug)]
pub struct OAuthConfig {
    /// Authorization server base URL, normalized (lowercase host, no trailing
    /// slash).
    pub auth_server_url: String,
    /// OAuth client identifier.
    pub client_id: String,
    /// Per-daemon shared secret for s2s calls.
    pub client_secret: SecretString,
    /// Previous secret during rotation window.
    pub previous_client_secret: Option<SecretString>,
    /// Ed25519 public keys in PASERK `k4.public.<data>` format (1 or 2).
    pub token_public_keys: Vec<String>,
    /// Clock skew tolerance in seconds.
    pub clock_tolerance: u64,
    /// Daemon's own public base URL, normalized.
    pub base_url: String,
}

impl OAuthConfig {
    /// Validate raw deserialized config.
    ///
    /// # Panics
    ///
    /// Panics if fields are invalid. This follows the existing config pattern
    /// where invalid config causes a startup panic with a descriptive message.
    pub fn from_raw(raw: OAuthConfigRaw) -> Self {
        let token_public_keys = raw.token_public_keys;

        assert!(
            !token_public_keys.is_empty(),
            "auth config: `token_public_keys` must contain at least one key"
        );

        assert!(
            token_public_keys.len() <= 2,
            "auth config: `token_public_keys` must contain at most 2 keys, got {}",
            token_public_keys.len()
        );

        for (i, key) in token_public_keys.iter().enumerate() {
            assert!(
                key.starts_with("k4.public."),
                "auth config: `token_public_keys[{i}]` must be a PASERK k4.public key, got: {key}"
            );
        }

        Self {
            auth_server_url: normalize_url(&raw.auth_server_url),
            client_id: raw.client_id,
            client_secret: raw.client_secret,
            previous_client_secret: raw.previous_client_secret,
            token_public_keys,
            clock_tolerance: raw.clock_tolerance,
            base_url: normalize_url(&raw.base_url),
        }
    }
}

/// Normalize a URL for consistent comparison: lowercase scheme and host, remove
/// trailing slash, keep explicit port only if non-default.
#[expect(
    clippy::panic,
    reason = "startup config validation: an unparsable auth server URL is unusable, and refusing to start is the intended outcome"
)]
fn normalize_url(url: &str) -> String {
    let url = url.trim_end_matches('/');

    // Parse to normalize scheme + host casing
    let Ok(parsed) = url::Url::parse(url) else {
        panic!("auth config: invalid URL: {url}");
    };

    let scheme = parsed.scheme();
    let host = parsed
        .host_str()
        .unwrap_or_else(|| panic!("auth config: URL has no host: {url}"));

    let is_default_port = matches!(
        (scheme, parsed.port()),
        ("https", None | Some(443)) | ("http", None | Some(80))
    );

    if is_default_port {
        format!("{scheme}://{host}")
    } else if let Some(port) = parsed.port() {
        format!("{scheme}://{host}:{port}")
    } else {
        format!("{scheme}://{host}")
    }
}

#[expect(dead_code)]
#[derive(Deserialize, Clone, Debug)]
pub struct IntegratorFees {
    // The address that will receive the collected fees
    fee_taker_address: String,
    // The percentage of the transfer amount to charge as a fee (in basis points - 1 basis point =
    // 0.01%)
    fee_bps: u16,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BungeeApiConfig {
    pub api_key: SecretString,
    pub affiliate: SecretString,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ZeroExApiConfig {
    pub api_key: SecretString,
    pub rpc_url: String,
}

// TODO: make zero ex api config (and client starting) optional
// with some backup which not require API keys and get rid of this default.
// Might be a problem if/when we'll move to some other chain
impl Default for ZeroExApiConfig {
    fn default() -> Self {
        Self {
            api_key: "".into(),
            rpc_url: DEFAULT_ZERO_EX_RPC_URL.to_string(),
        }
    }
}

impl SwapsConfig {
    /// Announce a public-default swaps RPC the same way
    /// `set_default_chains_if_missing` announces the chain endpoints.
    ///
    /// Startup always constructs `SwapsClients`, so omitting `swaps.zero_ex`
    /// puts the daemon on a free public node. Without this, an operator who
    /// configured both payment chains properly reads a clean log and concludes
    /// they are fully configured — which is the exact condition the chain
    /// warning exists to abolish.
    pub(super) fn warn_if_zero_ex_rpc_is_public_default(&self) {
        if self.zero_ex.rpc_url == DEFAULT_ZERO_EX_RPC_URL {
            tracing::warn!(
                rpc_url = %self.zero_ex.rpc_url,
                "Swaps are using a free public RPC endpoint. \
                 Set swaps.zero_ex.rpc_url to your own endpoint for production use."
            );
        }
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
pub struct SwapsConfig {
    #[serde(default)]
    pub bungee: Option<BungeeApiConfig>,
    #[serde(default)]
    pub zero_ex: ZeroExApiConfig,
    #[serde(default)]
    pub fees: Option<IntegratorFees>,
}
