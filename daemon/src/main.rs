mod api;
mod auth;
mod balance_checker;
mod chain;
mod chain_client;
mod clients;
mod configs;
mod dao;
mod error;
mod etherscan_client;
mod expiration_detector;
mod state;
mod swaps;
mod types;
mod utils;
mod webhook_sender;

use std::collections::{
    HashMap,
    HashSet,
};
use std::process::ExitCode;

use kalatori_client::types::ChainType;
use kalatori_client::utils::HmacConfig;
use secrecy::ExposeSecret;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;
use tracing::Level;

use chain::{
    InvoiceRegistry,
    TransactionsRecorder,
    TransfersExecutor,
    TransfersTracker,
};
use chain_client::{
    AssetHubClient,
    BlockChainClient,
    Keyring,
    PolygonClient,
};
use configs::{
    ChainsConfig,
    PaymentsConfig,
    auth_config_with_prefix,
    chains_config_with_prefix,
    database_config_with_prefix,
    etherscan_client_config_with_prefix,
    logger_config_with_prefix,
    payments_config_with_prefix,
    secrets_config_with_prefix,
    shop_config_with_prefix,
    swaps_config_with_prefix,
    web_server_config_with_prefix,
};
use dao::{
    DAO,
    DaoInterface,
};
use error::{
    Error,
    PrettyCause,
};
use etherscan_client::EtherscanClient;
use expiration_detector::ExpirationDetector;
use state::AppState;
use swaps::{
    SwapsExecutor,
    SwapsTracker,
};
use utils::logger;
use utils::shutdown::{
    self,
    ShutdownNotification,
    ShutdownOutcome,
};
use utils::task_tracker::TaskTracker;
use utils::RefundDestinationDetector;

use crate::balance_checker::BalanceChecker;
use crate::swaps::SwapsClients;

const DEFAULT_ENV_PREFIX: &str = "KALATORI";

fn main() -> ExitCode {
    let shutdown_notification = ShutdownNotification::new();

    // Sets the panic hook to print directly to the standard error because the
    // logger isn't initialized yet.
    shutdown::set_panic_hook(
        |panic| eprintln!("{panic}"),
        shutdown_notification.clone(),
    );

    let result = try_main(shutdown_notification.clone());

    if let Err(error) = result {
        // TODO: https://github.com/rust-lang/rust/issues/92698
        // An equilibristic to conditionally print an error message without storing it
        // as `String` on the heap.
        let print = |message| {
            if tracing::event_enabled!(Level::ERROR) {
                tracing::error!("{message}");
            } else {
                eprintln!("{message}");
            }
        };

        print(format_args!(
            "Badbye! The daemon's got an error during the initialization:{}",
            error.pretty_cause()
        ));

        ExitCode::FAILURE
    } else {
        match *shutdown_notification
            .outcome
            .read_blocking()
        {
            ShutdownOutcome::UserRequested => {
                tracing::info!("Goodbye!");

                ExitCode::SUCCESS
            },
            ShutdownOutcome::UnrecoverableError {
                panic,
            } => {
                tracing::error!(
                    "Badbye! The daemon's shut down with errors{}.",
                    if panic { " due to internal bugs" } else { "" }
                );

                ExitCode::FAILURE
            },
        }
    }
}

fn try_main(shutdown_notification: ShutdownNotification) -> Result<(), Error> {
    shutdown::set_panic_hook(
        |panic| eprintln!("{panic}"),
        shutdown_notification.clone(),
    );

    Runtime::new()
        .map_err(Error::Runtime)?
        .block_on(async_try_main(shutdown_notification))
}

async fn init_invoice_registry(dao: &impl DaoInterface) -> Result<InvoiceRegistry, Error> {
    let invoice_registry = InvoiceRegistry::new();

    let restore_invoices = dao
        .get_active_invoices_with_amounts()
        .await
        .map_err(|_| Error::Fatal)?;

    invoice_registry
        .add_invoices(restore_invoices)
        .await;

    Ok(invoice_registry)
}

fn validate_and_extend_configs(
    chains_config: &mut ChainsConfig,
    payments_config: &mut PaymentsConfig,
    restored_asset_ids: HashMap<ChainType, HashSet<String>>,
) -> Result<(), Error> {
    // Ensure that we have recipients for all chains from restored invoices and for
    // default chain
    let mut required_recipients: Vec<_> = restored_asset_ids
        .keys()
        .cloned()
        .collect();

    if !required_recipients.contains(&payments_config.default_chain) {
        required_recipients.push(payments_config.default_chain);
    }

    payments_config
        .validate_recipients(&required_recipients)
        .map_err(|_| Error::Fatal)?;

    // Extend chains config with default and restored asset IDs
    chains_config.add_default_asset_ids(&payments_config.default_asset_id);
    chains_config.add_restored_asset_ids(restored_asset_ids);

    Ok(())
}

#[expect(clippy::too_many_lines)]
async fn async_try_main(shutdown_notification: ShutdownNotification) -> Result<(), Error> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .unwrap();

    let env_prefix =
        std::env::var("KALATORI_APP_ENV_PREFIX").unwrap_or_else(|_| DEFAULT_ENV_PREFIX.to_string());

    let configs_path = std::env::var(format!("{env_prefix}_CONFIG_DIR_PATH")).unwrap_or_default();

    let logger_config = logger_config_with_prefix(&configs_path, &env_prefix);
    let loki_controller = logger::initialize(&logger_config)?;

    shutdown::set_panic_hook(
        |panic| tracing::error!("{panic}"),
        shutdown_notification.clone(),
    );

    tracing::info!(
        "Kalatori {} is starting...",
        env!("CARGO_PKG_VERSION")
    );

    let secrets_config = secrets_config_with_prefix(&configs_path, &env_prefix);
    let mut chains_config = chains_config_with_prefix(&configs_path, &env_prefix);
    let mut payments_config = payments_config_with_prefix(&configs_path, &env_prefix);
    let web_server_config = web_server_config_with_prefix(&configs_path, &env_prefix);
    let database_config = database_config_with_prefix(&configs_path, &env_prefix);
    let shop_config = shop_config_with_prefix(&configs_path, &env_prefix);
    let etherscan_client_config = etherscan_client_config_with_prefix(&configs_path, &env_prefix);
    let swaps_config = swaps_config_with_prefix(&configs_path, &env_prefix);
    let auth_config = auth_config_with_prefix(&configs_path, &env_prefix);

    let hmac_config = HmacConfig::new(
        secrets_config
            .api_secret_key
            .expose_secret()
            .as_bytes()
            .to_vec(),
        shop_config.signature_max_age_secs,
    );

    // Initialize DAO for SQLite database operations
    let dao = DAO::new(database_config.clone())
        .await
        .map_err(error::DaoError::Sqlx)?;

    let invoice_registry = init_invoice_registry(&dao).await?;

    validate_and_extend_configs(
        &mut chains_config,
        &mut payments_config,
        invoice_registry.used_asset_ids().await,
    )?;

    // Initialize Asset Hub client
    let asset_hub_chain_config = chains_config
        .chains
        .get(&ChainType::PolkadotAssetHub)
        .unwrap();

    let asset_hub_assets = chains_config
        .chains
        .get(&ChainType::PolkadotAssetHub)
        .unwrap()
        .assets
        .as_ref();

    let asset_hub_client = AssetHubClient::new(asset_hub_chain_config)
        .await
        .map_err(|_| {
            tracing::warn!("Failed to initialize Asset Hub client, continuing without it");
            Error::Fatal
        })?;

    asset_hub_client
        .init_asset_info(asset_hub_assets)
        .await
        .map_err(|_| {
            tracing::warn!("Failed to initialize Asset Hub asset info");
            Error::Fatal
        })?;

    // Initialize Polygon client
    let polygon_chain_config = chains_config
        .chains
        .get(&ChainType::Polygon)
        .unwrap();

    let polygon_assets = chains_config
        .chains
        .get(&ChainType::Polygon)
        .unwrap()
        .assets
        .as_ref();

    let polygon_client = PolygonClient::new(polygon_chain_config)
        .await
        .map_err(|e| {
            tracing::warn!(error = ?e, "Failed to initialize Polygon client, continuing without it");
            Error::Fatal
        })?;

    polygon_client
        .init_asset_info(polygon_assets)
        .await
        .map_err(|e| {
            tracing::warn!(error = ?e, "Failed to initialize Polygon asset info");
            Error::Fatal
        })?;

    // Collect asset names from both chains
    let mut asset_names_map = asset_hub_client
        .asset_info_store()
        .asset_names_map()
        .await;

    asset_names_map.extend(
        polygon_client
            .asset_info_store()
            .asset_names_map()
            .await,
    );

    let keyring = Keyring::new(secrets_config.seed);
    // Please don't keep keyring_client in this scope, it must be moved in order to
    // keep graceful shutdown working.
    let (keyring_handle, keyring_client) = keyring.ignite();

    let etherscan_client = EtherscanClient::new(etherscan_client_config);

    let transactions_recorder = TransactionsRecorder::new(
        dao.clone(),
        invoice_registry.clone(),
        payments_config.clone(),
    );

    let balance_checker = BalanceChecker::new(
        dao.clone(),
        invoice_registry.clone(),
        asset_hub_client.clone(),
        polygon_client.clone(),
        etherscan_client,
        transactions_recorder.clone(),
    );

    let expiration_detector = ExpirationDetector::new(
        dao.clone(),
        invoice_registry.clone(),
        payments_config.clone(),
        balance_checker.clone(),
    );

    let expiration_detector_handle =
        expiration_detector.ignite(shutdown_notification.token.clone());

    // Start Asset Hub transfers tracker
    let asset_hub_tracker = TransfersTracker::new(
        asset_hub_client.clone(),
        invoice_registry.clone(),
        transactions_recorder.clone(),
    );

    let asset_hub_tracker_handle = asset_hub_tracker.ignite(
        asset_hub_assets,
        shutdown_notification.token.clone(),
    );

    // Start Polygon transfers tracker
    let polygon_tracker = TransfersTracker::new(
        polygon_client.clone(),
        invoice_registry.clone(),
        transactions_recorder,
    );

    let polygon_tracker_handle = polygon_tracker.ignite(
        polygon_assets,
        shutdown_notification.token.clone(),
    );

    let swaps_clients = SwapsClients::new(swaps_config).await;

    let swaps_executor = SwapsExecutor::new(dao.clone(), swaps_clients.clone());

    let refund_destination_detector = RefundDestinationDetector::new(dao.clone());

    // Single executor handles both chains
    let transfer_executor = TransfersExecutor::new(
        refund_destination_detector,
        asset_hub_client,
        polygon_client,
        dao.clone(),
        keyring_client.clone(),
        swaps_executor.clone(),
    );

    let transfer_executor_handle = transfer_executor.ignite(shutdown_notification.token.clone());

    let webhook_sender = webhook_sender::WebhookSender::new(
        dao.clone(),
        shop_config.invoices_webhook_url.clone(),
        hmac_config.clone(),
    );

    let webhook_sender_handle = webhook_sender.ignite(shutdown_notification.token.clone());

    let swaps_tracker = SwapsTracker::new(
        dao.clone(),
        swaps_clients,
        balance_checker,
    );

    let swaps_tracker_handle = swaps_tracker.ignite(shutdown_notification.token.clone());

    let app_state = AppState::new(
        keyring_client,
        dao,
        invoice_registry,
        swaps_executor,
        asset_names_map,
        payments_config,
        shop_config,
        secrets_config.api_secret_key,
    );

    let api_handle = api::api_server(
        web_server_config,
        hmac_config,
        auth_config,
        app_state,
        shutdown_notification.token.clone(),
    )
    .await;

    let shutdown_completed = CancellationToken::new();
    let mut shutdown_listener = tokio::spawn(shutdown::listener(
        shutdown_notification.token.clone(),
        shutdown_completed.clone(),
    ));

    tracing::info!("The initialization has been completed.");
    let (task_tracker, error_rx) = TaskTracker::new();

    // Start the main loop and wait for it to gracefully end or the early
    // termination signal.
    let result = tokio::select! {
        biased;
        () = task_tracker.wait_and_shutdown(error_rx, shutdown_notification) => {
            shutdown_completed.cancel();

            let (
                shutdown_result,
                _keyring_result,
                _transfer_executor_result,
                _expiration_detector_result,
                _asset_hub_tracker_result,
                _polygon_tracker_result,
                _webhook_sender_result,
                _swaps_tracker_handle,
                _api_server_result,
            ) = tokio::join!(
                shutdown_listener,
                keyring_handle,
                transfer_executor_handle,
                expiration_detector_handle,
                asset_hub_tracker_handle,
                polygon_tracker_handle,
                webhook_sender_handle,
                swaps_tracker_handle,
                api_handle,
            );

            shutdown_result
        }
        shutdown_listener_result = &mut shutdown_listener => shutdown_listener_result
    }
    .expect("shutdown listener shouldn't panic");

    // Flush remaining logs to Loki after all components have stopped, so no
    // log records are lost.
    if let Some(controller) = loki_controller {
        controller.shutdown().await;
    }

    result
}
