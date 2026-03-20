# Coding Conventions

## Code Style

- **Rust edition 2024**, MSRV 1.93
- **rustfmt**: Nightly required (`cargo +nightly fmt --all`)
- Self-named modules only (e.g., `chain.rs` + `chain/` directory) — **never `mod.rs`** (enforced by `mod_module_files` clippy lint). Rationale: better Git history, avoids file renaming issues.

## Clippy Lints

Workspace lints in root `Cargo.toml`:

```toml
[workspace.lints.clippy]
allow_attributes = "deny"
cargo_common_metadata = "warn"
cast_possible_truncation = "warn"
ignored_unit_patterns = "warn"
mod_module_files = "warn"
```

**CI enforces** `RUSTFLAGS="-Dwarnings"` — all warnings are errors, including pedantic.

Per-crate lints (in `daemon/Cargo.toml`):
- `pedantic = { level = "warn", priority = -1 }`
- `arithmetic_side_effects = "warn"`
- `shadow_reuse`, `shadow_same`, `shadow_unrelated` = "warn"

## Logging

Uses `tracing` with `tracing-subscriber` and env-filter.

**Log levels:**

| Level | When to Use | Example |
|-------|-------------|---------|
| DEBUG | Error conversions, expected failures | Balance fetch for new account |
| INFO | Significant business events | "Payout completed", "Invoice paid" |
| WARN | Recoverable errors, degraded state | "RPC endpoint degraded" |
| ERROR | Critical failures requiring attention | "All RPC endpoints down" |

**Structured fields**: Use `error.category`, `error.operation`, `error.source` from constants in `daemon/src/utils/logging.rs`:

```rust
tracing::debug!(
    error.category = category::CHAIN_CLIENT,
    error.operation = operation::FETCH_BALANCE,
    error.source = ?e,
    "Balance fetch failed"
);
```

**The Layer Rule** — log at conversion boundary (Layer 3), skip intermediates (Layer 2), log business error at handler (Layer 1). Full details and examples: [docs/error-handling.md](error-handling.md) (Principle 2).

**Production config**: `RUST_LOG=info,kalatori::chain_client=debug`

## Security

- Seed phrase: `Zeroize` + `ZeroizeOnDrop` on `Keyring` struct, env vars removed after loading
- Never log private keys or seed phrases
- Keyring actor (`daemon/src/chain_client/keyring.rs`) isolates all cryptographic operations via mpsc channel
- API responses must never expose secrets — see [docs/error-handling.md](error-handling.md) (Principle 5)
- HMAC signing for webhook authenticity (`kalatori_client::utils::HmacConfig`)

## Dependency Management

- **subxt** and **subxt-cli** versions must match (pinned in `Makefile` as `subxt_cli_version`)
- **sqlx** and **sqlx-cli** versions must match (pinned in `Makefile` as `sqlx_cli_version`)
- **reqwest** version synced between daemon and client crates
- `cargo deny` checks licenses and security advisories (`make cargo-deny`)
- When updating subxt: reinstall CLI (`make install-subxt-cli`), regenerate metadata (`make download-node-metadata-ci`), rebuild

## Error Handling Quick Reference

Five principles guide error type design (full details: [docs/error-handling.md](error-handling.md)):

1. **Only enumerate errors requiring different handling** — don't create variants that differ only in log messages
2. **Log raw errors at the conversion point** — preserve library error details before transformation
3. **Include useful and required info only** — pass the "actionability test" for each error field
4. **Separate error enums for different domains** — split by caller usage context, not technical category
5. **Internal errors shouldn't leak to API** — use `ApiErrorExt` trait for public error representation

New code should use domain-specific error types (see `daemon/src/chain_client/errors.rs`), not the legacy monolithic `Error` enum in `daemon/src/error.rs`.

## DAO Conventions

- Keep methods focused on single responsibilities (create, read, update). No business logic in DAO.
- All creation and update methods return the full updated object.
- Manually update `updated_at` and increment `version` in UPDATE statements (no database triggers for this).
- Convert `chrono::DateTime<Utc>` to `NaiveDateTime` when binding SQL parameters for comparison compatibility.
- Use `sqlx prepare` for compile-time SQL verification (`make sqlx-prepare`).
