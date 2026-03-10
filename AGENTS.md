# Kalatori — AI Agent Guide

Self-hosted, non-custodial blockchain payment gateway daemon for Polkadot Asset Hub and Polygon. Derives unique HD payment accounts per invoice, monitors chains for incoming payments, auto-withdraws to merchant recipient. License: GPLv3. **Rust edition 2024, MSRV 1.88.** Status: Public Beta.

## Critical Pitfalls

- **Metadata regen required** when updating subxt or connecting to new chain version: `make install-subxt-cli && make download-node-metadata-ci`
- **Version sync**: subxt-cli must match subxt in Cargo.toml (both 0.44), sqlx-cli must match sqlx (both 0.8) — versions pinned in `Makefile`
- **Clippy strict**: CI runs `RUSTFLAGS="-Dwarnings"` — all warnings are errors, including pedantic lints
- **Never use `mod.rs`** — self-named modules only (enforced by `mod_module_files` clippy lint)
- **Nightly rustfmt**: `cargo +nightly fmt --all`
- **SQLite >= 3.47.0** required at runtime (see README.md for build-from-source instructions)
- **Seed security**: Zeroized on drop, env vars removed after loading, never log private keys
- **Legacy vs new errors**: `daemon/src/error.rs` is the legacy monolithic enum; new code should use domain-specific error types per [docs/error-handling.md](docs/error-handling.md)

## How We Work

### Documentation Policy
- **Done something — write it down.** Every architectural decision, troubleshooting finding, or pattern change gets recorded in `docs/`.
- When a doc contradicts code, ask the user which is correct and update the other.
- When a new subsystem emerges, create `docs/<name>.md` and add it to the Documentation Map below.
- See [docs/doc-update-triggers.md](docs/doc-update-triggers.md) for the mandatory update checklist.

### Research Policy
- **Never assume library APIs from memory** — look up in Context7 or Exa first.

### Trust Hierarchy
When sources disagree, trust in this order:
1. **Code, build files, CI workflows** — canonical source of truth
2. **Tests** — verify behavior claims
3. **Specific docs** (e.g., `docs/error-handling.md`) override general docs (e.g., `AGENTS.md`)
4. **Docs describe intent and patterns** — not guaranteed implementation truth
5. When unsure, **ask the user** which is correct and update the other

### Editing Strategy
- Prefer minimal, surgical edits. Don't refactor adjacent code opportunistically.
- Preserve local module conventions even if globally suboptimal — unless explicitly refactoring.
- When modifying legacy modules (e.g., code using `daemon/src/error.rs`), follow existing local patterns. Only introduce new error architecture at clear boundaries.
- Add or update tests when behavior changes.
- Check [docs/doc-update-triggers.md](docs/doc-update-triggers.md) after changes.

### Writing Style
- Context-aware, terse, informative, concise.
- No unnecessary abstractions — three similar lines > premature helper.

## Tech Stack

- **Workspace**: `daemon` (main binary), `client` (public client library)
- **Runtime**: tokio (multi-threaded)
- **Web**: axum 0.8
- **Database**: SQLite via sqlx 0.8 (migrations in `./migrations/`)
- **Chains**: Asset Hub (subxt 0.44, sr25519) + Polygon (alloy 1.5, secp256k1, Pimlico paymaster)
- **Key derivation**: BIP39 seed → Keyring actor (mpsc channel) → per-chain derivation
- **Config**: JSON files + env var overrides (`{PREFIX}_{CONFIG}_{FIELD}`)
- **Testing**: nextest, llvm-cov, cargo-mutants, Rust example-based integration tests
- **CI**: GitHub Actions with reusable `_job-*.yml` workflow templates

## Repository Layout

```
daemon/src/
  main.rs                     Entry point, component initialization
  state.rs                    AppState struct (Arc, direct async methods)
  api.rs + api/               Axum server: /public, /private, /internal, /dev
  chain.rs + chain/           TransfersTracker, TransfersExecutor, InvoiceRegistry, TransactionsRecorder
  chain_client.rs + chain_client/  BlockChainClient trait, asset_hub.rs, polygon.rs, keyring.rs, errors.rs
  dao.rs + dao/               SQLite DAO: DaoInterface trait, per-entity CRUD modules
  types.rs + types/           Domain types: Invoice, Payout, Transaction, Refund, Swap, etc.
  configs.rs + configs/       JSON config loading + env var overrides
  error.rs                    Legacy error types (being migrated)
  expiration_detector.rs      Periodic invoice expiration handling
  webhook_sender.rs           Periodic webhook dispatch with HMAC
  etherscan_client.rs         Etherscan/Polygonscan API client
  utils/                      logger, logging constants, task_tracker, shutdown

client/src/                   Public client library: types, HTTP client, HMAC utils, Axum middleware
configs/                      Example JSON config files
migrations/                   SQLite migration SQL files
chopsticks/                   Chopsticks (Substrate fork simulator) Docker setup
daemon/examples/              Integration test examples (crud, webhook)
```

## Essential Commands

| Command | What |
|---|---|
| `make setup` | Install subxt-cli + download metadata + copy configs |
| `make build-release` | Build release binary |
| `make run` | Start chopsticks + build and run daemon |
| `make run-release` | Run daemon in release mode (real chain) |
| `make cargo-check` | Compilation check (all targets + features) |
| `make cargo-clippy` | Lint check (strict: `-D warnings`) |
| `make cargo-fmt` | Format check (nightly) |
| `make cargo-deny` | Dependency license/security check |
| `make cargo-test` | Run tests via nextest |
| `make generate-coverage-report` | Test coverage as lcov.info |
| `make help` | Show all available targets |

**Prefer `make` targets over calling cargo directly.**

## Documentation Map

| Doc | Covers | When to consult |
|---|---|---|
| [docs/conventions.md](docs/conventions.md) | Code style, lints, logging, security, dependency rules, DAO conventions | Writing any Rust code |
| [docs/error-handling.md](docs/error-handling.md) | 5 Error Design Principles with examples | Designing error types |
| [docs/architecture.md](docs/architecture.md) | Component map, data flow, payment lifecycle, config, key derivation | Understanding the system |
| [docs/testing-strategy.md](docs/testing-strategy.md) | Test types, commands, CI pipeline, mock patterns | Adding/modifying tests |
| [docs/mcp-tooling.md](docs/mcp-tooling.md) | MCP server availability and usage patterns | Using MCP tools |
| [docs/doc-update-triggers.md](docs/doc-update-triggers.md) | What docs to update after code changes | After any PR |
| [docs/DATABASE.md](docs/DATABASE.md) | SQLite schema, DAO pattern, status transitions | DB schema changes |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Prerequisites, dev setup, release process | Releasing, onboarding |
| [README.md](README.md) | User-facing setup, compilation, usage | End-user documentation |

## MCP Tooling Summary

| Tool | Use for |
|---|---|
| **Serena** | Code navigation/editing: `find_symbol`, `get_symbols_overview`, `replace_symbol_body` |
| **Ripgrep** | Fast code search (preferred over grep via Bash) |
| **Context7** | Library docs (subxt, axum, sqlx, alloy). Always check before assuming APIs |
| **Exa** | Online search, code samples, best practices |
| **mcp-server-git** | Git read ops. Bash for complex git |
| **Playwright** | Browser automation for local testing |
| **Sequential Thinking** | Step-by-step reasoning for complex problems |

Details: [docs/mcp-tooling.md](docs/mcp-tooling.md)

## Key Links

- [V2 API spec](https://github.com/Kalapaja/kalatori-api/blob/master/kalatori.yaml)
- [Kalatori Matrix](https://matrix.to/#/#Kalatori-support:matrix.zymologia.fi)
- [GitHub Discussions](https://github.com/Kalapaja/Kalatori/discussions)
- [Roadmap](https://github.com/orgs/Kalapaja/projects/2) and [Milestones](https://github.com/Kalapaja/Kalatori/milestones)
