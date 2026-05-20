//! The shutdown module.
//!
//! Provides a single entry point for managing graceful shutdown, including:
//! - OS signal listening (SIGINT)
//! - Panic hook installation
//! - [`CancellationToken`] distribution to components

use std::future::Future;
use std::panic::{
    self,
    PanicHookInfo,
};

use tokio_util::sync::CancellationToken;

async fn ctrl_c_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install CTRL+C signal handler");
}

fn format_panic_message(info: &PanicHookInfo<'_>) -> String {
    let location = info.location().map_or_else(
        || "unknown location".to_string(),
        |location| location.to_string(),
    );

    let message = info
        .payload_as_str()
        .unwrap_or("no message");

    format!("Panicked at {location}: {message}")
}

pub struct Shutdown {
    token: CancellationToken,
}

impl Shutdown {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// Installs a panic hook that cancels the shutdown token on any panic.
    pub fn install_panic_hook(&self) {
        let token = self.token.clone();

        panic::set_hook(Box::new(move |panic_info| {
            tracing::error!("{}", format_panic_message(panic_info));
            token.cancel();
        }));
    }

    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Waits for an OS signal or an internal trigger (panic or component
    /// failure), then cancels the token to initiate graceful shutdown.
    async fn run<F>(
        self,
        signal: F,
    ) where
        F: Future<Output = ()>,
    {
        tokio::select! {
            () = signal => {
                tracing::info!("Received shutdown signal. Initiating graceful shutdown...");
                self.token.cancel();
            }
            () = self.token.cancelled() => {
                tracing::info!("Shutdown triggered by internal error.");
            }
        }
    }

    /// Spawns the shutdown coordinator as a background task
    pub fn watch_shutdown_signal(self) -> tokio::task::JoinHandle<()> {
        let signal = ctrl_c_signal();
        tokio::task::spawn(async move { self.run(signal).await })
    }
}
