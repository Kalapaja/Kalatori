use std::collections::{
    HashMap,
    HashSet,
};
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::str::FromStr;

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
    DEFAULT_POLKADOT_ASSET_HUB_ENDPOINTS,
    DEFAULT_POLYGON_ENDPOINTS,
    DEFAULT_POLYGON_USDC_ADDRESS,
    DEFAULT_PORT,
    DEFAULT_SIGNATURE_MAX_AGE_SECS,
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

// TODO: add some docs for fields, their purpose might be not obvious
#[derive(Deserialize, Clone, Debug, Default)]
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
                if !chain_config.assets.contains(&asset_id) {
                    chain_config.assets.push(asset_id);
                }
            }
        }
    }

    pub(super) fn set_default_chains_if_missing(&mut self) {
        for chain in ChainType::iter() {
            let chain_config = self.chains.entry(chain).or_default();

            if chain_config.endpoints.is_empty() {
                let endpoints = match chain {
                    ChainType::PolkadotAssetHub => DEFAULT_POLKADOT_ASSET_HUB_ENDPOINTS,
                    ChainType::Polygon => DEFAULT_POLYGON_ENDPOINTS,
                };

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

    pub fn get_asset_slippage_params(
        &self,
        chain: ChainType,
        asset_id: &str,
    ) -> SlippageParams {
        self.slippage_params
            .get(&chain)
            .and_then(|map| map.get(asset_id).copied())
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
    pub api_key: String,
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
            rpc_url: "https://polygon-bor-rpc.publicnode.com".to_string(),
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
