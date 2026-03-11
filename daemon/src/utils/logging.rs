//! Logging constants for structured logging across the application.
//!
//! These constants are used in tracing fields to enable consistent log
//! categorization and filtering in production.

/// Log category constants for identifying the source subsystem
pub mod category {
    pub const AUTH: &str = "auth";
    pub const CHAIN_CLIENT: &str = "chain_client";
}

/// Log operation constants for identifying specific operations within
/// subsystems
pub mod operation {
    // Auth operations
    pub const LOGIN: &str = "login";
    pub const CODE_EXCHANGE: &str = "code_exchange";
    pub const TOKEN_REFRESH: &str = "token_refresh";
    pub const SESSION_VALIDATION: &str = "session_validation";

    // Chain client operations
    pub const CONNECT_CLIENT: &str = "connect_client";
    pub const FETCH_BALANCE: &str = "fetch_balance";
    pub const FETCH_ASSET_INFO: &str = "fetch_asset_info";
    pub const FETCH_STORAGE: &str = "fetch_storage";
    pub const SUBMIT_TRANSACTION: &str = "submit_transaction";
    pub const WATCH_TRANSACTION: &str = "watch_transaction";
    pub const BUILD_TRANSFER: &str = "build_transfer";
    pub const SUBSCRIBE_TRANSFERS: &str = "subscribe_transfers";
}
