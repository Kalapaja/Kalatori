# Architecture

## System Overview

Kalatori is a self-hosted, non-custodial blockchain payment gateway daemon. It receives requests to create invoices with specific amounts and assets, generates unique payment accounts for each invoice, monitors blockchains for incoming payments, marks invoices as paid when payment is detected, and automatically withdraws funds to the merchant's recipient address.

**Supported chains**: Polkadot Asset Hub (via subxt) and Polygon (via alloy).

Called by external systems (e.g. e-commerce platforms) via HTTP API.

> **Note**: This document describes the current implementation. Future directions are in a labeled section at the end. When in doubt, verify against source code.

## High-Level Flow

```
HTTP Request → API Server (Axum) → AppState (Arc<AppState>)
                                      ├→ DAO (SQLite via sqlx)
                                      ├→ KeyringClient (mpsc → Keyring actor)
                                      └→ InvoiceRegistry (in-memory)

Background Tasks:
  TransfersTracker (per chain) → TransactionsRecorder → DAO
  TransfersExecutor → chain clients + Keyring → DAO
  ExpirationDetector → DAO + chain clients
  WebhookSender → DAO → external webhook URLs
```

## Component Responsibilities

### `daemon/src/state.rs` — AppState
Plain struct (NOT an actor). Passed as `Arc<AppState>` to Axum handlers. Provides async methods for invoice creation, status queries, payment marking. Coordinates DAO, KeyringClient, and InvoiceRegistry. Generic over `D: DaoInterface` for testability via mockall.

### `daemon/src/api.rs` + `daemon/src/api/` — HTTP API
Axum server with four namespaces:
- `/public` — Publicly accessible, no auth, sanitized responses
- `/private` — HMAC-authenticated merchant endpoints
- `/internal` — Internal operations
- `/dev` — Development/debug endpoints (feature-gated via `dev_api`)

`ApiErrorExt` trait in `api.rs` provides `category()`, `code()`, `message()`, `http_status_code()` for structured error responses. Request IDs via `x-request-id` header (UUID, auto-generated).

### `daemon/src/dao.rs` + `daemon/src/dao/` — Data Access Object
SQLite via sqlx 0.8. `DaoInterface` + `DaoTransactionInterface` traits (mockable). `DaoExecutor` trait for query execution. Submodules: `invoice.rs`, `transaction.rs`, `payout.rs`, `refund.rs`, `swap.rs`, `webhook_event.rs`, `changes.rs`. Migrations in `./migrations/`. See [docs/DATABASE.md](DATABASE.md) for schema.

### `daemon/src/chain_client.rs` + `daemon/src/chain_client/` — Blockchain Clients
`BlockChainClient` trait with `ChainConfig` associated types. Two implementations:
- `asset_hub.rs` — Asset Hub via subxt 0.44 (sr25519 keys, SCALE encoding)
- `polygon.rs` — Polygon via alloy 1.5 (secp256k1 keys, ERC-20 tokens, Pimlico paymaster for gas abstraction)

`AssetInfoStore` trait for per-chain asset metadata. Error types in `errors.rs` follow the [error handling principles](error-handling.md).

### `daemon/src/chain_client/keyring.rs` — Keyring (Actor)
Actor pattern: mpsc channel + oneshot responses. Holds seed phrase (`Zeroize` + `ZeroizeOnDrop`). Handles both:
- **Asset Hub**: sr25519 key derivation via `subxt_signer`, hard derivation with `DeriveJunction`
- **Polygon**: secp256k1 key derivation via alloy `MnemonicBuilder`, BIP-44 path from hashed params

Client interface: `KeyringClient` (mockable via `mockall_double`).

### `daemon/src/chain/` — Chain Monitoring & Execution
- **`transfer_tracker.rs`** (`TransfersTracker`): Subscribes to finalized blocks per chain, detects incoming transfers, and notifies `TransactionsRecorder`. Failed subscriptions and streams that end before delivering an event use a cancellation-aware exponential retry delay (1–60 seconds). Retry state resets only after a stream delivers an event; degradation is reported on entry and at most once per minute, with recovery reported separately.
- **`transactions_recorder.rs`** (`TransactionsRecorder`): Records detected transactions to DB, updates `InvoiceRegistry`
- **`executor.rs`** (`TransfersExecutor`): Builds and submits payout transactions for both chains. Single executor instance handles Asset Hub + Polygon.
- **`invoice_registry.rs`** (`InvoiceRegistry`): In-memory tracking of active invoices and their expected amounts. Thread-safe (internal `RwLock`).

### `daemon/src/expiration_detector.rs` — ExpirationDetector
Periodic background task. Checks for expired invoices, handles cleanup and status transitions.

### `daemon/src/webhook_sender.rs` — WebhookSender
Periodic background task. Sends unsent webhook events from DB to configured URLs with HMAC signatures.

### `daemon/src/etherscan_client.rs` — EtherscanClient
Client for Etherscan/Polygonscan API. Used by ExpirationDetector for transaction verification on EVM chains.

### `daemon/src/types/` — Domain Types
Business logic models: `Invoice`, `Payout`, `Transaction`, `Refund`, `Swap`, `WebhookEvent`, `Changes`. Separate from DAO row types and API response types.

### `daemon/src/error.rs` — Legacy Error Types
Monolithic `Error` enum with `PrettyCause` trait. Being migrated to domain-specific errors (see `chain_client/errors.rs`). `thiserror` derive for all error types.

### `daemon/src/utils/` — Utilities
- `logger.rs` — tracing-subscriber setup, optional Loki integration
- `logging.rs` — Structured log category/operation constants
- `task_tracker.rs` — Wraps `tokio_util::task::TaskTracker` with error collection
- `shutdown.rs` — `ShutdownNotification`, `CancellationToken`, panic hook, signal handling

### `client/` — Public Client Library
Rust crate for integrating with Kalatori: HTTP client, shared types (API types, invoice/transaction types), HMAC utilities, Axum middleware for signature verification.

## Key Derivation

### Asset Hub (sr25519)
```
Seed Phrase (BIP39) → sr25519 root keypair
  → hard derivation with invoice params → Unique Payment Account
```
Derivation params are `Vec<String>` — typically `[invoice_uuid.to_string()]`. Each param becomes a `DeriveJunction::hard`.

### Polygon (secp256k1)
```
Seed Phrase (BIP39) → SHA-256 hash of derivation params
  → account = first 4 bytes (& 0x7FFFFFFF)
  → index = next 4 bytes
  → BIP-44 path: m/44'/60'/{account}'/0/{index}
  → Unique Payment Account
```

Both are deterministic: same seed + same invoice params = same payment account.

## Payment Lifecycle

1. **Invoice created** via API → AppState derives payment address via Keyring → saves to DB → adds to InvoiceRegistry
2. **Customer pays** to payment address on-chain
3. **TransfersTracker** detects incoming transfer in finalized block → TransactionsRecorder saves transaction to DB, updates invoice status in InvoiceRegistry
4. **TransfersExecutor** picks up payouts from DB → builds transaction → signs via Keyring → submits to chain → records result
5. **ExpirationDetector** periodically checks for expired invoices → updates status
6. **WebhookSender** periodically sends unsent webhook events to configured URLs

## Configuration System

Eight config types loaded at startup (all support env var overrides):

| Config | File | Key Fields |
|--------|------|------------|
| Chains | `chains.json` | Chain endpoints, assets (mandatory) |
| Payments | `payments.json` | Recipient addresses, account lifetime, default chain/asset |
| Secrets | `secrets.json` | BIP39 seed phrase, API secret key |
| Database | `database.json` | Database path, temporary mode, fail-closed existing-database requirement |
| Web Server | (defaults) | Host, port (default 0.0.0.0:16726) |
| Shop | `shop.json` | Webhook URL, shop metadata, signature max age |
| Logger | `logger.json` | Log level, Loki endpoint |
| Etherscan | `etherscan_client.json` | API key for Etherscan/Polygonscan |

**Env var pattern**: `{PREFIX}_{CONFIG}_{FIELD}` (e.g., `KALATORI_PAYMENTS_RECIPIENT`)
**Custom prefix**: `KALATORI_APP_ENV_PREFIX`
**Config directory**: `KALATORI_CONFIG_DIR_PATH`
**Security**: Seed phrase and API secret key are zeroized from env/memory after loading.

Example configs in `configs/` directory.

At startup, the daemon always runs SQLite's `PRAGMA integrity_check` before migrations. Set
`require_existing` (or `KALATORI_DATABASE_REQUIRE_EXISTING`) to refuse startup when the configured
database file is missing or empty; this is incompatible with temporary in-memory mode.

## Background Task Management

**TaskTracker** (`daemon/src/utils/task_tracker.rs`):
- Wraps `tokio_util::task::TaskTracker` with error collection via unbounded mpsc channel
- Any task error triggers application shutdown

**Shutdown sequence**:
1. Signal received (SIGTERM/SIGINT) or fatal error → `CancellationToken` cancelled
2. TaskTracker waits for all tasks, then cancels shutdown listener
3. All component handles joined: Keyring, TransfersExecutor, ExpirationDetector, both TransfersTrackers, WebhookSender, API server
4. Loki logs flushed
5. Clean exit

## Known Limitations

1. **Configuration**: Hardcoded RPC URLs in Makefile (see TODOs)
2. **Scalability**: TransfersTracker queries all watched accounts every block
3. **Metadata**: Manual `metadata.scale` update process (should be automated)

## Future Vision

- Actor model only for chain monitoring and periodic tasks; rest via `Arc<State>` with direct async calls (largely done)
- DAO types migration: new types in `types` module, legacy types only in v2 API handlers
- Backward compatibility: existing API endpoints remain unchanged
