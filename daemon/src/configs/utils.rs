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
    config_from_file_or_env_with_list_keys(filename, env_prefix, &["endpoints"])
}

pub fn config_from_file_or_env_with_list_keys<T: DeserializeOwned>(
    filename: &str,
    env_prefix: &str,
    list_keys: &[&str],
) -> T {
    let mut env_source = config::Environment::with_prefix(env_prefix)
        .try_parsing(true)
        .list_separator(",");

    for key in list_keys {
        env_source = env_source.with_list_parse_key(key);
    }

    let config = Config::builder()
        .add_source(config::File::with_name(filename).required(false))
        .add_source(env_source)
        .build()
        .unwrap_or_else(|err| panic!("Failed to read config file: {filename}. Error: {err}"));

    config
        .try_deserialize()
        .unwrap_or_else(|err| panic!("Failed to parse config file: {filename}. Error: {err}"))
}

/// Returns `true` if any environment variables with the given prefix exist.
fn has_env_vars_with_prefix(prefix: &str) -> bool {
    let prefix_underscore = format!("{prefix}_");
    std::env::vars().any(|(key, _)| key.starts_with(&prefix_underscore))
}

/// Like `config_from_file_or_env_with_list_keys`, but returns `None` when no
/// config source is available (file missing and no env vars with the prefix).
pub fn try_config_from_file_or_env_with_list_keys<T: DeserializeOwned>(
    filename: &str,
    env_prefix: &str,
    list_keys: &[&str],
) -> Option<T> {
    let file_exists = std::path::Path::new(filename).exists();
    let has_env = has_env_vars_with_prefix(env_prefix);

    if !file_exists && !has_env {
        return None;
    }

    Some(config_from_file_or_env_with_list_keys(
        filename, env_prefix, list_keys,
    ))
}
