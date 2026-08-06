# Coding Conventions

## Code Style

- **Rust edition 2024**, MSRV 1.91 (`rust-version` in root `Cargo.toml`
  `[workspace.package]`; every member inherits it)
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

There are **no** per-crate lint tables — `daemon`, `client` and `tools/cargo-bin`
all carry only `[lints] workspace = true`, so the root block above plus the panic
gate below is the complete set. In particular `pedantic` and
`arithmetic_side_effects` are *not* enabled anywhere.

## Panic Gate

`set_panic_hook` (`daemon/src/utils/shutdown.rs`) cancels the shutdown token on
**any** panic, in any thread, and the hook runs *before* unwinding — so nothing
downstream can contain it. On the unauthenticated `/public` routes that makes a
reachable panic a remote kill switch for the payment gateway.
[Issue #349](https://github.com/Kalapaja/Kalatori/issues/349) was exactly this,
via a `split_once("|").unwrap()` on a payer-supplied signature.

These restriction lints are denied workspace-wide:

```toml
[workspace.lints.clippy]
expect_used      = "deny"
indexing_slicing = "deny"
panic            = "deny"
string_slice     = "deny"
todo             = "deny"
unimplemented    = "deny"
unreachable      = "deny"
unwrap_used      = "deny"
```

### What the gate does and does not cover

It covers the listed constructs **in first-party code**. It is not a proof that
the daemon cannot panic. Still uncaught, and still fatal:

- `assert!`, `assert_eq!`, `debug_assert!` — including production assertions.
- **Integer and `Decimal` arithmetic, division, shifts, datetime arithmetic.**
  `clippy::arithmetic_side_effects` is not enabled. See the backlog below.
- Panicking library APIs — `B256::from_slice`, `Vec::remove`, `chrono`
  constructors, and anything else that panics on out-of-domain input.
- `std::process::abort`/`exit`, and allocation failure.
- Panics inside dependencies on data we hand them.

Treat the gate as removing the *careless* panics, not as a guarantee.

### Test code

`clippy.toml` sets `allow-unwrap-in-tests`, `allow-expect-in-tests`,
`allow-panic-in-tests` and `allow-indexing-slicing-in-tests`. These cover the
whole `#[cfg(test)]` module — helper functions included, and `#[tokio::test]`
behaves like `#[test]`.

**Clippy has no equivalent option for `unreachable`, `todo`, `unimplemented` or
`string_slice`.** Those four fire inside test modules too, and need a local
`#[expect]` there.

Example binaries under `client/examples/` get no test exemption and carry
file-level `#![expect(clippy::unwrap_used)]`. That is a blunt instrument — it
also exempts every unwrap added to those files later. Prefer returning `Result`
from new examples.

### Satisfying the gate

In order of preference:

1. **Return a typed error.** Almost always right for anything a request, a
   database row, or a network peer can influence. See
   [error-handling.md](error-handling.md).
2. **Degrade gracefully** where there is no error channel — but only when the
   degraded path is *correct*, not merely non-panicking. A webhook that gets
   retried beats a daemon that is gone; an API server that silently stops
   serving does not. If the condition is genuinely fatal, cancel the shutdown
   token rather than swallowing it.
3. **Keep the panic and justify it** — only when the panicking case is provably
   impossible, or is a startup failure where refusing to run is correct:

   ```rust
   #[expect(
       clippy::indexing_slicing,
       reason = "SHA-256 always finalizes to 32 bytes, so the first 8 are always present"
   )]
   ```

   The workspace denies `allow_attributes`, so it must be `#[expect]`, not
   `#[allow]` — an expectation that stops firing becomes a warning, and under
   CI's `-Dwarnings` an error, which keeps annotations from going stale.

   **The reason must state why the panicking case cannot happen**, and must cite
   something actually enforced. "Should never fail" is not a reason.
   "Startup validation populates an entry for every `ChainType`" is — provided
   that validation really does. A reason that asserts an unenforced invariant is
   worse than no annotation, because it manufactures confidence.

### Backlog

- **Grandfathered sites.** Panics predating the gate that were not audited carry
  a marker reason. These deliberately do **not** satisfy the "state why it cannot
  happen" rule above — the marker records that the site is unaudited, and some
  are known to be reachable. It is a backlog entry, not a justification, and
  must not be extended to new code. Find them with:

  ```bash
  grep -rn "grandfathered when the panic gate landed" daemon/src client/src
  ```

  These are *not* cleared — some are known-reachable. Prefer converting one to a
  typed error over extending the marker to new code.

- **Arithmetic is not gated.** `clippy::arithmetic_side_effects` would catch
  overflow, division by zero and `Decimal` panics; roughly two dozen daemon
  sites trip it today. Division by zero panics unconditionally; overflow
  currently *wraps silently* in release, which for a daemon computing payment
  amounts is arguably worse than panicking.

- **`[profile.release]` is in `daemon/Cargo.toml`, a workspace member, so Cargo
  ignores it** — it prints `profiles for the non root package will be ignored`
  on every build. `panic = "abort"`, `overflow-checks`, `lto`, `strip` and
  `codegen-units` are therefore *not* in effect. Moving the block to the
  workspace root should be sequenced **after** the arithmetic gate above:
  enabling `overflow-checks` while two dozen unguarded sites remain would turn
  silent wrapping into live panics, i.e. create the very DoS class this gate
  exists to close.

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
