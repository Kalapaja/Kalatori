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
use serde::de::Deserializer;
use serde::{
    Deserialize,
    Serialize,
};
use url::{
    Host,
    Url,
};

use crate::chain::utils::to_base58_string;
use crate::types::ChainType;

use super::consts::{
    DEFAULT_ALLOW_INSECURE_ENDPOINTS,
    DEFAULT_ASSET_HUB_ASSET_ID,
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

// TODO: add some docs for fields, their purpose might be not obvious
#[derive(Deserialize, Clone, Debug, Default)]
pub struct ChainConfig {
    /// RPC endpoints for the chain node. Can be left empty to use defaults.
    #[serde(default)]
    pub endpoints: Vec<String>,
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
    pub fn get_random_endpoint(&self) -> Option<String> {
        let mut rng = rand::rng();
        self.endpoints.choose(&mut rng).cloned()
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
                    .map(|s| s.to_string())
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
#[derive(Deserialize, Clone, Copy, Debug, Default)]
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

    #[expect(dead_code)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    pub reown_project_id: String,
}

#[derive(Clone)]
pub struct ShopConfig {
    pub invoices_webhook_url: Url,
    pub signature_max_age_secs: u64,
    pub allowed_base_redirect_url: Option<Host<String>>,
    pub allowed_base_image_urls: Option<Vec<Host<String>>>,
    pub meta: ShopMetaConfig,
}

impl ShopConfig {
    pub fn into_inner(
        self
    ) -> (
        String,
        u64,
        Host<String>,
        Vec<Host<String>>,
        ShopMetaConfig,
    ) {
        let webhook_domain = self
            .invoices_webhook_url
            .domain()
            .expect("shop config webhook URL must have host")
            .to_owned();

        let allowed_base_redirect_domain = self
            .allowed_base_redirect_url
            .unwrap_or_else(|| Host::Domain(webhook_domain.clone()));

        let allowed_base_image_domains = self
            .allowed_base_image_urls
            .unwrap_or_else(|| vec![Host::Domain(webhook_domain.clone())]);

        (
            self.invoices_webhook_url.to_string(),
            self.signature_max_age_secs,
            allowed_base_redirect_domain,
            allowed_base_image_domains,
            self.meta,
        )
    }
}

impl<'de> Deserialize<'de> for ShopConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawShopConfig {
            #[serde(alias = "invoice_webhook_url")]
            invoices_webhook_url: Url,
            #[serde(default = "default_signature_max_age_secs")]
            signature_max_age_secs: u64,
            #[serde(default)]
            allowed_redirect_url: Option<Url>,
            #[serde(default)]
            allowed_image_urls: Option<Vec<Url>>,
            #[serde(flatten)]
            meta: ShopMetaConfig,
        }

        let raw = RawShopConfig::deserialize(deserializer)?;

        // Validate url is of https protocol, port 443 (if has any) and has a domain
        // host.
        fn validate_https_443(url: &Url) -> Result<(), String> {
            if url.scheme() != "https" {
                return Err(format!(
                    "URL must use https scheme: {url}"
                ));
            }

            if let Some(port) = url.port()
                && port != 443
            {
                return Err(format!(
                    "URL port must be 443 when explicitly set: {url}"
                ));
            }

            if url.domain().is_none() {
                return Err(format!("URL has no host: {url}"));
            }

            Ok(())
        }

        // Validate invoices_webhook_url
        validate_https_443(&raw.invoices_webhook_url).map_err(serde::de::Error::custom)?;

        // Validate allowed_redirect_url
        if let Some(ref redirect_url) = raw.allowed_redirect_url {
            validate_https_443(redirect_url).map_err(serde::de::Error::custom)?;
        }

        // Validate allowed_image_urls
        if let Some(ref image_urls) = raw.allowed_image_urls {
            for image_url in image_urls {
                validate_https_443(image_url).map_err(serde::de::Error::custom)?;
            }
        }

        // Extract domain from URL.
        // We only care about domain for allowlists, so we ignore scheme, port and path.
        fn domain_from_url(url: Url) -> Result<Host<String>, String> {
            url.domain()
                .map(|host| Host::Domain(host.to_string()))
                .ok_or_else(|| format!("URL has no host in shop config: {url}"))
        }

        let allowed_base_redirect_url = raw
            .allowed_redirect_url
            .map(domain_from_url)
            .transpose()
            .map_err(serde::de::Error::custom)?;

        let allowed_base_image_urls = raw
            .allowed_image_urls
            .map(|urls| {
                urls.into_iter()
                    .map(domain_from_url)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
            .map_err(serde::de::Error::custom)?;

        Ok(Self {
            invoices_webhook_url: raw.invoices_webhook_url,
            signature_max_age_secs: raw.signature_max_age_secs,
            allowed_base_redirect_url,
            allowed_base_image_urls,
            meta: raw.meta,
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shop_config_defaults_allowlists_from_webhook_domain() {
        let config: ShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://payments.example.com/webhooks/invoices",
            "signature_max_age_secs": 60,
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let (_, _, redirect_domain, image_domains, _) = config.into_inner();

        assert_eq!(
            redirect_domain,
            Host::Domain("payments.example.com".to_string())
        );
        assert_eq!(
            image_domains,
            vec![Host::Domain("payments.example.com".to_string())]
        );
    }

    #[test]
    fn shop_config_normalizes_provided_allowlists() {
        let config: ShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://payments.example.com/webhooks/invoices",
            "signature_max_age_secs": 60,
            "allowed_redirect_url": "https://checkout.example.com/redirect",
            "allowed_image_urls": [
                "https://cdn.example.com/assets",
                "https://images.example.com/path"
            ],
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let (_, _, redirect_domain, image_domains, _) = config.into_inner();

        assert_eq!(
            redirect_domain,
            Host::Domain("checkout.example.com".to_string())
        );
        assert_eq!(
            image_domains,
            vec![
                Host::Domain("cdn.example.com".to_string()),
                Host::Domain("images.example.com".to_string())
            ]
        );
    }

    #[test]
    fn shop_config_rejects_non_https_allowlist_urls() {
        let result: Result<ShopConfig, _> = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://payments.example.com/webhooks/invoices",
            "allowed_redirect_url": "http://checkout.example.com/redirect",
            "shop_name": "Shop",
            "reown_project_id": "project"
        }));

        assert!(result.is_err());
    }

    #[test]
    fn shop_config_rejects_non_domain_allowlist_urls() {
        let result: Result<ShopConfig, _> = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://payments.example.com/webhooks/invoices",
            "allowed_redirect_url": "https://192.168.0.1/redirect",
            "shop_name": "Shop",
            "reown_project_id": "project"
        }));

        assert!(result.is_err());
    }
}
