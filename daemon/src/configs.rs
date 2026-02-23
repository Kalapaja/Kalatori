mod consts;
mod types;
mod utils;

pub use types::*;
use utils::*;

// TODO: add logger config

pub fn secrets_config_with_prefix(
    config_dir_path: &str,
    prefix: &str,
) -> SecretsConfig {
    let config_path = format_config_path(config_dir_path, "secrets.json");
    let env_prefix = format_prefix(prefix, "SECRETS");
    let config = config_from_file_or_env::<SecretsConfig>(&config_path, &env_prefix);

    // Function is unsafe because of potential race conditions in multithreaded
    // environment. We call it at very start of the program before spawn any
    // futures which might cause this error therefore can consider it safe. If you
    // know some better way to handle it (except of forbid to provide seed
    // throgh env var) please let us know.
    unsafe {
        std::env::remove_var(format!("{env_prefix}_SEED"));
        std::env::remove_var(format!("{env_prefix}_API_SECRET_KEY"));
    }

    config
}

pub fn chains_config_with_prefix(
    config_dir_path: &str,
    prefix: &str,
) -> ChainsConfig {
    let config_path = format_config_path(config_dir_path, "chains.json");
    let env_prefix = format_prefix(prefix, "CHAINS");
    let mut config: ChainsConfig = config_from_file_or_env(&config_path, &env_prefix);
    config.set_default_chains_if_missing();
    config
}

pub fn payments_config_with_prefix(
    config_dir_path: &str,
    prefix: &str,
) -> PaymentsConfig {
    let config_path = format_config_path(config_dir_path, "payments.json");
    let env_prefix = format_prefix(prefix, "PAYMENTS");
    let mut config: PaymentsConfig = config_from_file_or_env(&config_path, &env_prefix);
    config.set_default_asset_id_if_missing();
    config
}

pub fn web_server_config_with_prefix(
    config_dir_path: &str,
    prefix: &str,
) -> WebServerConfig {
    let config_path = format_config_path(config_dir_path, "web_server.json");
    let env_prefix = format_prefix(prefix, "WEB_SERVER");
    config_from_file_or_env(&config_path, &env_prefix)
}

pub fn database_config_with_prefix(
    config_dir_path: &str,
    prefix: &str,
) -> DatabaseConfig {
    let config_path = format_config_path(config_dir_path, "database.json");
    let env_prefix = format_prefix(prefix, "DATABASE");
    config_from_file_or_env(&config_path, &env_prefix)
}

pub fn shop_config_with_prefix(
    config_dir_path: &str,
    prefix: &str,
) -> ShopConfig {
    let config_path = format_config_path(config_dir_path, "shop.json");
    let env_prefix = format_prefix(prefix, "SHOP");
    config_from_file_or_env(&config_path, &env_prefix)
}

pub fn logger_config_with_prefix(
    config_dir_path: &str,
    prefix: &str,
) -> LoggerConfig {
    let config_path = format_config_path(config_dir_path, "logger.json");
    let env_prefix = format_prefix(prefix, "LOGGER");
    config_from_file_or_env(&config_path, &env_prefix)
}

pub fn etherscan_client_config_with_prefix(
    config_dir_path: &str,
    prefix: &str,
) -> EtherscanClientConfig {
    let config_path = format_config_path(config_dir_path, "etherscan_client.json");
    let env_prefix = format_prefix(prefix, "ETHERSCAN_CLIENT");
    config_from_file_or_env(&config_path, &env_prefix)
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     use serial_test::serial;
//     use subxt_signer::ExposeSecret;

//     // TODO: those tests suppose that `make copy-configs` was executed. Need
// somehow     // ensure that it happend

//     #[test]
//     #[serial]
//     fn test_seed_config_with_prefix() {
//         // load from default config dir without any overrides
//         {
//             let config = seed_config_with_prefix("", "");
//             assert_eq!(
//                 config.seed.expose_secret(),
//                 "bottom drive obey lake curtain smoke basket hold race lonely
// fit walk"             );
//         }

//         // override seed with env var and ensure this env var was removed
// after config         // load
//         {
//             let value = "test seed";
//             unsafe {
//                 std::env::set_var("SEED_SEED", value);
//             }
//             let config = seed_config_with_prefix("", "");
//             assert_eq!(config.seed.expose_secret(), value);

//             let env_var = std::env::var("SEED_SEED");
//             assert!(matches!(
//                 env_var,
//                 Err(std::env::VarError::NotPresent)
//             ));
//         }

//         // same as previous + override env var prefix. Also set some
// different dir which         // shouldn't affect anything in this case
//         {
//             let value = "test seed 2";
//             unsafe {
//                 std::env::set_var("KALATORI_SUPER_PREFIX_SEED_SEED", value);
//             }
//             let config = seed_config_with_prefix(
//                 "somewhere-nowhere",
//                 "KALATORI_SUPER_PREFIX",
//             );
//             assert_eq!(config.seed.expose_secret(), value);

//             let env_var = std::env::var("KALATORI_SUPER_PREFIX_SEED_SEED");
//             assert!(matches!(
//                 env_var,
//                 Err(std::env::VarError::NotPresent)
//             ));
//         }
//     }

//     #[test]
//     #[serial]
//     fn test_payments_config_with_prefix() {
//         // load config from default config dir without any overrides
//         {
//             let config = payments_config_with_prefix("", "");
//             assert_eq!(
//                 config.account_lifetime_millis,
//                 default_account_lifetime_millis()
//             );

//             assert_eq!(
//                 config.recipient,
//                 // It's base58 representation of Alice address with prefix 0
// (Polkadot)                 "15oF4uVJwmo4TdGW7VfQxNLavjCXviqxT9S1MgbjMNHr6Sp5"
//             );
//         }

//         // override config dir and set `recipient` in env var
//         {
//             unsafe {
//                 // Recipient must be a valid address
//                 std::env::set_var(
//                     "PAYMENTS_RECIPIENT",
//                     "14E5nqKAp3oAJcmzgZhUD2RcptBeUBScxKHgJKU4HPNcKVf3",
//                 );
//                 std::env::set_var("PAYMENTS_DEFAULT_CHAIN",
//                     "PolkadotAssetHub");
//                 std::env::set_var("PAYMENTS_DEFAULT_ASSET_ID", "1337");
//             }

//             let config = payments_config_with_prefix("somewhere-nowhere",
// "");

//             assert_eq!(
//                 config.account_lifetime_millis,
//                 default_account_lifetime_millis()
//             );

//             assert_eq!(
//                 config.recipient,
//                 "14E5nqKAp3oAJcmzgZhUD2RcptBeUBScxKHgJKU4HPNcKVf3"
//             );
//             assert_eq!(config.default_chain, ChainType::PolkadotAssetHub);
//             assert_eq!(config.default_asset_id, "1337");
//         }

//         // override config env prefix
//         {
//             unsafe {
//                 std::env::set_var(
//                     "KALATORI_PAYMENTS_ACCOUNT_LIFETIME_MILLIS",
//                     "123",
//                 );
//             }

//             let config = payments_config_with_prefix("", "KALATORI");

//             assert_eq!(config.account_lifetime_millis, 123);

//             assert_eq!(
//                 config.recipient,
//                 // It's base58 representation of Alice address with prefix 0
// (Polkadot)                 "15oF4uVJwmo4TdGW7VfQxNLavjCXviqxT9S1MgbjMNHr6Sp5"
//             );
//         }
//     }

//     #[test]
//     #[serial]
//     fn test_chain_config_with_prefix() {
//         let mut expected_endpoints = vec!["ws://localhost:9000".to_string()];

//         let expected_assets = vec![
//             AssetConfig {
//                 name: "USDC".to_string(),
//                 id: 1337,
//             },
//             AssetConfig {
//                 name: "USDt".to_string(),
//                 id: 1984,
//             },
//         ];

//         // load config from default config dir without any overrides
//         {
//             let config = chain_config_with_prefix("", "");

//             assert_eq!(config.name, ChainType::PolkadotAssetHub);
//             assert_eq!(config.endpoints, expected_endpoints);
//             assert_eq!(config.assets, expected_assets);
//         }

//         // override endpoints with env vars
//         {
//             unsafe {
//                 std::env::set_var(
//                     "CHAIN_ENDPOINTS",
//                     "ws://localhost:9000,ws://localhost:9500",
//                 );
//             }

//             expected_endpoints.push("ws://localhost:9500".to_string());
//             let config = chain_config_with_prefix("", "");
//             assert_eq!(config.name, ChainType::PolkadotAssetHub);
//             assert_eq!(config.endpoints, expected_endpoints);
//             assert_eq!(config.assets, expected_assets);
//         }

//         // TODO: uncomment and update test after config structure refactoring
//         // override env var prefix
//         // {
//         //     unsafe {
//         //         std::env::set_var("KALATORI_CHAIN_NAME", "kusama");
//         //     }

//         //     let _unused = expected_endpoints.pop();
//         //     let config = chain_config_with_prefix("", "KALATORI");
//         //     assert_eq!(config.name, "kusama");
//         //     assert_eq!(config.endpoints, expected_endpoints);
//         //     assert_eq!(config.assets, expected_assets);
//         // }
//     }

//     #[test]
//     #[should_panic(
//         expected = "Failed to parse config file:
// somewhere-nowhere/chain.json. Error: missing configuration field \"name\""
//     )]
//     #[serial]
//     fn test_panic_on_unexisting_config() {
//         let _config = chain_config_with_prefix("somewhere-nowhere", "");
//     }

//     #[test]
//     #[serial]
//     fn test_web_server_config_with_prefix() {
//         // load config from default config dir without any overrides
//         {
//             let config = web_server_config_with_prefix("", "");
//             assert_eq!(
//                 config.host,
//                 IpAddr::V4(Ipv4Addr::UNSPECIFIED)
//             );
//             assert_eq!(config.port, 16726);
//         }

//         // override config dir to unexisting one but as long as all config
// fields are         // optional it should work
//         {
//             let config = web_server_config_with_prefix("somewhere-nowhere",
// "");             assert_eq!(
//                 config.host,
//                 IpAddr::V4(Ipv4Addr::UNSPECIFIED)
//             );
//             assert_eq!(config.port, 16726);
//         }

//         // override some parameter with env var
//         {
//             unsafe {
//                 std::env::set_var("WEB_SERVER_PORT", "12345");
//             }

//             let config = web_server_config_with_prefix("", "");
//             assert_eq!(
//                 config.host,
//                 IpAddr::V4(Ipv4Addr::UNSPECIFIED)
//             );
//             assert_eq!(config.port, 12345);
//         }

//         // override some parameter with env var with customized prefix
//         {
//             unsafe {
//                 std::env::set_var(
//                     "SUPER_KALATORI_WEB_SERVER_HOST",
//                     Ipv4Addr::LOCALHOST.to_string(),
//                 );
//             }

//             let config = web_server_config_with_prefix("", "SUPER_KALATORI");
//             assert_eq!(
//                 config.host,
//                 IpAddr::V4(Ipv4Addr::LOCALHOST)
//             );
//             assert_eq!(config.port, 16726);
//         }
//     }

//     #[test]
//     #[serial]
//     fn test_database_config_with_prefix() {
//         // load config from default config dir without any overrides
//         {
//             let config = database_config_with_prefix("", "");
//             assert_eq!(config.path, "kalatori.db".to_string());
//             assert!(!config.temporary);
//         }

//         // override configs dir to unexisting one but as long as all config
// fields are         // optional it should work
//         {
//             let config = database_config_with_prefix("somewhere-nowhere",
// "");             assert_eq!(config.path, "kalatori.db".to_string());
//             assert!(!config.temporary);
//         }

//         // override some parameter with env var
//         {
//             unsafe {
//                 std::env::set_var("DATABASE_TEMPORARY", "true");
//             }

//             let config = database_config_with_prefix("", "");
//             assert_eq!(config.path, "kalatori.db".to_string());
//             assert!(config.temporary);
//         }

//         // override some parameter with env var with customized prefix
//         {
//             unsafe {
//                 std::env::set_var(
//                     "MEGA_KALATORI_DATABASE_PATH",
//                     "mega_kalatori.db",
//                 );
//             }

//             let config = database_config_with_prefix("", "MEGA_KALATORI");
//             assert_eq!(config.path, "mega_kalatori.db");
//             assert!(!config.temporary);
//         }
//     }
// }
