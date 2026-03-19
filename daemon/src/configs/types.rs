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
    pub ankr_api_token: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct ShopConfig {
    #[serde(default)]
    pub invoices_webhook_url: Option<String>,
    #[serde(default = "default_signature_max_age_secs")]
    pub signature_max_age_secs: u64,
    #[serde(flatten)]
    pub meta: ShopMetaConfig,
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

#[derive(Deserialize, Default, Clone, Debug)]
pub struct SwapsConfig {
    #[serde(default)]
    pub bungee: Option<BungeeApiConfig>,
    #[serde(default)]
    pub fees: Option<IntegratorFees>,
}
