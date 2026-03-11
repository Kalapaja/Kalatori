use config::Config;
use serde::de::DeserializeOwned;

use super::consts::DEFAULT_CONFIG_DIR_PATH;

pub fn format_prefix(
    prefix: &str,
    config_prefix: &str,
) -> String {
    if prefix.is_empty() {
        config_prefix.to_string()
    } else {
        format!("{prefix}_{config_prefix}")
    }
}

pub fn format_config_path(
    config_dir_path: &str,
    config_name: &str,
) -> String {
    if config_dir_path.is_empty() {
        format!("{DEFAULT_CONFIG_DIR_PATH}/{config_name}")
    } else if config_dir_path.ends_with('/') {
        format!("{config_dir_path}{config_name}")
    } else {
        format!("{config_dir_path}/{config_name}")
    }
}

pub fn config_from_file_or_env<T: DeserializeOwned>(
    filename: &str,
    env_prefix: &str,
) -> T {
    let config = Config::builder()
        .add_source(config::File::with_name(filename).required(false))
        .add_source(
            config::Environment::with_prefix(env_prefix)
                .try_parsing(true)
                // allow set ChainConfig.endpoints over env vars
                .with_list_parse_key("endpoints")
                .with_list_parse_key("allowed_base_image_urls")
                .list_separator(","),
        )
        .build()
        .unwrap_or_else(|err| panic!("Failed to read config file: {filename}. Error: {err}"));

    config
        .try_deserialize()
        .unwrap_or_else(|err| panic!("Failed to parse config file: {filename}. Error: {err}"))
}
