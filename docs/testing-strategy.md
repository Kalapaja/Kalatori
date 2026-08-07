# Testing Strategy

## Test Types

### Unit Tests
- In-module `#[tokio::test]` tests using `mockall` for mocking
- Mock traits: `MockDaoInterface`, `MockDaoTransactionInterface` (from `daemon/src/dao/interface.rs`), `MockKeyringClient` (via `mockall_double`), `MockBlockChainClient`
- Example: `daemon/src/state.rs` tests module

### Integration Tests (Black-Box)
- Rust examples (`crud`, `webhook`) run against a live daemon + Chopsticks (Substrate chain fork simulator)
- `make run-test-examples` executes `cargo run --example crud; cargo run --example webhook`
- CI workflow: starts daemon + Chopsticks in background, then runs examples

### Mutation Testing
- `cargo-mutants` for evaluating test quality
- Runs against git diff to focus on changed code

## Commands

| Command | What |
|---|---|
| `make cargo-test` | Unit/integration tests via nextest |
| `make generate-coverage-report` | Coverage report as `lcov.info` (llvm-cov) |
| `make open-coverage-report` | Coverage report in browser (HTML) |
| `make cargo-mutants-for-diff` | Mutation testing on git diff |

**Prefer `make` targets over calling cargo directly.**

### Running Integration Tests

```bash
# Terminal 1: Start daemon with Chopsticks
make run

# Terminal 2: Run integration examples
make run-test-examples
```

This runs the Rust examples against the live daemon (default: `localhost:16726`).

## CI Pipeline

GitHub Actions with reusable workflow templates in `.github/workflows/`:

### PR to dev
`semantic-pr` → `fmt` → `clippy` → `cargo-deny` → `cargo-test-coverage` → `integration-test`

### PR to main
`release-validate` → `fmt` → `clippy` → `cargo-deny` → `cargo-test`

### Merge to dev
`docker-build` (pushes to dev GHCR package)

### Release (tag push)
`release-prepare` → `release-validate` → `docker-build` → `github-release`

Reusable job templates: `_job-cargo-check.yml`, `_job-cargo-test.yml`, `_job-cargo-test-coverage.yml`, `_job-clippy.yml`, `_job-fmt.yml`, `_job-cargo-deny.yml`, `_job-docker-build.yml`, `_job-integration-test.yml`, `_job-github-release.yml`, `_job-release-prepare.yml`, `_job-release-validate.yml`, `_job-semantic-pr.yml`

## Test Environment

- **Chopsticks**: Substrate chain fork simulator, configs in `chopsticks/`
- **Docker network**: `kalatori-network` (create with `docker network create kalatori-network`)
- **Start/stop**: `make start-chopsticks` / `make stop-chopsticks`

## Tool Versions

Pinned in `[workspace.metadata.bin]` in the root `Cargo.toml`, installed by
`make setup-utils`:
- nextest: 0.9.133
- llvm-cov: 0.8.4
- cargo-insta: 1.46.3
- cargo-mutants: 26.2.0

Dependabot does not watch these — `[workspace.metadata.bin]` is a metadata
table, not a dependency section — so they only move when someone checks.

**These pins cannot currently be changed.** CI caches `.bin/` under
`bins-${{ hashFiles('Cargo.toml') }}`, so editing the root `Cargo.toml` at all
forces a cache miss and a real `make setup-utils`. That install then fails:
subxt ships no release binaries, so `cargo binstall` falls back to
`cargo install`, which rejects the `--install-path` flag `cargo-run-bin`
passes. The cache is the only reason this works today. See
[#390](https://github.com/Kalapaja/Kalatori/issues/390).

Install all: `make setup-utils`
