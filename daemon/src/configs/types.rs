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
use url::Url;

use crate::chain::utils::to_base58_string;
use crate::error::inputs_validation::ConfigInputValidationError;
use crate::types::ChainType;
use crate::utils::url_validation;

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

/// Raw shop configuration parsed from the config file.
#[derive(Clone, Deserialize)]
pub(super) struct RawShopConfig {
    invoices_webhook_url: Url,
    #[serde(default = "default_signature_max_age_secs")]
    signature_max_age_secs: u64,
    /// Allowlisted base URL for redirect URLs.
    /// `None` defaults to the invoices webhook URL's domain.
    allowed_base_redirect_url: Option<Url>,
    /// Allowlisted base URLs for image URLs.
    /// `None` defaults to the invoices webhook URL's domain.
    allowed_base_image_urls: Option<Vec<Url>>,
    /// When `true`, no validation checks for URLs are performed.
    #[serde(default)]
    allow_insecure_urls: bool,
    #[serde(flatten)]
    meta: ShopMetaConfig,
}

impl RawShopConfig {
    /// Resolves optional allowlist fields, defaulting to the webhook domain.
    /// Called once on program startup after deserialization succeeds.
    pub(super) async fn validated(self) -> Result<ValidatedShopConfig, ConfigInputValidationError> {
        let Self {
            invoices_webhook_url,
            signature_max_age_secs,
            allowed_base_redirect_url,
            allowed_base_image_urls,
            allow_insecure_urls,
            meta,
        } = self;
        let api_validator_config = if !allow_insecure_urls {
            url_validation::validate(invoices_webhook_url.as_str())
                .await
                .map_err(ConfigInputValidationError::InvalidInvoiceWebhookUrl)?;

            let allowed_base_redirect_url = if let Some(redirect_url) = allowed_base_redirect_url {
                url_validation::validate_base_url(&redirect_url)
                    .map_err(ConfigInputValidationError::InvalidAllowedBaseRedirectUrl)?;

                redirect_url
            } else {
                Self::base_url_from_webhook(&invoices_webhook_url)?
            };

            let allowed_base_image_urls = if let Some(image_urls) = allowed_base_image_urls {
                if image_urls.is_empty() {
                    return Err(ConfigInputValidationError::AllowedBaseImageUrlsEmpty);
                }

                for url in &image_urls {
                    url_validation::validate_base_url(url)
                        .map_err(ConfigInputValidationError::InvalidAllowedBaseImageUrl)?;
                }

                image_urls
            } else {
                vec![Self::base_url_from_webhook(&invoices_webhook_url)?]
            };

            ApiValidatorConfig {
                allowed_base_redirect_url,
                allowed_base_image_urls,
                allow_insecure_urls: false,
            }
        } else {
            ApiValidatorConfig::insecure_config()
        };

        Ok(ValidatedShopConfig {
            api_validator_config,
            invoices_webhook_url,
            signature_max_age_secs,
            meta,
        })
    }

    fn base_url_from_webhook(invoice_webhook_url: &Url) -> Result<Url, ConfigInputValidationError> {
        invoice_webhook_url
            .domain()
            .map(|host| Url::parse(&format!("https://{host}/")).expect("valid URL"))
            .ok_or(ConfigInputValidationError::InvoiceWebhookUrlHasNoDomain)
    }
}

/// [`ShopConfig`], but with validated fields.
#[derive(Debug, Clone)]
pub struct ValidatedShopConfig {
    pub api_validator_config: ApiValidatorConfig,
    pub invoices_webhook_url: Url,
    pub signature_max_age_secs: u64,
    pub meta: ShopMetaConfig,
}

/// Minimal configuration consumed by api params validator.
#[derive(Debug, Clone)]
pub struct ApiValidatorConfig {
    pub allowed_base_redirect_url: Url,
    // TODO: bring type level guarantees that isn't empty?
    pub allowed_base_image_urls: Vec<Url>,
    pub allow_insecure_urls: bool,
}

impl ApiValidatorConfig {
    pub fn insecure_config() -> Self {
        // Set any allowed* domains, as they will be ignore in validation.
        Self {
            allowed_base_redirect_url: Url::parse("https://example.com/").unwrap(),
            allowed_base_image_urls: vec![Url::parse("https://example.com/").unwrap()],
            allow_insecure_urls: true,
        }
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

fn default_etherscan_limit_per_second() -> NonZeroU32 {
    DEFAULT_ETHERSCAN_LIMIT_PER_SECOND
}

#[derive(Deserialize, Clone, Debug)]
pub struct EtherscanClientConfig {
    #[serde(default = "default_etherscan_limit_per_second")]
    pub requests_per_second: NonZeroU32,
    pub api_key: String,
}

#[cfg(test)]
mod tests {
    use super::super::consts::DEFAULT_SIGNATURE_MAX_AGE_SECS;
    use super::*;
    use crate::utils::url_validation::UrlValidationError;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[tokio::test]
    async fn shop_config_defaults_allowlists_from_webhook_domain() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "signature_max_age_secs": 60,
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let validated = config.validated().await.unwrap();

        assert_eq!(
            validated
                .api_validator_config
                .allowed_base_redirect_url,
            url("https://example.com/")
        );
        assert_eq!(
            validated
                .api_validator_config
                .allowed_base_image_urls,
            vec![url("https://example.com/")]
        );
        assert!(
            !validated
                .api_validator_config
                .allow_insecure_urls
        );
    }

    #[tokio::test]
    async fn shop_config_normalizes_provided_allowlists() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "signature_max_age_secs": 60,
            "allowed_base_redirect_url": "https://checkout.example.com/redirect/",
            "allowed_base_image_urls": [
                "https://cdn.example.com/assets/",
                "https://images.example.com/path/"
            ],
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let validated = config.validated().await.unwrap();

        assert_eq!(
            validated
                .api_validator_config
                .allowed_base_redirect_url,
            url("https://checkout.example.com/redirect/")
        );
        assert_eq!(
            validated
                .api_validator_config
                .allowed_base_image_urls,
            vec![
                url("https://cdn.example.com/assets/"),
                url("https://images.example.com/path/")
            ]
        );
    }

    #[test]
    fn shop_config_allow_insecure_urls_defaults_to_false() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://payments.example.com/webhooks/invoices",
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        assert!(!config.allow_insecure_urls);
    }

    #[tokio::test]
    async fn shop_config_rejects_non_https_allowlist_urls() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "allowed_base_redirect_url": "http://checkout.example.com/redirect/",
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("invalid config");

        let res = config.validated().await;
        assert!(matches!(
            res,
            Err(
                ConfigInputValidationError::InvalidAllowedBaseRedirectUrl(
                    UrlValidationError::InvalidScheme(_)
                )
            )
        ));
    }

    #[tokio::test]
    async fn shop_config_rejects_non_domain_allowlist_urls() {
        let result: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "allowed_base_redirect_url": "https://192.168.0.1/redirect/",
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("invalid config");

        let res = result.validated().await;
        assert!(matches!(
            res,
            Err(
                ConfigInputValidationError::InvalidAllowedBaseRedirectUrl(
                    UrlValidationError::UrlHostIsNotDomain
                )
            )
        ));
    }

    #[tokio::test]
    async fn shop_config_rejects_non_https_webhook_url() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "http://example.com/webhooks/invoices",
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let res = config.validated().await;
        assert!(matches!(
            res,
            Err(
                ConfigInputValidationError::InvalidInvoiceWebhookUrl(
                    UrlValidationError::InvalidScheme(_)
                )
            )
        ));
    }

    #[tokio::test]
    async fn shop_config_rejects_ip_webhook_url_when_no_explicit_allowlists() {
        // 93.184.216.34 is a globally-routable IP so validate() passes, but
        // domain() returns None, triggering InvoiceWebhookUrlHasNoDomain when
        // no explicit allowed_base_* entries are provided to fall back on.
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://93.184.216.34/webhooks/invoices",
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let res = config.validated().await;
        assert!(matches!(
            res,
            Err(ConfigInputValidationError::InvoiceWebhookUrlHasNoDomain)
        ));
    }

    #[tokio::test]
    async fn shop_config_rejects_non_https_image_allowlist_url() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "allowed_base_image_urls": ["http://cdn.example.com/assets/"],
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let res = config.validated().await;
        assert!(matches!(
            res,
            Err(
                ConfigInputValidationError::InvalidAllowedBaseImageUrl(
                    UrlValidationError::InvalidScheme(_)
                )
            )
        ));
    }

    #[tokio::test]
    async fn shop_config_rejects_ip_host_in_image_allowlist_url() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "allowed_base_image_urls": ["https://192.168.0.1/assets/"],
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let res = config.validated().await;
        assert!(matches!(
            res,
            Err(
                ConfigInputValidationError::InvalidAllowedBaseImageUrl(
                    UrlValidationError::UrlHostIsNotDomain
                )
            )
        ));
    }

    #[tokio::test]
    async fn shop_config_allow_insecure_urls_skips_all_validation() {
        // A URL that would normally fail validation (non-https, localhost) passes
        // when allow_insecure_urls is set.
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "http://localhost/webhooks",
            "allow_insecure_urls": true,
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let validated = config.validated().await.unwrap();
        assert!(
            validated
                .api_validator_config
                .allow_insecure_urls
        );
    }

    #[test]
    fn shop_config_signature_max_age_secs_default() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        // DEFAULT_SIGNATURE_MAX_AGE_SECS is 5 minutes (300 s)
        assert_eq!(
            config.signature_max_age_secs,
            DEFAULT_SIGNATURE_MAX_AGE_SECS
        );
    }

    #[tokio::test]
    async fn shop_config_validated_preserves_webhook_url_and_meta() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "signature_max_age_secs": 120,
            "shop_name": "My Shop",
            "logo_url": "https://example.com/logo.png",
            "reown_project_id": "proj-abc"
        }))
        .expect("shop config should deserialize");

        let validated = config.validated().await.unwrap();

        assert_eq!(
            validated.invoices_webhook_url.as_str(),
            "https://example.com/webhooks/invoices",
        );
        assert_eq!(validated.signature_max_age_secs, 120);
        assert_eq!(validated.meta.shop_name, "My Shop");
        assert_eq!(
            validated.meta.logo_url.as_deref(),
            Some("https://example.com/logo.png"),
        );
        assert_eq!(
            validated.meta.reown_project_id,
            "proj-abc"
        );
    }

    #[tokio::test]
    async fn shop_config_rejects_empty_allowed_base_image_urls() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "allowed_base_image_urls": [],
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let res = config.validated().await;
        assert!(matches!(
            res,
            Err(ConfigInputValidationError::AllowedBaseImageUrlsEmpty)
        ));
    }

    #[tokio::test]
    async fn base_url_from_host_is_a_valid_base_url() {
        let config: RawShopConfig = serde_json::from_value(serde_json::json!({
            "invoices_webhook_url": "https://example.com/webhooks/invoices",
            "shop_name": "Shop",
            "reown_project_id": "project"
        }))
        .expect("shop config should deserialize");

        let validated = config.validated().await.unwrap();

        assert!(
            url_validation::validate_base_url(
                &validated
                    .api_validator_config
                    .allowed_base_redirect_url
            )
            .is_ok()
        );

        for url in &validated
            .api_validator_config
            .allowed_base_image_urls
        {
            assert!(url_validation::validate_base_url(url).is_ok());
        }
    }
}
