# Error Handling Principles

These principles guide error type design across the codebase, particularly in `chain_client` and related modules.

**Reference implementation**: `daemon/src/chain_client/errors.rs` demonstrates these principles in practice.

> **Note**: The legacy monolithic `Error` enum in `daemon/src/error.rs` predates these principles. New code should use domain-specific error types following the patterns below. The actual API error trait is `ApiErrorExt` in `daemon/src/api.rs` (see Principle 5 note).

## Principle 1: Only Enumerate Errors Requiring Different Handling

**Core Rule**: Create separate error variants ONLY when the calling code needs to behave differently based on the variant.

**Decision Test**: For any two error scenarios, ask: "Does the caller need to DO something different?"
- If YES (different retry logic, user message, metrics, etc.) → Separate variants
- If NO (same handling, only context differs) → Single variant with context fields

**Examples:**

✅ **Good - Variants have different handling:**
```rust
pub enum TransactionError {
    // Different handling: try different RPC endpoint
    NetworkError { endpoint: String },

    // Different handling: mark as failed, notify user
    InsufficientBalance {
        transaction_id: TxId,
        required: Option<Decimal>,
        available: Option<Decimal>,
    },

    // Different handling: wait for mortality period, lookup via API
    SubmissionStatusUnknown {
        transaction_hash: H256,
    },
}
```

❌ **Bad - Same handling, unnecessary variants:**
```rust
pub enum ChainError {
    // All handled identically: log and retry
    BlockTimestampFetchFailed,
    BlockHashFetchFailed,
    BlockExtrinsicsFetchFailed,
    // Should be: BlockDataFetchFailed { data_type: String }
}
```

**Anti-Patterns to Avoid:**

1. **Over-specification**: Don't create variants that differ only in log messages
2. **String-based discrimination**: If you find yourself doing `if error_msg.contains("balance")`, you probably need a variant
3. **Lost type safety**: Use enums for context fields when reasonable (not just `String`)

**Mitigations for Weaknesses:**

This principle trades some type safety for maintainability. Mitigate with:

1. **Structured logging** - Include error classification in logs:
   ```rust
   tracing::warn!(
       error.type = "storage_query_failed",
       error.operation = "fetch_balance",
       asset_id = %asset_id,
       "Storage query failed"
   );
   ```

2. **Enum context fields** - Use typed enums instead of strings when possible:
   ```rust
   pub enum Operation { FetchBalance, FetchMetadata }
   pub struct Error { operation: Operation }  // Not String
   ```

3. **Document handling** - Explain recovery strategy in error docs:
   ```rust
   /// Network error during submission.
   /// **Recovery:** Try different RPC endpoint immediately.
   NetworkError { endpoint: String },
   ```

4. **Error code constants** - For matching and metrics:
   ```rust
   impl Error {
       pub const NETWORK_ERROR: &'static str = "network_error";
       pub fn error_code(&self) -> &'static str { ... }
   }
   ```

**Context**: Our architecture uses a worker-based retry system with database-backed transaction state. Retry logic lives in the worker (caller), not in error types. Errors should classify WHAT went wrong, not prescribe HOW to fix it.

## Principle 2: Log Raw Errors at the Point They Occur

**Core Rule**: When converting a library error (like `subxt::Error`) to a custom error type, log the full original error at the exact conversion point before transformation.

**Why**: Once you convert `subxt::Error` → `ChainError`, you lose rich library error details (request IDs, internal state, specific error codes). Logs preserve this information for debugging.

**Pattern:**

```rust
// ❌ BAD: Convert without logging
.map_err(|e| ChainError::ConnectionFailed { endpoint })?

// ✅ GOOD: Log with structured fields, then convert
.map_err(|e| {
    tracing::debug!(
        error.category = "chain_client",
        error.operation = "fetch_balance",
        asset_id = %asset_id,
        account = %account,
        error.source = ?e,  // Full library error
        "Balance fetch failed"
    );
    ChainError::StorageQueryFailed { ... }
})?
```

**Log Level Guidelines:**

| Level | When to Use | Example |
|-------|-------------|---------|
| DEBUG | Error conversions, expected failures | Balance fetch for new account |
| INFO | Significant business events | "Payout completed", "Order paid" |
| WARN | Recoverable errors, degraded state | "RPC endpoint degraded" |
| ERROR | Critical failures requiring attention | "All RPC endpoints down" |

**Correlation IDs for Multi-Step Operations:**

Generate request IDs at entry points (HTTP handlers via `x-request-id` header, worker job pickup), then use `#[instrument]` for nested functions:

```rust
// Entry point: Generate correlation_id and create root span
async fn handle_payout_request(payout_id: u64, client: Client, db: Database) {
    let correlation_id = Uuid::new_v4();
    let span = tracing::info_span!(
        "payout_request",
        correlation_id = %correlation_id,
        payout_id = %payout_id
    );

    process_payout(payout_id, &client, &db)
        .instrument(span)
        .await
}

// Nested functions: Use #[instrument], automatically inherit correlation_id
#[instrument(skip(client, db))]
async fn process_payout(payout_id: u64, client: &Client, db: &Database) -> Result<()> {
    // All logs here include correlation_id and payout_id from parent span
    let balance = client.fetch_balance(...).await?;
    let tx = client.build_transfer(...).await?;
    Ok(())
}

#[instrument(skip(self))]
async fn fetch_balance(&self, asset_id: u32, account: &AccountId) -> Result<Decimal> {
    // Still includes correlation_id from root span
    self.client.storage().fetch(...).await
        .map_err(|e| {
            tracing::debug!(
                error.source = ?e,
                asset_id = %asset_id,
                // correlation_id automatically included from span
                "Balance fetch failed"
            );
            ChainError::StorageQueryFailed { ... }
        })
}
```

**Standard Categories** (defined in `daemon/src/utils/logging.rs`):

```rust
pub mod category {
    pub const CHAIN_CLIENT: &str = "chain_client";
}

pub mod operation {
    pub const CONNECT_CLIENT: &str = "connect_client";
    pub const FETCH_BALANCE: &str = "fetch_balance";
    pub const FETCH_ASSET_INFO: &str = "fetch_asset_info";
    pub const SUBMIT_TRANSACTION: &str = "submit_transaction";
    // ...
}
```

**The Layer Rule** - Avoid duplicate logging (see also [docs/conventions.md](conventions.md)):

- **Layer 3** (conversion boundary): Log raw library error with structured fields
- **Layer 2** (intermediate): Don't log, just convert custom error types
- **Layer 1** (handler): Log business-level error for user/ops

```rust
// Layer 3: chain_client (conversion boundary)
#[instrument(skip(self))]
async fn fetch_balance(...) -> Result<Decimal, ChainError> {
    self.client.storage().fetch(...).await
        .map_err(|e| {
            tracing::debug!(error.source = ?e, ...);  // ← Log here
            ChainError::StorageQueryFailed { ... }
        })
}

// Layer 2: payout logic
#[instrument(skip(client))]
async fn execute_payout(...) -> Result<(), PayoutError> {
    fetch_balance(...).await
        .map_err(|e| PayoutError::from(e))?  // ← Don't log, just convert
}

// Layer 1: payout worker
match execute_payout(...).await {
    Err(e) => tracing::warn!(payout_id, error = %e, ...)  // ← Log business error
}
```

**Production Configuration:**

```bash
# Default: INFO level, DEBUG for chain_client only
RUST_LOG=info,kalatori::chain_client=debug

# Output format: JSON for aggregation
RUST_LOG_FORMAT=json
```

**Context**: We use INFO level in production with DEBUG for detailed modules. Structured JSON output enables future log aggregation. Correlation IDs are critical for debugging multi-step payout/order workflows.

## Principle 3: Include Useful and Required Information Only

**Core Rule**: Error struct fields should pass the "actionability test" - include information that enables decision-making, recovery, or user communication. Avoid fields that belong in logs or database.

**The Actionability Test** - For each field, ask:

1. **Does this change what code DOES?** → Required
2. **Is it needed for user communication?** → Useful
3. **Can it be reconstructed from context?** → Remove (caller already has it)
4. **Is it only for debugging?** → Remove (put in logs via Principle 2)

**Examples:**

```rust
// ❌ BAD: Duplicates caller's context
async fn execute_payout(
    payout_id: u64,
    sender: AccountId,
    recipient: AccountId,
) -> Result<(), PayoutError> {
    // ...
    Err(PayoutError {
        payout_id,    // ← Caller already has these
        sender,       // ← Caller already has these
        recipient,    // ← Caller already has these
    })
}

// ✅ GOOD: Minimal error, caller has context
async fn execute_payout(
    payout_id: u64,
    sender: AccountId,
    recipient: AccountId,
) -> Result<(), PayoutError> {
    // ...
    Err(PayoutError::TransferFailed)  // ← Caller has payout_id in scope
}
```

**Project-Specific Rules:**

1. **Never include `endpoint`** - Logged at error site, available via `client.current_endpoint()` if needed
2. **Never include timestamps** - Always in logs and database records
3. **Never include retry state** - Worker manages retries via database (retry_count, last_attempt_at, etc.)
4. **Never include transaction hash for pre-finalization errors** - Caller has internal transaction ID; hash is unreliable on Asset Hub
5. **Include blockchain coordinates only when blockchain becomes source of truth** - After finalization, need (block_number, extrinsic_index) to re-query chain

**Source of Truth Pattern:**

| Lifecycle Stage | Source of Truth | What Error Needs |
|-----------------|-----------------|------------------|
| Planned (in DB) | Database | Nothing (caller has internal ID) |
| Submitted (unknown) | Database | Nothing (caller has internal ID) |
| Finalized | Blockchain | Coordinates (block_number, extrinsic_index) |

```rust
// Pre-finalization: No identifier needed
pub enum TransactionError {
    SubmissionStatusUnknown,  // ← Caller has internal_tx_id
}

// Caller code:
let internal_tx_id = db.create_planned_transaction(...)?;
match client.submit_transaction(...).await {
    Err(TransactionError::SubmissionStatusUnknown) => {
        // internal_tx_id is RIGHT HERE in scope
        db.mark_transaction_unknown_state(internal_tx_id)?;
    }
}

// Post-finalization: Blockchain coordinates needed
pub enum TransactionError<T: ChainConfig> {
    ExecutionFailed {
        transaction_id: T::TransactionId,  // e.g., (block_number, extrinsic_index)
        error_code: String,
    }
}

// Caller code:
match result {
    Err(TransactionError::ExecutionFailed { transaction_id, .. }) => {
        // Can retry fetching from blockchain using coordinates
        client.refetch_transaction_info(transaction_id).await?;
    }
}
```

**Use `Option<T>` When:**
- Information genuinely might not be available (chain error doesn't include amounts)
- Handling can degrade gracefully

```rust
// ✅ Good use of Option
InsufficientBalance {
    transaction_id: TxId,           // ← Always have this
    required: Option<Decimal>,      // ← Chain might not provide
    available: Option<Decimal>,     // ← Might not have fetched
}

// Handling degrades gracefully:
match error {
    InsufficientBalance { required: Some(r), available: Some(a), .. } => {
        format!("Need {} more", r - a)  // Best case
    }
    InsufficientBalance { .. } => {
        "Insufficient balance".to_string()  // Fallback
    }
}
```

**Prefer Struct Variants Over Tuples:**

```rust
// ❌ Unclear
FetchTransactionInfoError((BlockNumber, H256))  // Which is which?

// ✅ Clear
FetchTransactionInfoError {
    block_number: u32,
    transaction_hash: H256,
}
```

**Where Additional Info Lives:**

Document in error type where to find information not included:

```rust
/// Connection operation failed.
///
/// **Available information:**
/// - Operation: In error type
/// - Endpoint: `client.current_endpoint()`
/// - Timestamp: In logs with correlation_id
/// - Retry state: In database payout/transaction record
pub enum ChainError {
    ConnectionFailed { operation: String },
}
```

**Context**: Our architecture has multiple sources of truth: database (planned transactions, retry state), logs (timestamps, endpoints, detailed errors), and blockchain (finalized transactions). Errors only include what's not available elsewhere or what's needed for immediate handling decisions.

## Principle 4: Separate Error Enums for Different Domains

**Core Rule**: Create multiple focused error types for different **usage contexts** (not technical categories). Split by what the caller is doing, not by error's technical nature.

**The Domain Test**: Errors belong in the same enum if they:
1. Share the same calling context (same functions produce/handle them)
2. Require similar recovery strategies
3. Represent the same abstraction level

**Project Domains** (chain_client):

```rust
// 1. Initialization
pub enum ClientError {
    AllEndpointsUnreachable,
    MetadataFetchFailed,
    InvalidConfiguration { field: String },
    UnknownAssetId { asset_id: u32 },  // Validated at init AND runtime
}

// 2. One-off blockchain queries
pub enum QueryError {
    RpcRequestFailed,        // Try different endpoint
    NotFound { query_type: String },
    DecodeFailed { data_type: String },
}

// 3. Block streaming
pub enum SubscriptionError {
    SubscriptionFailed,      // Restart subscription
    StreamClosed,
    BlockProcessingFailed { block_number: u32 },  // Skip block
}

// 4. Transaction lifecycle
pub enum TransactionError<T: ChainConfig> {
    BuildFailed { reason: String },
    SubmissionStatusUnknown,  // Mark unknown in DB
    ExecutionFailed {
        transaction_id: T::TransactionId,  // Post-finalization
        error_code: String,
    },
    InsufficientBalance {
        transaction_id: T::TransactionId,
        required: Option<Decimal>,
        available: Option<Decimal>,
    },
    UnknownAsset {
        transaction_id: T::TransactionId,
        asset_id: T::AssetId,
    },
}
```

**Why separate QueryError and SubscriptionError?**
Different recovery: queries retry immediately with different endpoint; subscriptions restart entire stream.

**Cross-Domain Conversion:**

Use `From` for obvious conversions:
```rust
impl From<KeyringError> for TransactionError<T> {
    fn from(e: KeyringError) -> Self {
        tracing::debug!(error.source = ?e, ...);  // Log conversion (Principle 2)
        TransactionError::BuildFailed { reason: "Signing failed".into() }
    }
}
```

Use `.map_err()` when context matters:
```rust
client.fetch_balance(...).await
    .map_err(|e| PayoutError::PreflightCheckFailed {
        check: "balance", underlying: e.to_string()
    })?
```

**API Layer Boundary:**

Internal errors never leak to public API. Convert at handler:
```rust
async fn handler(...) -> Result<Json<Response>, ApiError> {
    state.execute(...).await
        .map_err(|e| match e {
            InternalError::Specific { .. } => ApiError {
                code: "error_code",
                description: "User message",
                extra_data: Some(json!({ ... })),
            },
            // ... conversions
        })?;
}
```

**Avoid Unifier Enums Internally:**

✅ **Preferred** - Separate return types:
```rust
async fn fetch_balance(...) -> Result<Decimal, QueryError>;
async fn subscribe_transfers(...) -> Result<Stream, SubscriptionError>;
```

✅ **Acceptable** - Flattened enum (not nested unifier):
```rust
pub enum ClientError {
    AllEndpointsUnreachable,   // From connection concern
    MetadataFetchFailed,       // From query concern
    InvalidConfiguration { field: String },
}
```

❌ **Avoid** - Deep unifier hierarchies:
```rust
pub enum ChainError {
    Client(ClientError),
    Query(QueryError),
    // ...
}
```

**Relationship to Principle 1:**
- Principle 1 (within domain): Only enumerate errors requiring different handling
- Principle 4 (between domains): Separate error types for different usage contexts

**Context**: Usage-based domains align with recovery strategies. Each domain has focused error handling: initialization fails fast, queries retry with failover, subscriptions restart stream, transactions use DB-backed retry worker.

## Principle 5: Internal Errors Shouldn't Leak to API

**Core Rule**: Public API responses must never expose secrets or unnecessarily verbose internal details. All handler errors implement a trait to provide their own API representation.

**Key Innovation**: Instead of centralized conversion functions, each error type defines its own API representation via trait. This is decentralized, type-safe, and exhaustive (compiler enforces complete coverage).

> **Current implementation**: The actual trait in the codebase is `ApiErrorExt` in `daemon/src/api.rs`, which provides `category()`, `code()`, `message()`, and `http_status_code()` methods plus a `to_api_error()` helper. The aspirational design below uses `KalatoriApiError` as the trait name with a blanket `IntoResponse` implementation. New error types should follow the `ApiErrorExt` pattern.

**The ApiErrorExt Trait** (actual, in `daemon/src/api.rs`):

```rust
pub trait ApiErrorExt: std::error::Error {
    fn category(&self) -> &str;
    fn code(&self) -> &str;
    fn message(&self) -> &str;
    fn http_status_code(&self) -> StatusCode;

    fn to_api_error(&self) -> ApiError {
        ApiError {
            category: self.category().to_string(),
            code: self.code().to_string(),
            message: self.message().to_string(),
            details: None,
        }
    }
}
```

**Target Design** (aspirational pattern for future blanket implementation):

```rust
pub trait KalatoriApiError: std::error::Error {
    /// Machine-readable error code (snake_case, stable across versions)
    fn code(&self) -> String;

    /// Human-readable error message (safe for display)
    fn message(&self) -> String;

    /// Optional structured data (flexible schema per error variant)
    fn data(&self) -> Option<serde_json::Value> {
        None  // Default: no extra data
    }

    /// HTTP status code for this error
    fn http_code(&self) -> StatusCode;
}
```

**Implementation Example:**

```rust
impl KalatoriApiError for PayoutError {
    fn code(&self) -> String {
        match self {
            PayoutError::InsufficientBalance { .. } => "insufficient_balance",
            PayoutError::ChainUnavailable => "service_unavailable",
            PayoutError::AccountNotFound => "account_not_found",
            PayoutError::InvalidRequest { .. } => "invalid_request",
            // Compiler enforces exhaustiveness - no `_ =>` needed!
        }.to_string()
    }

    fn message(&self) -> String {
        match self {
            PayoutError::InsufficientBalance { required, available, .. } => {
                match (required, available) {
                    (Some(r), Some(a)) => {
                        format!("Insufficient balance. Required: {}, Available: {}", r, a)
                    },
                    _ => "Insufficient balance to complete payout.".to_string(),
                }
            },
            PayoutError::ChainUnavailable => {
                "Blockchain temporarily unavailable. Please retry.".to_string()
            },
            PayoutError::AccountNotFound => {
                "Payment account not found or expired.".to_string()
            },
            PayoutError::InvalidRequest { reason } => {
                format!("Invalid request: {}", reason)
            },
        }
    }

    fn data(&self) -> Option<serde_json::Value> {
        match self {
            PayoutError::InsufficientBalance { transaction_id, required, available } => {
                Some(json!({
                    "internal_transaction_id": transaction_id,  // OK to include
                    "required": required.map(|d| d.to_string()),
                    "available": available.map(|d| d.to_string()),
                }))
            },
            PayoutError::InvalidRequest { field, reason } => {
                Some(json!({
                    "field": field,
                    "reason": reason,
                }))
            },
            _ => None,
        }
    }

    fn http_code(&self) -> StatusCode {
        match self {
            PayoutError::InsufficientBalance { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            PayoutError::ChainUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            PayoutError::AccountNotFound => StatusCode::NOT_FOUND,
            PayoutError::InvalidRequest { .. } => StatusCode::BAD_REQUEST,
        }
    }
}
```

**Handler Usage:**

Handlers simply return the error - trait converts automatically:

```rust
async fn create_payout_handler(
    State(state): State<AppState>,
    Path(payout_id): Path<u64>,
) -> Result<Json<PayoutResponse>, PayoutError> {
    state.execute_payout(payout_id).await
        .map_err(|e| {
            tracing::warn!(
                payout_id = %payout_id,
                error.internal = ?e,       // Log full internal error
                error.code = e.code(),     // API error code
                "Payout execution failed"
            );
            e  // Return error - IntoResponse handles conversion
        })?;

    Ok(Json(result))
}
```

**What NOT to Expose:**

Never include:
1. **Secrets** - Seed phrases, private keys, API tokens
2. **Security-sensitive info** - Database connection strings, authentication tokens
3. **Stack traces** - Use request_id in logs instead
4. **Raw library errors** - Full `subxt::Error` details (convert to meaningful message)
5. **Implementation details** - "Failed to connect to sqlite", "database locked"

Safe to include:
1. **Identifiers** - Order IDs, internal transaction IDs, asset IDs
2. **Business data** - Amounts, balances, asset names
3. **Actionable info** - Validation errors, required fields, retry counts
4. **System state** - Queue positions, worker IDs (when useful)
5. **Blockchain data** - Block numbers, transaction hashes, extrinsic indices

**Guideline**: Include information that helps users/operators understand and act on the error. Exclude only secrets and unnecessarily verbose internals.

**ApiError Structure (Output Only):**

Defined in `client/src/types/api.rs`:

```rust
#[derive(Serialize)]
pub struct ApiError {
    pub category: String,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
```

**Error Code Conventions:**

Format: `snake_case`, stable across versions

Standard codes: `invalid_request`, `validation_failed`, `account_not_found`, `order_not_found`, `insufficient_balance`, `payment_already_processed`, `asset_not_supported`, `service_unavailable`, `internal_error`, `blockchain_error`, `timeout`

Stability: Can add codes, change messages, add data fields. Cannot remove/rename without version bump.

**Cross-Domain Conversion:**

When errors convert across domains, use `From` trait:

```rust
impl From<QueryError> for PayoutError {
    fn from(e: QueryError) -> Self {
        match e {
            QueryError::RpcRequestFailed => PayoutError::ChainUnavailable,
            QueryError::NotFound { .. } => PayoutError::AccountNotFound,
            QueryError::DecodeFailed { .. } => PayoutError::ChainDataCorrupted,
        }
    }
}
```

Only the final error type (returned from handler) needs `ApiErrorExt` implementation.

**Benefits:**

- Decentralized: Error definition and API representation in same place
- Type-safe: Compiler enforces exhaustiveness, no `_ =>` fallback needed
- Clean handlers: Auto-conversion via trait
- No centralized conversion boilerplate
- Maintainable: Add variant -> compiler forces trait implementation

**Relationship to Other Principles:**
- Principle 1: Only create variants requiring different handling - trait ensures each gets proper API representation
- Principle 3: Internal errors have rich context; trait methods extract only actionable info for API
- Principle 4: Each domain error implements trait independently

**Context**: Trait-based approach trades some coupling (errors know about HTTP) for massive gains in maintainability and type safety. No centralized conversion function to keep in sync. Request ID (in `x-request-id` header) links API errors to internal logs.
