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

use futures::future::OptionFuture;
use kalatori_client::strum::IntoEnumIterator;
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
    ShopConfig,
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
use utils::shutdown::{
    self,
    ShutdownNotification,
    ShutdownOutcome,
};
use utils::task_tracker::TaskTracker;
use utils::{
    RefundDestinationDetector,
    logger,
};

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
    shop_config: &ShopConfig,
    restored_asset_ids: HashMap<ChainType, HashSet<String>>,
) -> Result<(), Error> {
    shop_config
        .validate_invoices_webhook_url()
        .map_err(|error| {
            tracing::error!(%error, "Invalid shop config");
            Error::Fatal
        })?;

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

    // Canonicalize EVM asset addresses before any comparisons/merges: chain
    // clients report checksummed addresses, and a differently-cased config
    // entry would never match them
    payments_config
        .canonicalize_evm_asset_ids()
        .map_err(|error| {
            tracing::error!(%error, "Invalid EVM asset address in payments config");
            Error::Fatal
        })?;
    chains_config
        .canonicalize_evm_asset_ids()
        .map_err(|error| {
            tracing::error!(%error, "Invalid EVM asset address in chains config");
            Error::Fatal
        })?;

    // Extend chains config with default and restored asset IDs
    chains_config.add_default_asset_ids(&payments_config.default_asset_id);
    chains_config.add_restored_asset_ids(restored_asset_ids);

    Ok(())
}

/// Bring one chain client up, or report the chain as unavailable.
///
/// Failing to reach a chain is degraded state, not a fatal one: the daemon
/// keeps serving every chain that did come up. Deciding whether what came up
/// is enough to run on is [`report_chain_availability`]'s job, not this one's.
async fn init_chain_client<T, C>(
    chain_config: &configs::ChainConfig,
    assets: &[String],
) -> Option<C>
where
    T: chain_client::ChainConfig,
    C: BlockChainClient<T>,
{
    let chain = T::CHAIN_TYPE;

    match C::new(chain_config).await {
        Ok(client) => finish_chain_client(client, assets).await,
        Err(error) => {
            tracing::warn!(
                error.category = utils::logging::category::CHAIN_CLIENT,
                error.operation = utils::logging::operation::CONNECT_CLIENT,
                error.source = ?error,
                %chain,
                "Failed to connect the chain client, continuing without this chain"
            );

            None
        },
    }
}

/// The half of chain-client startup that runs once a connection exists.
///
/// A client whose asset metadata cannot be read is as unusable as one that
/// never connected — transfers on it could not be priced or matched — so it
/// degrades the chain the same way.
async fn finish_chain_client<T, C>(
    client: C,
    assets: &[String],
) -> Option<C>
where
    T: chain_client::ChainConfig,
    C: BlockChainClient<T>,
{
    let chain = T::CHAIN_TYPE;

    if let Err(error) = client.init_asset_info(assets).await {
        tracing::warn!(
            error.category = utils::logging::category::CHAIN_CLIENT,
            error.operation = utils::logging::operation::FETCH_ASSET_INFO,
            error.source = ?error,
            %chain,
            "Failed to initialize chain asset info, continuing without this chain"
        );

        return None
    }

    tracing::info!(%chain, "Chain client initialized");

    Some(client)
}

/// Report which chains came up, and decide whether that is enough to run on.
///
/// Two cases are fatal, and only two:
///
/// - **No chain came up.** There is nothing to serve; a gateway that cannot
///   reach any chain is not usefully running.
/// - **The default chain did not come up.** Every invoice is created on
///   `default_chain` and every swap settles there, so the daemon could accept
///   no payment at all — it would answer requests while being unable to do the
///   one thing it exists for.
///
/// Anything else is degraded: the chains that came up are served normally, and
/// the ones that did not are reported by name. There is no reconnection path —
/// a chain that failed here stays unavailable until the daemon is restarted —
/// so the warnings say that outright rather than implying a recovery that does
/// not exist.
fn report_chain_availability(
    available_chains: &HashSet<ChainType>,
    default_chain: ChainType,
    chains_with_active_invoices: &HashSet<ChainType>,
) -> Result<(), Error> {
    if available_chains.is_empty() {
        tracing::error!("No chain client could be initialized, refusing to start");

        return Err(Error::Fatal)
    }

    if !available_chains.contains(&default_chain) {
        tracing::error!(
            chain = %default_chain,
            "The configured default chain is unavailable, refusing to start: \
             every invoice would be created on a chain this daemon cannot reach"
        );

        return Err(Error::Fatal)
    }

    for chain in ChainType::iter() {
        if available_chains.contains(&chain) {
            continue
        }

        // Deliberately narrow. Provider-routed swaps are *not* covered: they
        // settle through the swap provider's API rather than through our node,
        // so `TransfersExecutor::send_transfer` still routes them and they can
        // still execute. Saying "no payouts" here would be the same kind of
        // false claim this whole change exists to remove.
        tracing::warn!(
            %chain,
            "Running without this chain: no transfers will be tracked, and no direct payouts \
             or refunds will be submitted on it, until the daemon is restarted. \
             Swaps routed through a swap provider are unaffected, but their settlement \
             cannot be confirmed while this chain is unavailable"
        );

        if chains_with_active_invoices.contains(&chain) {
            tracing::warn!(
                %chain,
                "Active invoices restored from the database are on this unavailable chain; \
                 payments to them will not be detected until the daemon is restarted"
            );
        }
    }

    Ok(())
}

#[expect(clippy::too_many_lines)]
async fn async_try_main(shutdown_notification: ShutdownNotification) -> Result<(), Error> {
    // This must stay the first statement in the function, and in particular
    // must precede `logger::initialize` below. rustls allows exactly one
    // process-wide default provider: whoever installs first wins, and every
    // later attempt fails.
    //
    // Every money path -- Asset Hub, Polygon, Pimlico, Etherscan, merchant
    // webhooks -- runs on this provider, so it has to be the one we chose.
    // Log shipping is the only other TLS user in the process, and it does not
    // get to decide: tracing-loki resolves reqwest 0.12, whose rustls feature
    // set compiles in the ring backend.
    //
    // What the ordering buys is not a won race. reqwest 0.12 never installs a
    // default of its own: it reads one via `CryptoProvider::get_default` and,
    // finding the slot empty, falls back to a locally built ring provider
    // (`async_impl/client.rs`). So initialising the logger first would leave
    // this `unwrap` intact and fail silently instead -- Loki on ring, every
    // payment on aws-lc, two crypto backends live in one process. Installing
    // here is what collapses that back to one.
    //
    // Nothing else in the tree calls `install_default`, which is what makes
    // the `#[expect]` reason below true.
    #[expect(
        clippy::unwrap_used,
        reason = "startup: this is the first install of the process-wide crypto provider, so it cannot already be set"
    )]
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
    let dao = DAO::new(database_config.clone()).await?;

    // Recovery must finish before any executor is ignited: once workers can claim
    // rows, a sweep could reset work claimed by this process.
    let recovered_payouts = dao
        .recover_in_progress_payouts()
        .await
        .map_err(|error| {
            tracing::error!(
                error.source = ?error,
                "Could not recover in-progress payouts, refusing to start"
            );
            Error::Fatal
        })?;
    if recovered_payouts > 0 {
        tracing::info!(
            count = recovered_payouts,
            "Recovered in-progress payouts"
        );
    }

    let recovered_refunds = dao
        .recover_in_progress_refunds()
        .await
        .map_err(|error| {
            tracing::error!(
                error.source = ?error,
                "Could not recover in-progress refunds, refusing to start"
            );
            Error::Fatal
        })?;
    if recovered_refunds > 0 {
        tracing::info!(
            count = recovered_refunds,
            "Recovered in-progress refunds"
        );
    }

    let invoice_registry = init_invoice_registry(&dao).await?;

    let restored_asset_ids = invoice_registry.used_asset_ids().await;
    // Kept for the availability report below: an operator whose restored
    // invoices sit on a chain that did not come up needs to be told so by
    // name, not left to infer it.
    let restored_chains: HashSet<ChainType> = restored_asset_ids
        .keys()
        .copied()
        .collect();

    validate_and_extend_configs(
        &mut chains_config,
        &mut payments_config,
        &shop_config,
        restored_asset_ids,
    )?;

    // Initialize Asset Hub client
    #[expect(
        clippy::unwrap_used,
        reason = "startup: `set_default_chains_if_missing` establishes an entry for every `ChainType` before this point, and config validation has already succeeded"
    )]
    let asset_hub_chain_config = chains_config
        .chains
        .get(&ChainType::PolkadotAssetHub)
        .unwrap();

    #[expect(
        clippy::unwrap_used,
        reason = "startup: `set_default_chains_if_missing` establishes an entry for every `ChainType` before this point, and config validation has already succeeded"
    )]
    let asset_hub_assets = chains_config
        .chains
        .get(&ChainType::PolkadotAssetHub)
        .unwrap()
        .assets
        .as_ref();

    let asset_hub_client =
        init_chain_client::<_, AssetHubClient>(asset_hub_chain_config, asset_hub_assets).await;

    // Initialize Polygon client
    #[expect(
        clippy::unwrap_used,
        reason = "startup: `set_default_chains_if_missing` establishes an entry for every `ChainType` before this point, and config validation has already succeeded"
    )]
    let polygon_chain_config = chains_config
        .chains
        .get(&ChainType::Polygon)
        .unwrap();

    #[expect(
        clippy::unwrap_used,
        reason = "startup: `set_default_chains_if_missing` establishes an entry for every `ChainType` before this point, and config validation has already succeeded"
    )]
    let polygon_assets = chains_config
        .chains
        .get(&ChainType::Polygon)
        .unwrap()
        .assets
        .as_ref();

    let polygon_client =
        init_chain_client::<_, PolygonClient>(polygon_chain_config, polygon_assets).await;

    let mut available_chains = HashSet::new();
    if asset_hub_client.is_some() {
        available_chains.insert(ChainType::PolkadotAssetHub);
    }
    if polygon_client.is_some() {
        available_chains.insert(ChainType::Polygon);
    }

    report_chain_availability(
        &available_chains,
        payments_config.default_chain,
        &restored_chains,
    )?;

    // Collect asset names from the chains that came up. An unavailable chain
    // contributes nothing, which is why the default chain has to be available:
    // invoice creation reads its asset name and decimals from these maps.
    let mut asset_names_map = HashMap::new();
    let mut asset_decimals_map = HashMap::new();

    if let Some(client) = asset_hub_client.as_ref() {
        let store = client.asset_info_store();

        asset_names_map.extend(store.asset_names_map().await);
        asset_decimals_map.insert(
            ChainType::PolkadotAssetHub,
            store.asset_decimals_map().await,
        );
    }

    if let Some(client) = polygon_client.as_ref() {
        let store = client.asset_info_store();

        asset_names_map.extend(store.asset_names_map().await);
        asset_decimals_map.insert(
            ChainType::Polygon,
            store.asset_decimals_map().await,
        );
    }

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

    // Start a transfers tracker per available chain. A chain that did not come
    // up gets no tracker at all rather than one that can never subscribe.
    let asset_hub_tracker_handle = asset_hub_client.clone().map(|client| {
        TransfersTracker::new(
            client,
            invoice_registry.clone(),
            transactions_recorder.clone(),
        )
        .ignite(
            asset_hub_assets,
            shutdown_notification.token.clone(),
        )
    });

    let polygon_tracker_handle = polygon_client.clone().map(|client| {
        TransfersTracker::new(
            client,
            invoice_registry.clone(),
            transactions_recorder,
        )
        .ignite(
            polygon_assets,
            shutdown_notification.token.clone(),
        )
    });

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
        asset_decimals_map,
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
                OptionFuture::from(asset_hub_tracker_handle),
                OptionFuture::from(polygon_tracker_handle),
                webhook_sender_handle,
                swaps_tracker_handle,
                api_handle,
            );

            shutdown_result
        }
        shutdown_listener_result = &mut shutdown_listener => shutdown_listener_result
    }
    .unwrap_or_else(|error| {
        // Reached only if the shutdown listener task itself panicked or was
        // cancelled. Re-panicking here would abort before the logs are
        // flushed below, so report it and let the exit code carry the failure.
        tracing::error!(
            error.source = ?error,
            "The shutdown listener did not complete cleanly"
        );

        Err(Error::Fatal)
    });

    // Flush remaining logs to Loki after all components have stopped, so no
    // log records are lost.
    if let Some(controller) = loki_controller {
        controller.shutdown().await;
    }

    result
}

#[cfg(test)]
mod tests {
    use chain_client::{
        ClientError,
        MockBlockChainClient,
        PolygonChainConfig,
    };

    use super::*;

    fn chains(chains: &[ChainType]) -> HashSet<ChainType> {
        chains.iter().copied().collect()
    }

    #[test]
    fn no_chain_at_all_is_fatal() {
        assert!(matches!(
            report_chain_availability(
                &HashSet::new(),
                ChainType::Polygon,
                &HashSet::new()
            ),
            Err(Error::Fatal)
        ));
    }

    #[test]
    fn an_unavailable_default_chain_is_fatal() {
        assert!(matches!(
            report_chain_availability(
                &chains(&[ChainType::PolkadotAssetHub]),
                ChainType::Polygon,
                &HashSet::new()
            ),
            Err(Error::Fatal)
        ));
    }

    /// The defect this replaced: the daemon logged "continuing without it" and
    /// then returned `Error::Fatal`, so a Polygon-only merchant could not boot
    /// while Polkadot RPC was down. One healthy chain — the default one — is
    /// enough to run on.
    #[test]
    #[tracing_test::traced_test]
    fn a_non_default_chain_going_missing_is_degraded_not_fatal() {
        assert!(
            report_chain_availability(
                &chains(&[ChainType::Polygon]),
                ChainType::Polygon,
                &HashSet::new()
            )
            .is_ok()
        );

        // Naming the missing chain is the point of the line, and so is not
        // promising a reconnection the daemon does not implement.
        assert!(logs_contain(
            "Running without this chain"
        ));
        assert!(logs_contain("PolkadotAssetHub"));
        assert!(logs_contain(
            "until the daemon is restarted"
        ));
    }

    #[test]
    #[tracing_test::traced_test]
    fn restored_invoices_on_an_unavailable_chain_are_called_out_by_chain() {
        assert!(
            report_chain_availability(
                &chains(&[ChainType::Polygon]),
                ChainType::Polygon,
                &chains(&[ChainType::PolkadotAssetHub])
            )
            .is_ok()
        );

        assert!(logs_contain(
            "Active invoices restored from the database are on this unavailable chain"
        ));
    }

    /// A chain reachable enough to connect but not to answer for its assets is
    /// no more usable than one that never connected, so it degrades the same
    /// way instead of aborting startup.
    #[tokio::test]
    async fn a_client_whose_asset_info_fails_degrades_the_chain() {
        let mut client = MockBlockChainClient::<PolygonChainConfig>::default();

        client
            .expect_init_asset_info()
            .once()
            .returning(|_| Err(ClientError::MetadataFetchFailed));

        assert!(
            finish_chain_client::<PolygonChainConfig, _>(
                client,
                &["0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359".to_string()]
            )
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn a_client_that_comes_up_is_kept() {
        let mut client = MockBlockChainClient::<PolygonChainConfig>::default();

        client
            .expect_init_asset_info()
            .once()
            .returning(|_| Ok(()));

        assert!(
            finish_chain_client::<PolygonChainConfig, _>(client, &[])
                .await
                .is_some()
        );
    }
}
