//! The shutdown module.
//!
//! Handles a graceful shutdown with an asynchronous runtime with respect of
//! panics (by incorporating a custom panic handler) & unrecoverable errors from
//! another modules.

use std::fmt::{
    Display,
    Formatter,
    Result as FmtResult,
};
use std::io::Result as IoResult;
use std::panic::{
    self,
    PanicHookInfo,
};
use std::sync::Arc;
use std::time::Duration;

use async_lock::{
    RwLock,
    RwLockUpgradableReadGuard,
};
use tokio::{
    signal,
    time,
};
use tokio_util::sync::CancellationToken;

use crate::error::Error;

const TIP_FUSE_SECS: u64 = 15;

#[derive(Clone)]
#[expect(clippy::module_name_repetitions)]
pub struct ShutdownNotification {
    pub token: CancellationToken,
    pub outcome: Arc<RwLock<ShutdownOutcome>>,
}

impl ShutdownNotification {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            outcome: Arc::new(RwLock::new(
                ShutdownOutcome::UserRequested,
            )),
        }
    }
}

pub async fn listener(
    shutdown_notification: CancellationToken,
    shutdown_completed: CancellationToken,
) -> Result<(), Error> {
    tokio::select! {
        biased;
        result = signal::ctrl_c() => {
            process_signal(result)?;

            tracing::info!("Received the shutdown signal. Initialising the shutdown...");

            shutdown_notification.cancel();
        }
        () = shutdown_notification.cancelled() => {
            tracing::info!("The shutdown started by an unrecoverable error...");
        }
    }

    let shutdown_completed_clone = shutdown_completed.clone();
    let tip = tokio::spawn(async move {
        tokio::select! {
            biased;
            () = shutdown_completed_clone.cancelled() => {}
            () = time::sleep(Duration::from_secs(TIP_FUSE_SECS)) => {
                tracing::warn!(
                    "Send the shutdown signal one more time to early terminate the daemon \
                    instead of waiting for the graceful shutdown."
                );
            }
        }
    });

    tokio::select! {
        biased;
        () = shutdown_completed.cancelled() => {}
        result = signal::ctrl_c() => {
            process_signal(result)?;

            tracing::info!("Received the second shutdown signal. Terminating the daemon...");

            return Ok(());
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "the tip task body is a `tokio::select!` over two cancellation futures and cannot panic"
    )]
    tip.await
        .expect("tip task shouldn't panic");

    tracing::debug!("The shutdown signal listener is shut down.");

    Ok(())
}

fn process_signal(result: IoResult<()>) -> Result<(), Error> {
    result.map_err(Error::ShutdownSignal)?;

    // Print shutdown log messages on the next line after the Control-C command.
    println!();

    Ok(())
}

#[derive(Clone, Copy)]
#[expect(clippy::module_name_repetitions)]
pub enum ShutdownOutcome {
    UserRequested,
    UnrecoverableError { panic: bool },
}

pub fn set_panic_hook(
    print: impl Fn(PrettyPanic<'_>) + Send + Sync + 'static,
    shutdown_notification: ShutdownNotification,
) {
    panic::set_hook(Box::new(move |panic_info| {
        let outcome = shutdown_notification
            .outcome
            .upgradable_read_blocking();

        let first = if matches!(
            *outcome,
            ShutdownOutcome::UnrecoverableError {
                panic: true
            }
        ) {
            false
        } else {
            *RwLockUpgradableReadGuard::upgrade_blocking(outcome) =
                ShutdownOutcome::UnrecoverableError {
                    panic: true,
                };

            shutdown_notification.token.cancel();

            true
        };

        print(PrettyPanic {
            panic_info,
            first,
        });
    }));
}

pub struct PrettyPanic<'a> {
    panic_info: &'a PanicHookInfo<'a>,
    first: bool,
}

// It looks like it's impossible to acquire `PanicInfo` outside of
// `panic::set_hook`, which could alter execution of other unit tests, so,
// without mocking the `panic_info` field, there's no way to test the
// `Display`ing.
impl Display for PrettyPanic<'_> {
    fn fmt(
        &self,
        f: &mut Formatter<'_>,
    ) -> FmtResult {
        f.write_str("A panic detected")?;

        if let Some(location) = self.panic_info.location() {
            f.write_str(" at ")?;
            location.fmt(f)?;
        }

        let payload = self.panic_info.payload();
        let message_option = match payload.downcast_ref() {
            Some(string) => Some(*string),
            #[expect(
                clippy::string_slice,
                reason = "`[..]` is the full range, so there is no character boundary to land inside"
            )]
            None => payload
                .downcast_ref::<String>()
                .map(|string| &string[..]),
        };

        if let Some(panic_message) = message_option {
            f.write_str(":\n    ")?;
            f.write_str(panic_message)?;
        }

        f.write_str(".")?;

        // Print the report request only on the first panic.

        if self.first {
            f.write_str(concat!(
                "\n\nThis is a bug. Please report it at ",
                env!("CARGO_PKG_REPOSITORY"),
                "/issues."
            ))?;
        }

        Ok(())
    }
}
