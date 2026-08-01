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
- `/admin` — OAuth-session authenticated admin UI and API. Owner-only routes are
  `/api/integration-settings` and `/api/get-plugin`, because both disclose the
  merchant HMAC secret (the plugin embeds it). Read-only settings, identity,
  invoice, payout, transaction, and swap routes are available to Owner,
  Operator, Viewer, and Support. `/api/payout/initiate` is not routed at all —
  it answers 404, not an error status or a feature flag — until its hardcoded
  amount is replaced with product-defined amount semantics.
- `/dev` — Development/debug endpoints (feature-gated via `dev_api`)

`ApiErrorExt` trait in `api.rs` provides `category()`, `code()`, `message()`, `http_status_code()` for structured error responses. Request IDs via `x-request-id` header (UUID, auto-generated).

### `daemon/src/dao.rs` + `daemon/src/dao/` — Data Access Object
SQLite via sqlx 0.8. `DaoInterface` + `DaoTransactionInterface` traits (mockable). `DaoExecutor` trait for query execution. Submodules: `invoice.rs`, `transaction.rs`, `payout.rs`, `refund.rs`, `swap.rs`, `webhook_event.rs`, `changes.rs`. Migrations in `./migrations/`. See [docs/DATABASE.md](DATABASE.md) for schema.

### `daemon/src/chain_client.rs` + `daemon/src/chain_client/` — Blockchain Clients
`BlockChainClient` trait with `ChainConfig` associated types. Two implementations:
- `asset_hub.rs` — Asset Hub via subxt 0.44 (sr25519 keys, SCALE encoding)
- `polygon.rs` — Polygon via alloy 1.5 (secp256k1 keys, ERC-20 tokens, Pimlico paymaster for gas abstraction)

`AssetInfoStore` trait for per-chain asset metadata. Error types in `errors.rs` follow the [error handling principles](error-handling.md).
Incoming base-unit amounts are normalized at the asset scale before conversion to `Decimal`. Outgoing amounts must convert exactly to integer base units; sub-base-unit dust and unrepresentable precision are rejected, and deterministic amount failures are not retried.

### `daemon/src/chain_client/keyring.rs` — Keyring (Actor)
Actor pattern: mpsc channel + oneshot responses. Holds seed phrase (`Zeroize` + `ZeroizeOnDrop`). Handles both:
- **Asset Hub**: sr25519 key derivation via `subxt_signer`, hard derivation with `DeriveJunction`
- **Polygon**: secp256k1 key derivation via alloy `MnemonicBuilder`, BIP-44 path from hashed params

Client interface: `KeyringClient` (mockable via `mockall_double`).

### `daemon/src/chain/` — Chain Monitoring & Execution
- **`transfer_tracker.rs`** (`TransfersTracker`): Subscribes to finalized blocks per chain, detects incoming transfers, and notifies `TransactionsRecorder`. Failed subscriptions and streams that end before delivering an event use a cancellation-aware exponential retry delay (1–60 seconds). Retry state resets only after a stream delivers an event; degradation is reported on entry and at most once per minute, with recovery reported separately.
- **`transactions_recorder.rs`** (`TransactionsRecorder`): Records detected transactions to DB, updates `InvoiceRegistry`. Its transaction-scoped recording path lets Asset Hub balance reconciliation read the persisted received total and write a synthetic adjustment in one SQLite transaction; payout/refund/webhook/status side effects use that same path and the registry changes only after commit. Checked amount failures abort the database transaction and are surfaced with invoice/transaction coordinates; durable live-event replay remains a separate persistence concern.
- **`executor.rs`** (`TransfersExecutor`): Builds and submits payout transactions for both chains. Single executor instance handles Asset Hub + Polygon.
- **`invoice_registry.rs`** (`InvoiceRegistry`): In-memory tracking of active invoices and their expected amounts. Thread-safe (internal `RwLock`). Invoice-data refreshes update only records that are still present, preserving the received amount; they never reinsert an invoice removed concurrently after reaching a terminal status.

### `daemon/src/expiration_detector.rs` — ExpirationDetector
Periodic background task. Checks for expired invoices, handles cleanup and status transitions.

### `daemon/src/webhook_sender.rs` — WebhookSender
Periodic background task. Sends unsent webhook events from DB to configured URLs with HMAC signatures.

### `daemon/src/etherscan_client.rs` — EtherscanClient
Client for Etherscan/Polygonscan API. Used by ExpirationDetector for transaction verification on EVM chains. Transfer conversion is isolated per response item: unrepresentable items are logged with chain coordinates while valid items in the same batch continue through reconciliation.

### `daemon/src/types/` — Domain Types
Business logic models: `Invoice`, `Payout`, `Transaction`, `Refund`, `Swap`, `WebhookEvent`, `Changes`. Separate from DAO row types and API response types.

### `daemon/src/error.rs` — Legacy Error Types
Monolithic `Error` enum with `PrettyCause` trait. Being migrated to domain-specific errors (see `chain_client/errors.rs`). `thiserror` derive for all error types.

### `daemon/src/utils/` — Utilities
- `logger.rs` — tracing-subscriber setup, optional Loki integration (see [TLS](#tls))
- `logging.rs` — Structured log category/operation constants
- `amount.rs` — Exact checked conversion between token base units and `Decimal`
- `task_tracker.rs` — Wraps `tokio_util::task::TaskTracker` with error collection
- `shutdown.rs` — `ShutdownNotification`, `CancellationToken`, panic hook, signal handling

## TLS

**One TLS implementation, process-wide: rustls with the aws-lc backend.** No
OpenSSL, and nothing in the tree links the C `libssl`.

`async_try_main` installs the aws-lc provider as rustls' process-wide default in
its **first statement**, before `logger::initialize`. This ordering is
load-bearing, not stylistic. Log shipping is the only other TLS user in the
process — `tracing-loki` resolves reqwest 0.12, whose rustls feature set
compiles in the ring backend — and installing first is what keeps it on aws-lc.

Note what the ordering does *not* protect against: reqwest 0.12 never installs
a default of its own. It reads one via `CryptoProvider::get_default` and, when
the slot is empty, falls back to a locally built ring provider. Initialising
the logger first would therefore not fail loudly — it would silently leave two
crypto backends live in one process, Loki on ring and every payment on aws-lc.
Nothing else in the tree calls `install_default`, which is what lets the call
site `unwrap` its result. The comment there records the same thing.

**Trust store: the operating system's, everywhere.** Three HTTP stacks are in
the tree and they reach that answer differently:

| Consumer | Client | How roots are chosen |
|---|---|---|
| Money paths — Polygon, Pimlico, Etherscan, merchant webhooks | reqwest 0.13 | `rustls` feature → `rustls-platform-verifier` 0.7 → OS store |
| Money paths — Asset Hub | `subxt` → `jsonrpsee` 0.24 | `jsonrpsee-client-transport` → `rustls-platform-verifier` **0.5** → OS store |
| Log shipping — `tracing-loki` | reqwest 0.12 | `rustls-tls` → bundled webpki roots, **plus** `rustls-tls-native-roots` enabled explicitly in `daemon/Cargo.toml` → OS store as well |

Asset Hub does not go through reqwest at all: `subxt` talks WebSocket via
`jsonrpsee`, which carries its own `rustls-platform-verifier` at a different
major. That is why the crate appears twice in `cargo tree --duplicates`, and it
is upstream-bound — jsonrpsee still requires `rustls-platform-verifier ^0.5`
even at 0.26, so the subxt 0.44 → 0.50 upgrade will not collapse the pair.

reqwest 0.12's `rustls-tls` is hard-wired to the bundled `webpki-roots`, so
without that explicit feature Loki would be the one subsystem in the daemon
ignoring a CA the operator installed — silently, and only for logs. The two root
sources are additive, so the result is never fewer roots than an operator
expects. `tracing-loki` 0.2 exposes no native-roots variant of its own, which is
why the feature is enabled on reqwest directly.

`deny.toml` does not allow the `OpenSSL` license, which keeps the second stack
from returning through a future dependency's default features.

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
   - Asset Hub balance recovery re-reads the transaction sum inside the same transaction that writes its coordinate-less adjustment. A transfer committed after the chain balance fetch is therefore included before the delta is calculated, while a genuine positive shortfall is still recorded.
4. **TransfersExecutor** picks up payouts from DB → builds transaction → signs via Keyring → submits to chain → records result
5. **ExpirationDetector** periodically checks for expired invoices → updates status
6. **WebhookSender** periodically sends unsent webhook events to configured URLs

## Configuration System

Ten config types loaded at startup (all support env var overrides):

| Config | File | Key Fields |
|--------|------|------------|
| Chains | `chains.json` | Chain endpoints (optional — see below), assets |
| Payments | `payments.json` | Recipient addresses, invoice lifetime, default chain/asset |
| Secrets | `secrets.json` | BIP39 seed phrase, API secret key |
| Database | `database.json` | Database path, temporary mode, fail-closed existing-database requirement |
| Web Server | (defaults) | Host, port (default 0.0.0.0:16726) |
| Shop | `shop.json` | Webhook URL, shop metadata, signature max age |
| Logger | `logger.json` | Log level, Loki endpoint |
| Etherscan | `etherscan_client.json` | API key for Etherscan/Polygonscan |
| Auth | `auth.json` | OAuth client credentials, token lifetimes |
| Swaps | `swaps.json` | 0x API key and RPC URL (RPC optional — see below) |

**Public-default endpoints**: chain endpoints and the swaps 0x RPC URL are *not*
mandatory. Leaving either unset falls back to compiled-in free public providers
(`configs/consts.rs`), which carry no availability guarantee and are unsuitable
for production — a three-month unnoticed dependency on one of them was the
subject of [#333](https://github.com/Kalapaja/Kalatori/issues/333). Each
fallback emits a WARN at startup carrying `error.category = "config"`, so the
degraded state is greppable and alertable rather than silent. Note that chain
endpoints currently cannot be supplied by environment variable at all
([#338](https://github.com/Kalapaja/Kalatori/issues/338)); use `chains.json`.

**Env var pattern**: `{PREFIX}_{CONFIG}_{FIELD}` (e.g., `KALATORI_PAYMENTS_RECIPIENT`)
**Custom prefix**: `KALATORI_APP_ENV_PREFIX`
**Config directory**: `KALATORI_CONFIG_DIR_PATH`
**Security**: Seed phrase and API secret key are zeroized from env/memory after loading.

`invoice_lifetime_millis` is validated during configuration loading. Startup fails with an invalid-configuration error when the duration cannot produce a representable invoice expiry timestamp.

Example configs in `configs/` directory.

When configured, the shop webhook URL is validated at startup as an absolute
HTTP(S) URL with a host. Validation is structural only and performs no network
reachability check.

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
