//! The task tracker module.
//!
//! Contains utilities for orchestrating crucial initialization & loop tasks,
//! and logic for handling errors occurring in them.
use std::error::Error as StdError;
use std::fmt::{
    Debug,
    Display,
};
use std::future::Future;

use async_lock::RwLockUpgradableReadGuard;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{
    self,
    Receiver,
    Sender,
    UnboundedReceiver,
    UnboundedSender,
};
use tokio::task::JoinHandle;
use tokio_util::task::TaskTracker as InnerTaskTracker;

use crate::error::{
    Error,
    PrettyCause,
    TaskError,
};

use super::shutdown::{
    ShutdownNotification,
    ShutdownOutcome,
};

/// Needed to place [`TaskName`]s in error types.
pub trait Print: Display + Debug {}

impl<T: Display + Debug> Print for T {}

pub type TaskName = Box<dyn Print + Send>;

fn create_closed() -> InnerTaskTracker {
    let tt = InnerTaskTracker::new();

    tt.close();

    tt
}

fn spawn_inner<T, E>(
    tt: &InnerTaskTracker,
    name: impl Print + Send + 'static,
    task: impl Future<Output = Result<T, E>> + Send + 'static,
    handler: impl FnOnce(T) + Send + 'static,
    send_error: impl Fn((TaskName, E)) + Send + 'static,
) -> JoinHandle<()> {
    tt.spawn(async move {
        match task.await {
            Ok(output) => handler(output),
            Err(error) => send_error((Box::new(name), error)),
        }
    })
}

async fn wait_inner<S>(
    tt: InnerTaskTracker,
    error_tx: S,
    handler: impl Future<Output = Result<(), Error>>,
) -> Result<(), Error> {
    // `self` holds the last `error_tx`, so we need to drop it; otherwise it'll
    // create a deadlock on `error_rx.recv()` (see `wait_inner()` callers).
    drop(error_tx);

    handler.await?;

    tt.wait().await;

    Ok(())
}

fn print_fatal_error<E: StdError>(
    from: &TaskName,
    error: &(impl PrettyCause<E> + Display),
) {
    tracing::error!(
        "Received a fatal error from {from}:{}",
        error.pretty_cause()
    );
}

/// The main task tracker for processing the core main loop.
///
/// Only 1 should be used across the program (see [`Self::wait_and_shutdown`]).
/// Otherwise, the shutdown behaviour is unknown.
#[derive(Clone)]
pub struct TaskTracker {
    inner: InnerTaskTracker,
    error_tx: UnboundedSender<(TaskName, Error)>,
}

impl TaskTracker {
    /// [`tokio::mpsc::UnboundedReceiver`] can't be cloned and hence isn't
    /// stored inside the struct, so it's the caller's responsibility to
    /// take the receiver and then pass it to [`Self::wait_and_shutdown`].
    pub fn new() -> (
        Self,
        UnboundedReceiver<(TaskName, Error)>,
    ) {
        let (error_tx, error_rx) = mpsc::unbounded_channel();

        (
            Self {
                inner: create_closed(),
                error_tx,
            },
            error_rx,
        )
    }

    /// Starts the program main loop & waits until all tasks are finished.
    ///
    /// If any errors would occur in awaited tasks, `shutdown_notification` is
    /// triggered to shut down all listening it tasks, and, eventually, the
    /// program.
    pub async fn wait_and_shutdown(
        self,
        mut error_rx: UnboundedReceiver<(TaskName, Error)>,
        shutdown_notification: ShutdownNotification,
    ) {
        wait_inner(self.inner, self.error_tx, async {
            let mut no_errors = true;

            while let Some((from, error)) = error_rx.recv().await {
                print_fatal_error(&from, &error);

                if no_errors {
                    let outcome = shutdown_notification
                        .outcome
                        .upgradable_read()
                        .await;

                    if matches!(*outcome, ShutdownOutcome::UserRequested) {
                        *RwLockUpgradableReadGuard::upgrade(outcome).await =
                            ShutdownOutcome::UnrecoverableError {
                                panic: false,
                            };

                        shutdown_notification.token.cancel();
                    }

                    no_errors = false;
                }
            }

            Ok(())
        })
        .await
        .unwrap_or_else(|error| {
            // The closure above only ever returns `Ok`, so this is dead in
            // practice — but this runs during shutdown, where a panic would
            // abort the process instead of letting the tasks wind down.
            tracing::error!(
                error.source = ?error,
                "The task tracker's shutdown future returned an error"
            );
        });
    }
}

type ErrorChannel = (
    Sender<(TaskName, TaskError)>,
    Receiver<(TaskName, TaskError)>,
);

/// The short-lived task tracker.
///
/// Should be used for tracking temporal tasks (e.g. initialization ones).
#[expect(clippy::module_name_repetitions)]
pub struct ShortTaskTracker {
    inner: InnerTaskTracker,
    error_channel: ErrorChannel,
}

#[expect(dead_code)]
impl ShortTaskTracker {
    pub fn new() -> Self {
        Self {
            inner: create_closed(),
            error_channel: mpsc::channel(1),
        }
    }

    pub fn spawn(
        &self,
        name: impl Print + Send + 'static,
        task: impl Future<Output = Result<(), TaskError>> + Send + 'static,
    ) -> JoinHandle<()> {
        let error_tx = self.error_channel.0.clone();

        spawn_inner(
            &self.inner,
            name,
            task,
            |()| (),
            move |tn_and_error| {
                if let Err(
                    TrySendError::Full((from, error)) | TrySendError::Closed((from, error)),
                ) = error_tx.try_send(tn_and_error)
                {
                    print_fatal_error(&from, &error);
                }
            },
        )
    }

    /// Waits until all [`spawn`](Self::spawn)ed tasks are finished.
    ///
    /// If an error occurs in awaited tasks, this method immediately returns the
    /// first error. Do note that it doesn't cancel unfinished tasks, they
    /// will continue running until completion & log all occured errors.
    pub async fn try_wait(mut self) -> Result<(), Error> {
        wait_inner(
            self.inner,
            self.error_channel.0,
            async {
                if let Some((from, error)) = self.error_channel.1.recv().await {
                    return Err(Error::Task(from, error));
                }

                Ok(())
            },
        )
        .await
    }
}
