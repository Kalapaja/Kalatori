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

Hybrid setup: **Dagger** (TypeScript module in `ci/`) runs build/check/test logic, **GitHub Actions** handles orchestration, secrets, and GitHub-native integrations.

### Dagger checks (run via `dagger call <command>`)
`check-fmt`, `check-clippy`, `check-deny`, `check-machete` run in parallel. Tests via `test-unit`, `test-unit-coverage`, `test-integration`. See [docs/dagger-migration-plan.md](dagger-migration-plan.md) for full details.

### GHA orchestration

#### PR to dev/main
`semantic-pr` → matrix of Dagger checks (fmt, clippy, deny, machete, tests, integration)

#### Merge to dev
`docker-build` (pushes to dev GHCR package)

#### Release (tag push)
`release-prepare` → `release-validate` → `docker-build` → `github-release`

GHA-only reusable templates: `_job-semantic-pr.yml`, `_job-release-validate.yml`, `_job-release-prepare.yml`, `_job-github-release.yml`

Legacy templates (being replaced by Dagger): `_job-cargo-test.yml`, `_job-cargo-test-coverage.yml`, `_job-clippy.yml`, `_job-fmt.yml`, `_job-cargo-deny.yml`, `_job-docker-build.yml`, `_job-integration-test.yml`

## Test Environment

- **Chopsticks**: Substrate chain fork simulator, configs in `chopsticks/`
- **Docker network**: `kalatori-network` (create with `docker network create kalatori-network`)
- **Start/stop**: `make start-chopsticks` / `make stop-chopsticks`

## Tool Versions

Pinned in `Makefile` and `ci/src/versions.ts`:
- nextest: 0.9.129
- llvm-cov: 0.8.4
- cargo-mutants: 26.2.0

Install all: `make setup-utils`
