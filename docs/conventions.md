# Coding Conventions

## Code Style

- **Rust edition 2024**, MSRV 1.91 (`rust-version` in root `Cargo.toml`
  `[workspace.package]`; every member inherits it)
- **rustfmt**: Nightly required (`cargo +nightly fmt --all`)
- Self-named modules only (e.g., `chain.rs` + `chain/` directory) — **never `mod.rs`** (enforced by `mod_module_files` clippy lint). Rationale: better Git history, avoids file renaming issues.

## Lints

The question people arrive with is *"will CI catch X?"*, so the gate is
described in the four parts that answer it.

**1. Explicit clippy lints** — `[workspace.lints.clippy]` in the root
`Cargo.toml`:

```toml
allow_attributes = "deny"
cargo_common_metadata = "warn"
cast_possible_truncation = "warn"
ignored_unit_patterns = "warn"
mod_module_files = "warn"
```

Plus the eight panic-gate denials listed under [Panic Gate](#panic-gate) below,
which live in the same table.

**2. Explicit rustc lint groups** — `[workspace.lints.rust]`, in the same file
and easy to miss:

```toml
future_incompatible = "warn"
let_underscore = "warn"
rust_2018_idioms = "warn"
unused = "warn"
```

Two of these matter beyond the compiler's own defaults: `rust_2018_idioms` and
`let_underscore` are groups containing allow-by-default lints, so they add
enforcement rather than restating it.

**3. Compiler and clippy defaults** — everything warn-by-default in `rustc` and
`clippy`, always on.

**4. Command-line escalation** — `RUSTFLAGS="-Dwarnings"`. This is narrower
than it sounds, so it is spelled out in the next section.

Both tables reach every crate through `[lints] workspace = true`, which
`daemon`, `client` and `tools/cargo-bin` all carry. That inheritance is the
load-bearing mechanism and it is invisible: a crate that omits the two lines
silently opts out of *everything* above, and nothing enforces that it does not.
There are **no** per-crate lint tables, so buckets 1–3 are the whole set.

### Where `-Dwarnings` actually applies

`RUSTFLAGS="-Dwarnings"` is set in exactly one place: the `cargo-clippy` recipe
in the `Makefile`, invoked by the PR **clippy job**. It lints all targets and
all features across the workspace, and anything an enabled lint reports there
fails that job.

The build, test, coverage, integration-test and release jobs do **not** set it,
so a warning that only surfaces during a `cargo build` fails nothing.
Dependencies are capped regardless, so nothing in the dependency graph can fail
the build whatever the flag says. Nothing in this repo configures that — Cargo
passes `--cap-lints warn` to every non-path dependency on its own, which is why
`RUSTFLAGS` reaching the whole graph is harmless.

Note also that `-Dwarnings` escalates lints that are *already enabled*. It
cannot turn on a lint that is off — which is why the list above, not the flag,
determines what protects you.

### What is not enabled, and why the docs used to say otherwise

`pedantic`, `arithmetic_side_effects` and the `shadow_*` family are **off**.

They were not declined after deliberation — they were configured and then
removed, and the documentation simply went stale:

- `pedantic` and `arithmetic_side_effects` were removed on 2026-01-16 in
  `ed941bc`, titled *"style: fix clippy and rustfmt warnings"* — a commit that
  silenced the warnings by config rather than fixing them.
- The `shadow_*` family went the same way in `abcaf13` (2025-12-02).

Turning them back on today produces **471 warnings** across the workspace,
deduplicated by site — so adopting them is a cleanup project, not a config
change. The bulk is shadowing (149 `shadow_unrelated`, 52 `shadow_reuse`), then
45 unseparated long literals, 32 unchecked arithmetic operations, 25
over-long functions, 23 missing doc backticks and 19 inlinable `format!` args.

Re-measure with:

```bash
cargo clippy --all-targets --all-features -- \
  -W clippy::pedantic \
  -W clippy::arithmetic_side_effects \
  -W clippy::shadow_reuse -W clippy::shadow_same -W clippy::shadow_unrelated
```

The `--` separator is required: lint flags belong to `clippy-driver`, and Cargo
rejects them without it. Run `make setup` first — `metadata.scale` and
`static/index.html` are generated inputs, absent from a clean checkout, and the
daemon aborts before finishing the lint pass without them ([#339](https://github.com/Kalapaja/Kalatori/issues/339)).

### `#[expect]` on a lint nothing enables

`#[expect(clippy::too_many_lines)]` and friends sit in the tree without tripping
`unfulfilled_lint_expectations`, even though `pedantic` is off. Of the 122
clippy expectations in first-party code, **28 name a lint nothing enables**,
across six of them: `arithmetic_side_effects` (13), `too_many_lines` and
`cast_sign_loss` (4 each), `struct_field_names` and `module_name_repetitions`
(3 each), and `unused_self`.

That works because `#[expect]` is a *scoped lint level* ([RFC 2383](https://rust-lang.github.io/rfcs/2383-lint-reasons.html)):
it raises the lint's level for its scope, so the lint runs there even though
nothing enables it globally, and the genuine violation fulfils the expectation.
Clippy does not evaluate disabled lints and discard the output — many never run
at all. Adding an expectation where the code does *not* violate the lint still
warns.

So these annotations mark real violations of lints nothing enforces. Useful as
documentation; not evidence that anything is checking.

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
  overflow, division by zero and `Decimal` panics; 32 sites trip it today (see
  the measurement above), plus 13 already carrying an `#[expect]`. Division by
  zero panics unconditionally; overflow currently *wraps silently* in release,
  which for a daemon computing payment amounts is arguably worse than panicking.

- **`[profile.release]` is in `daemon/Cargo.toml`, a workspace member, so Cargo
  ignores it** — it prints `profiles for the non root package will be ignored`
  on every build. `panic = "abort"`, `overflow-checks`, `lto`, `strip` and
  `codegen-units` are therefore *not* in effect. Moving the block to the
  workspace root should be sequenced **after** the arithmetic gate above:
  enabling `overflow-checks` while those sites remain unguarded would turn
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

- CLI tool versions are pinned in `[workspace.metadata.bin]` in the root
  `Cargo.toml` and installed by `make setup-utils`, which runs
  `cargo bin --install`. They are *not* Makefile variables, and Dependabot
  cannot see them — a metadata table is not a dependency section, so they
  drift silently and need checking by hand. Note they also cannot currently be
  *changed* without breaking CI; see
  [testing-strategy.md](testing-strategy.md) and
  [#390](https://github.com/Kalapaja/Kalatori/issues/390).
- **subxt** and **subxt-cli** versions must match. Note `subxt-cli` is pinned
  in **two** places: `[workspace.metadata.bin]` and `Dockerfile`. Update both.
- **sqlx** and **sqlx-cli** versions must match (`[workspace.metadata.bin]`)
- **reqwest** version synced between daemon and client crates
- `cargo deny` checks licenses and security advisories (`make cargo-deny`)
- When updating subxt: reinstall the CLI (`make setup-utils`), regenerate
  metadata (`make download-node-metadata`), rebuild

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
