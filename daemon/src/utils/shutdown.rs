//! The shutdown module.
//!
//! Provides a single entry point for managing graceful shutdown, including:
//! - OS signal listening (SIGINT)
//! - Panic hook installation
//! - [`CancellationToken`] distribution to components

use std::panic::{
    self,
    PanicHookInfo,
};

use tokio::signal;
use tokio_util::sync::CancellationToken;

use crate::error::Error;

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
    pub async fn run(self) -> Result<(), Error> {
        tokio::select! {
            result = signal::ctrl_c() => {
                result.map_err(Error::ShutdownSignal)?;
                tracing::info!("Received shutdown signal. Initiating graceful shutdown...");
                self.token.cancel();
            }
            () = self.token.cancelled() => {
                tracing::info!("Shutdown triggered by internal error.");
            }
        }

        Ok(())
    }
}
