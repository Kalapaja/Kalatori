use tracing_loki::BackgroundTaskController;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::configs::LoggerConfig;

/// Initialize the tracing subscriber with a JSON fmt layer and an optional Loki
/// layer. Returns a [`BackgroundTaskController`] if Loki is enabled, which must
/// be shut down after all other components to avoid losing log records.
pub fn initialize(config: &LoggerConfig) -> Option<BackgroundTaskController> {
    let filter =
        EnvFilter::try_new(&config.directives).expect("Failed to parse logger filter directives");

    let fmt_layer = tracing_subscriber::fmt::layer().json();

    let (loki_layer, loki_controller) = if let Some(url) = &config.loki_url {
        let parsed_url: tracing_loki::url::Url = url
            .parse()
            .expect("Failed to parse Loki URL");

        let (layer, controller, task) = tracing_loki::builder()
            .build_controller_url(parsed_url)
            .expect("Failed to initialize Loki layer");

        tokio::spawn(task);

        (Some(layer), Some(controller))
    } else {
        (None, None)
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(loki_layer)
        .init();

    loki_controller
}
