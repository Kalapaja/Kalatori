# Kalatori Changelog

All notable changes to this project will be documented in this file.
**Please note:**
This is a public beta release of the Kalatori daemon. While it adheres to the [API specs](https://kalapaja.github.io/kalatori-api), it is still under active development. We encourage you to test it and provide feedback.

## [0.8.2] - 2026-03-13

### 🚀 Features

- Cross-chain swaps via Across and Bungee protocols
- Native asset support for cross-chain swaps (POL on Polygon not yet available)
- Dev API handler for inspecting live invoice registry state
- Balance recovery on invoice expiration via Etherscan/Polygonscan API
- Basic RPC endpoint rotation for failover
- Configurable Ankr API token for multichain balance queries
- Webhook simulator developer tool (`tools/webhook-simulator/`)
- HMAC test vector generator (`client/examples/generate_hmac_test_vectors.rs`)

### 🔧 CI/CD

- Restructured pipeline into 12 reusable `_job-*.yml` workflow templates
- Tag-triggered release workflow with changelog generation
- Codecov integration for test coverage tracking
- Separate GHCR package for dev builds (`kalatori-dev:latest`)
- GitHub Actions updated to latest versions

### 🐳 Docker

- Non-root container user (`kalatori:1000`)
- Optimized layer caching (dependencies built separately from source)
- SQLite build cached between runs

### 🧪 Testing

- llvm-cov for local coverage reports
- cargo-mutants targets for mutation testing on diffs
- Increased test coverage for critical functionality

### 📖 Documentation

- AGENTS.md — comprehensive AI agent guide
- docs/architecture.md, conventions.md, error-handling.md, testing-strategy.md
- docs/mcp-tooling.md, doc-update-triggers.md
- docs/DATABASE.md updated with swap schema

### 🎨 Frontend (Kassette v0.0.15)

- Angular 21 SPA with signal-based state management
- Internationalization (English and Spanish)
- Responsive layouts (mobile, tablet, desktop)
- Payment persistence with speed-up (replace-by-fee) option
- Partial payment top-up support
- Backend swap orchestration
- Chain configs fetched from API at runtime
- Native token payments on all chains except Polygon

## [0.8.1] - 2025-02-14

Kalatori v0.8.1 is a ground-up rewrite of the payment gateway daemon. The core payment flow remains the same — merchants create invoices, customers pay, funds auto-withdraw to merchant wallets — but the entire codebase, infrastructure, and feature set have been rebuilt for production readiness.

## Architecture Rewrite

The project has been restructured from a single crate into a **Rust workspace**:

- **`daemon/`** — Main binary crate (kalatori)
- **`client/`** — Public client library (kalatori-client) with HMAC utilities, Axum middleware, and example integrations

All old source code (`src/`) has been replaced by structured modules under `daemon/src/`:

- `state.rs` — `AppState` (Arc-wrapped, direct async methods replacing the old mpsc actor)
- `api/` — Axum 0.8 server with 4 route groups: `/public`, `/private`, `/internal`, `/dev`
- `chain/` — `TransfersTracker`, `TransfersExecutor`
- `chain_client/` — `BlockChainClient` trait with implementations per chain
- `chain_client/keyring.rs` — Actor-pattern seed management (sr25519 + secp256k1)
- `dao/` — SQLite DAO with `DaoInterface` trait, per-entity CRUD modules
- `configs/` — JSON config loading + env var overrides
- `types/` — Domain types: Invoice, Payout, Transaction, Refund, WebhookEvent

## Polygon (EVM) Chain Support

Kalatori now supports **Polygon PoS** alongside Polkadot Asset Hub, enabling merchants to accept payments on both Substrate and EVM chains from a single daemon instance.

- **ERC-20 payments** (USDC on Polygon) with real-time WebSocket monitoring
- **Gasless transactions** via Pimlico paymaster (ERC-4337 account abstraction) — outgoing transfers pay gas in USDC, no native POL needed
- **Unified key derivation** — same BIP39 seed phrase derives both sr25519 (Asset Hub) and secp256k1 (Polygon) keypairs

## Database: Sled → SQLite

Migrated from **Sled** (embedded key-value store) to **SQLite** (via sqlx 0.8):

- Compile-time SQL verification
- Schema migrations in `./migrations/`
- Status transitions enforced via CHECK constraints and triggers
- Trait-based DAO (`DaoInterface` + `DaoTransactionInterface`) — mockable for testing
- Per-entity modules: invoice, transaction, payout, refund, webhook_event, changes
- Requires SQLite >= 3.47.0 at runtime

## API Redesign (V2)

Completely redesigned API with four route groups:

- **`/public`** — Customer-facing: invoice lookup, shop metadata
- **`/private`** — Merchant operations: invoice CRUD, payment configuration
- **`/internal`** — Inter-service: changes polling, state synchronization
- **`/dev`** — Debug endpoints (disabled in production)

All authenticated endpoints use **HMAC-SHA256 request signing**. Responses use structured `result`/`error` format.

## Webhooks

Invoice lifecycle events delivered to merchant webhook endpoints:

- Events: invoice created, paid, expired, partially paid, canceled
- **HMAC-SHA256 signing** for authenticity verification
- Automatic retry with status tracking
- Client library includes verification utilities

## Client Library

New `kalatori-client` crate for merchant integrations:

- HTTP client for all daemon API endpoints
- HMAC request signing utilities
- Axum middleware for webhook verification
- Example integrations: CRUD operations, webhook handling

## Payment UI (Kassette)

The daemon ships with an embedded payment frontend ([Kassette](https://github.com/Kalapaja/Kassette)), served as static files with merchant branding injected at runtime (`%VITE_MERCHANT_NAME%`, `%VITE_MERCHANT_LOGO_URL%`, etc.).

## Configuration Overhaul

Replaced TOML config files with **JSON configs** + **environment variable overrides**:

| Config | Purpose |
| --- | --- |
| `chains.json` | Chain endpoints, asset metadata |
| `payments.json` | Recipient addresses, payment URL base |
| `secrets.json` | Seed phrase |
| `shop.json` | Merchant name, logo, Reown project ID |
| `logger.json` | Log level directives |

Override any field via environment: `KALATORI_PAYMENTS_PAYMENT_URL_BASE=https://...`

Env vars are removed from the process after loading (security).

## Slippage Configuration

Added configurable slippage tolerance for underpayment/overpayment cases, allowing merchants to define acceptable payment variance thresholds.

## Code Quality

- Rust edition 2024, MSRV 1.88
- Strict Clippy (`-D warnings` including pedantic lints)
- Nightly rustfmt
- cargo-deny for license and security auditing
- Domain-specific error types (migrating from legacy monolithic enum)
- `Makefile` with standardized build/check/run targets

## Breaking Changes

- **Configuration format**: TOML configs replaced by JSON — existing configs must be migrated
- **Database**: Sled replaced by SQLite — no automatic data migration from previous installations
- **API**: Entirely new endpoint structure — V1 API no longer available
- **Build**: Now a Rust workspace — build with `make build-release` or target `daemon/` specifically
- **Dependencies**: Requires SQLite >= 3.47.0, Rust >= 1.88

## [0.4.1] - 2025-09-26

### 🐛 Bug Fixes

- Base64ct bumbed MSRV in minor update, pin older version to avoid compability issues
- Increase delay in integration test to 40 seconds cause test fails on CI and can not be reproduced locally
- Add rustfmt and clippy components installation to respective jobs
- Daemon wasn't able to connect to nodes because of lack of certificates. Added ca-certificates installation to the Dockerfile. Also added some logs around Chain Tracker connection

### 🚜 Refactor

- Remove unused commented consts from database.rs

### ⚙️ Miscellaneous Tasks

- Remove  option from semantic PR check job. This option require PR write permission which caused pipeline failures
- Bump semantic PR action version to 6
- Change semantic trigger from pull_request_target to pull_request, updated workflows formatting
- Bump version to 0.4.1

### Fix

- Upgraded version of  Encode macro generates warning. Applied temporary fix for that


## [0.4] - 2025-09-14
Metadata v16 support

## [0.3] - 2024-11-28

This is a public beta release of the Kalatori daemon. While it adheres to the [API specs](https://kalapaja.github.io/kalatori-api), it is still under active development. We encourage you to test it and provide feedback.


## [0.2.8] - 2024-11-13

### 🚀 Features

- Order transaction storage implementation.

## [0.2.7] - 2024-11-18

### 🚀 Features

- Asset Hub transactions with fee currency
- Autofill tip with asset
- Pass asset id into transaction constructor to properly select fee currency

### 🧪 Testing

- Test cases to cover partial withdrawal and Asset Gub transfers

## [0.2.6] - 2024-11-01

### 🚀 Features

- Force withdrawal call implementation
- Docker container for the app
- Containerized test environment

### 🐛 Bug Fixes

- Fixed the storage fetching.
- Removed redundant name checks & thereby fixed the connection to Asset Hub chains.

## [0.2.5] - 2024-10-29

### 🚀 Features

- Callback in case callback url provided

### 🐛 Bug Fixes

- fix error handling as a result of dep uupgrade
- fix order withdraw transaction
- mark order withdrawn on successful withdraw

## [0.2.4] - 2024-10-21

### ⚡ Performance

- Switched from the unmaintained `hex` crate to `const-hex`.

### 🚜 Refactor

- Moved all utility modules under the utils module.
- Removed all `mod.rs` files & added a lint rule to prevent them.

## [0.2.3] - 2024-10-15

### 🚀 Features

- Server health call implementation

## [0.2.2] - 2024-10-10

### 🚀 Features

- Docker environment for chopsticks and compose to spawn 4 chopsticks instances in parallel looking at different RPCs

### 🐛 Bug Fixes

- Server_status API request returns instance_id instead of placeholder
- Mark_paid function will mark order correctly now

## [0.2.1] - 2024-10-07

Making the order request work according to specs in the [specs](https://kalapaja.github.io/kalatori-api/#/).
Using the tests from [kalatori-api-test-suite]() in order to validate.
Added git cliff and configuration for it to generate CHANGELOG like this one, see [CONTRIBUTING.md](CONTRIBUTING.md)

### 🐛 Bug Fixes

- API specs Balances->Native
- Not having currency in the request responds with Fatal
- Validate missing order parameters
- Get order handler functionality part
- Get order and update order
- Removed version check for PRs

### ⚙️ Miscellaneous Tasks

- Resolve conflicts
