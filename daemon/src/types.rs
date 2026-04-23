/// Module defining various types used across the Kalatori application.
/// Each domain-specific type (or collection of types) is organized into its own
/// submodule.
///
/// For testing purposes, it's also recommended to create fixtures functions
/// within each submodule to facilitate easy generation of test data. For
/// example: ```ignore
/// // In invoice.rs
/// #[cfg(test)]
/// fn default_invoice() -> Invoice {
///    // Create and return a default Invoice instance for testing
/// }
/// ```
mod admin;
mod changes;
mod fee_payout;
mod common;
mod invoice;
mod payout;
mod refund;
mod swap;
mod transaction;
mod webhook_event;

// Re-export commonly used types for convenience
pub use admin::*;
pub use changes::*;
pub use fee_payout::*;
pub use common::*;
pub use invoice::*;
pub use payout::*;
pub use refund::*;
pub use swap::*;
pub use transaction::*;
pub use webhook_event::*;
