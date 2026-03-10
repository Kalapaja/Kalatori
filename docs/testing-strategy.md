# Testing Strategy

## Test Types

### Unit Tests
- In-module `#[tokio::test]` tests using `mockall` for mocking
- Mock traits: `MockDaoInterface`, `MockDaoTransactionInterface` (from `daemon/src/dao/interface.rs`), `MockKeyringClient` (via `mockall_double`), `MockBlockChainClient`
- Example: `daemon/src/state.rs` tests module

### Integration Tests (Black-Box)
- Jest/TypeScript test suite in `tests/kalatori-api-test-suite/`
- Hits live API endpoints against a running daemon + Chopsticks (Substrate chain fork simulator)
- Docker Compose environment in `tests/`

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
# Terminal 1: Start test environment
cd tests
docker-compose up

# Terminal 2: Run tests
cd tests/kalatori-api-test-suite
yarn
yarn test

# Run a specific test
yarn test -t "should create, repay, and automatically withdraw an order in USDC"
```

Ensure `DAEMON_HOST` points to the running daemon (default: `localhost:16726`).

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
- **Docker network**: `kalatori-network` (create with `make create-network` or `docker network create kalatori-network`)
- **Start/stop**: `make start-chopsticks` / `make stop-chopsticks`

## Tool Versions

Pinned in `Makefile`:
- nextest: 0.9.129
- llvm-cov: 0.8.4
- cargo-mutants: 26.2.0

Install all: `make setup-utils`
