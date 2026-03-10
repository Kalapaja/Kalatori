# Migrate Kalatori CI to Dagger

## Context

Kalatori's CI currently uses 16 GitHub Actions workflow files (5 entry points + 11 reusable `_job-*.yml` workflows + 2 custom composite actions). While well-structured, this setup has typical GHA pain points: no local reproducibility, ephemeral caches that miss frequently, slow SQLite-from-source builds repeated across jobs, and a 5-minute sleep hack for integration test startup. Migrating to Dagger with the existing shared remote engine will give us: persistent layer + volume caches across runs, local dev parity via `dagger call`, MCP integration for AI-assisted development, and simplified workflow files.

## Dagger Module Structure

```
ci/
  dagger.json              # Module config (sdk: typescript, pinned engineVersion)
  package.json / tsconfig.json
  src/
    index.ts               # Module class (KalatoriCi) + all() entrypoint
    versions.ts            # ALL tool versions centralized here
    base.ts                # DAG branches: osBase, rustBase, depCache, subxtCli,
                           #   metadataFile, kassetteFrontEnd, buildContainer, runtimeImage
    checks.ts              # checkFmt, checkClippy, checkDeny, checkMachete
    tests.ts               # testUnit, testUnitCoverage, testIntegration
    docker.ts              # dockerBuild, dockerSmoke, dockerPublish
    helpers.ts             # withCargoCaches, shared utils
```

Written in **TypeScript** (official Dagger SDK).

### `versions.ts` — Single source of truth

All versions currently scattered across `Makefile`, `Dockerfile`, `.github/actions/`:

```typescript
export const VERSIONS = {
  rust: "1.93",
  rustNightly: "nightly",           // pin to a date if formatting becomes inconsistent
  subxtCli: "0.44.0",
  sqlite: "3.51.0",
  sqliteSourceUrl: "https://www.sqlite.org/2025/sqlite-autoconf-3510000.tar.gz",
  sqlxCli: "0.8.6",
  nextest: "0.9.129",
  llvmCov: "0.8.4",
  mutants: "26.2.0",
  cargoDeny: "0.19.0",
  cargoMachete: "0.9.1",
  cargoChef: "0.1.71",              // dependency pre-build caching
  kassette: "0.0.4",
  metadataRpcUrl: "wss://asset-hub-polkadot-rpc.n.dwellir.com",
} as const;
```

### Function Naming (kebab-case CLI)

| TS Method | CLI command | What it does |
|---|---|---|
| `checkFmt` | `check-fmt` | nightly rustfmt |
| `checkClippy` | `check-clippy` | clippy -D warnings (pedantic) |
| `checkDeny` | `check-deny` | cargo-deny advisories + bans/licenses |
| `checkMachete` | `check-machete` | **NEW** — unused dependency detection |
| `testUnit` | `test-unit` | cargo-nextest (no coverage) |
| `testUnitCoverage` | `test-unit-coverage` | nextest + llvm-cov → lcov.info |
| `testIntegration` | `test-integration` | Chopsticks services + daemon + examples |
| `dockerBuild` | `docker-build` | Programmatic image build |
| `dockerSmoke` | `docker-smoke` | Run `/app/kalatori --help` |
| `dockerPublish` | `docker-publish` | Push to GHCR |
| `all` | `all` | Everything in parallel (local dev) |

The module constructor takes `source: Directory` (defaultPath `..`) and `gitDir: Directory` (defaultPath `../.git`). All public functions use `@doc` + default path annotations for MCP.

---

## Base Container: DAG Caching Strategy

The key insight for Rust in Dagger: **dependency compilation is the expensive step**, and Dagger is a **DAG, not a linear layer chain**. We decompose the build into independent branches that run in parallel and converge only when needed.

### Why not linear layers?

The old plan's Layer 3 bundled subxt-cli (changes rarely), metadata.scale (regenerated every run), and Kassette front-end (changes frequently for staging) into one layer. Any Kassette bump would invalidate everything downstream, including the expensive dependency build. The DAG model eliminates this coupling entirely.

### Branch A: Build Environment (sequential, cached aggressively)

**A1 — OS + Build Tools + SQLite** (changes: ~never)

`debian:bookworm-slim` + build-essential, clang, pkg-config, libssl-dev, ca-certificates, curl, git. Then compile SQLite 3.51.0 from source with the exact CFLAGS from the current Dockerfile/setup-sqlite action. Install to `/usr/local`, set `PKG_CONFIG_PATH`, `LD_LIBRARY_PATH`, `SQLITE3_LIB_DIR`, `SQLITE3_INCLUDE_DIR`.

**A2 — Rust Toolchain** (changes: on Rust version bump)

`rustup` install at pinned version. `PATH` includes `~/.cargo/bin`. Set `CARGO_INCREMENTAL=0` (incremental compilation wastes disk and doesn't help with Dagger's layer-based caching; the persistent engine's CacheVolumes handle reuse).

**A3 — cargo-chef: Dependency Pre-build** (changes: on Cargo.toml/Cargo.lock change)

Uses [cargo-chef](https://github.com/LukeMathWalker/cargo-chef) instead of manual dummy source stubs. Two steps:

1. **Prepare**: `cargo chef prepare --recipe-path recipe.json` — runs against full source, produces a deterministic `recipe.json` capturing the workspace's complete dependency topology (all `Cargo.toml` files, `Cargo.lock`, features, examples, build scripts).
2. **Cook**: `cargo chef cook --release --all-features --recipe-path recipe.json` — builds all dependencies from `recipe.json` without any project source code. This Dagger layer is cached as long as `recipe.json` doesn't change (i.e., `Cargo.toml`/`Cargo.lock` are unchanged).

cargo-chef is installed via the `cargo-tools` CacheVolume pattern (instant after first run).

**Why cargo-chef over manual stubs?** The current Dockerfile has 5+ manual stub files (`daemon/src/main.rs`, `client/src/lib.rs`, 3 examples). Adding a workspace member or example requires updating stubs. cargo-chef's `recipe.json` captures the full workspace topology automatically, including features, inter-crate deps, and build scripts.

**Note**: `metadata.scale` is NOT needed during `cargo chef cook`. The `#[subxt::subxt(runtime_metadata_path = ...)]` proc macro at `daemon/src/chain_client/asset_hub.rs:57` only expands when daemon source is compiled — during cook, only dependency crates compile.

CacheVolumes mounted during cook:
- `cargo-registry` → `/usr/local/cargo/registry`
- `cargo-git` → `/usr/local/cargo/git/db`

### Branch B: subxt-cli Installation (independent of A3)

Reuses A1+A2 base → `cargo install subxt-cli` with `cargo-tools` CacheVolume. Independent of the dependency build — on warm cache, subxt-cli is available instantly.

If `paritytech/subxt` publishes pre-built Linux binaries, use `dag.http()` instead (~30s download vs ~2-3 min compile on cold cache). Check releases page during implementation.

### Branch C: metadata.scale (depends on B only)

Uses subxt-cli from Branch B → `subxt metadata --url $MetadataRpcUrl -f bytes -o metadata.scale` → returns a `File`.

Cache-busted every pipeline run via timestamp env var (~5s, avoids stale metadata from chain runtime upgrades). Runs **in parallel** with the expensive Branch A3 dep compilation — its cost is fully hidden.

### Branch D: Kassette Front-end (parallel, needed at build AND runtime)

`dag.http()` download (content-addressed by URL) → unzip in lightweight container → returns a `Directory`.

**Important**: Kassette is both a **build-time AND runtime** dependency:
- `daemon/src/api/public.rs:33` uses `include_str!("../../../static/index.html")` — embeds `index.html` into the binary at compile time
- `daemon/src/api/public.rs:122` uses `ServeDir::new("static/assets")` — serves JS/CSS assets from filesystem at runtime

This means Kassette must be downloaded and mounted into the source tree **before `cargo build`** (not just at runtime image assembly). However, Branch D is still independent of Branches A-C and runs in parallel with them — it joins the DAG at the final build convergence point. A Kassette version bump invalidates only the final build layer; the expensive cargo-chef dep cache is unaffected since `include_str!` resolves during project source compilation, not dependency compilation.

### Convergence: Final Build

Mount paths are critical for macro resolution:
- **Kassette** → `/src/static/` (so `include_str!("../../../static/index.html")` resolves from `/src/daemon/src/api/public.rs`)
- **metadata.scale** → `/src/metadata.scale` (so `runtime_metadata_path = "../metadata.scale"` resolves from `/src/daemon/Cargo.toml` via `CARGO_MANIFEST_DIR`)
- **.git** → `/src/.git` (for shadow-rs in `build.rs`)

```
A3 (cached deps)
  + C (metadata File → /src/metadata.scale)
  + D (Kassette → /src/static/)
  + .git dir → /src/.git (optional — see below)
  + full source
  → cargo build --release

Binary + A1 (SQLite .so) + D (Kassette static/) → runtime image
```

### Handling `.git` for shadow-rs

`daemon/build.rs` uses `shadow-rs` (`new_deny`) to embed `COMMIT_HASH`, `LAST_TAG`, `TAG`, `GIT_CLEAN`, etc. into the binary. `.dockerignore` excludes `.git` (and Dagger respects this for `defaultPath`).

**Graceful degradation**: shadow-rs does NOT fail when `.git` is missing — it produces placeholder values. The existing Dockerfile has always excluded `.git`, and Docker builds work fine. This means `.gitDir` is a **nice-to-have** for correct `--version` output on release binaries, not a build requirement. Non-release functions (checkClippy, tests) work without it.

**Solution**: Load `.git` as a separate `Directory` argument in the module constructor:

```typescript
constructor(
  @argument({ defaultPath: ".." })
  source: Directory,              // excludes .git per .dockerignore — stable for caching
  @argument({ defaultPath: "../.git" })
  gitDir: Directory,              // .git loaded separately — changes per commit
)
```

`.git` is mounted **only in `buildContainer`/`runtimeImage`** (release builds) — the dep cache (cargo-chef cook) never sees it. Since the final build already invalidates on every source change, mounting `.git` adds zero additional cache cost. shadow-rs finds `.git` at `/src/.git` and embeds correct git metadata. For non-release functions (checkClippy, tests), `.gitDir` is not mounted — shadow-rs falls back to placeholders, which is acceptable.

### DAG Parallelism Diagram

```
A1 (OS+SQLite) ─► A2 (Rust) ─┬─► A3 (chef cook) ───────────────────────┐
                             │                                         │
                             └─► B (subxt-cli) ─► C (metadata.scale) ──┼─► Final Build ─► Runtime Image
                                                                       │       │                │
                                             D (Kassette) ─────────────┘       │                │
                                                                               │  D (assets/) ──┘
                                             .git (separate arg) ──────────────┘
```

`D` participates twice: `static/index.html` is mounted before `cargo build` (for `include_str!`), and `static/assets/` is copied into the runtime image (for `ServeDir`).

**Parallelism gains:**
- A3 (dep compilation, 2-5 min) runs simultaneously with B+C (metadata, ~8s total) and D (Kassette, ~2s)
- A Kassette version bump: ~2s download cost, invalidates only the final build layer (dep cache unaffected)
- A metadata regeneration: ~5s cost, hidden behind dep compilation

---

## Caching Conventions

### Module-scoped CacheVolumes

Dagger CacheVolumes are scoped to the module that creates them. `dag.cacheVolume("cargo-registry")` in `kalatori-ci` is isolated from identically-named volumes in other modules on the shared remote engine. No namespace prefix is needed.

### Standard cache volume names

| Volume name | Mount path | What it caches | Used by | Sharing mode |
|---|---|---|---|---|
| `cargo-registry` | `/usr/local/cargo/registry` | Downloaded crate sources | All cargo-based functions | Locked |
| `cargo-git` | `/usr/local/cargo/git/db` | Git dependency checkouts | All cargo-based functions | Locked |
| `cargo-tools` | `/cargo-tools` | Installed tool binaries: cargo-chef, cargo-deny, cargo-machete, subxt-cli, nextest, llvm-cov (via `CARGO_INSTALL_ROOT`) | All tool-installing functions | Shared (write-once) |

**No `cargo-target` CacheVolume**: With cargo-chef, dependency compilation is cached as a Dagger layer (stable per `recipe.json`). Mounting a CacheVolume at `/src/target` would **shadow** the cook output (Dagger volumes overlay the path and start empty), defeating the dep cache. The final build compiles only project source (~30s with `CARGO_INCREMENTAL=0`). Retries hit Dagger's layer cache (same inputs = same cached layer).

### Layer cache vs CacheVolumes

The persistent remote engine provides **automatic layer caching** — identical `withExec` steps are cached if all preceding layers match. This is free and works well for deterministic commands.

**CacheVolumes** add robustness on top:
- Survive layer cache evictions (LRU pressure from other builds on the shared engine)
- Work across base image changes (Rust version bump invalidates layers but not volumes)
- Enable cross-function sharing (checkDeny and checkMachete share the same registry cache)

**Rule of thumb**: Use CacheVolumes for any directory that accumulates data across runs (downloads, compiled artifacts, installed binaries). Rely on layer cache for everything else.

### Tool installation pattern

Never mount a CacheVolume at `/usr/local/cargo/bin` — it hides `cargo`/`rustc` from the base image (empty volume on first mount). Instead, use `CARGO_INSTALL_ROOT` pointed at a cached path:

```typescript
.withEnvVariable("CARGO_INSTALL_ROOT", "/cargo-tools")
.withMountedCache("/cargo-tools", dag.cacheVolume("cargo-tools"))
.withEnvVariable("PATH", "/cargo-tools/bin:$PATH", { expand: true })
```

This lets `cargo install` detect existing binaries and skip recompilation entirely.

### CacheSharingMode

`cargo-registry` and `cargo-git` use **`Locked`** mode (serialized access). Without this,
concurrent `cargo install` commands (e.g. `checkDeny` + `checkMachete` running in parallel)
collide when unpacking crate sources — "File exists (os error 17)" on `.cargo-ok` files.
The performance impact is negligible: on warm cache, `cargo install` exits in ~0.1s so the
lock is held briefly.

`cargo-tools` uses default **`Shared`** mode — it's write-once per tool version with no
concurrent unpacking conflict.

---

## What Stays in GitHub Actions vs Moves to Dagger

| Component | Where | Rationale |
|---|---|---|
| Semantic PR title validation | **GHA** | `amannn/action-semantic-pull-request` — reads GitHub PR metadata |
| Release validate (branch, semver, changelog) | **GHA** | Needs git tags, branch names from GitHub context |
| Release prepare (extract version/changelog) | **GHA** | Pure text, outputs consumed by GHA jobs |
| GitHub Release creation | **GHA** | `softprops/action-gh-release` GitHub API |
| Codecov upload | **GHA** | `codecov/codecov-action` PR integration; Dagger exports lcov.info |
| Dependabot | **GHA** | Native GitHub feature |
| **Everything else** | **Dagger** | fmt, clippy, deny, machete, tests, coverage gen, integration, Docker |

---

## CI Workflow Files (Post-Migration)

```
.github/
  workflows/
    pr.yml                  # Replaces pr-to-dev.yml AND pr-to-main.yml
    merge-to-dev.yml        # Simplified
    merge-to-main.yml       # Simplified
    release.yml             # Simplified
    _job-semantic-pr.yml    # KEPT AS-IS
    _job-release-validate.yml  # KEPT (or inlined)
    _job-release-prepare.yml   # KEPT (or inlined)
    _job-github-release.yml    # KEPT (or inlined)
  actions/
    setup-dagger/action.yml # NEW: SSH + Dagger CLI setup (replaces setup-rust-build-env + setup-sqlite)
```

### `setup-dagger/action.yml` — Composite action

Reuses the existing org-level secrets/variables:

```yaml
inputs:
  ssh-private-key:
    required: true
  known-host:
    required: true
runs:
  using: composite
  steps:
    - uses: webfactory/ssh-agent@v0.9.0
      with:
        ssh-private-key: ${{ inputs.ssh-private-key }}
    - name: Trust remote Dagger host
      shell: bash
      run: |
        mkdir -p ~/.ssh
        echo "${{ inputs.known-host }}" >> ~/.ssh/known_hosts
    - uses: dagger/dagger-for-github@v8
      with:
        version: "0.20.1"  # must match dagger.json engineVersion
```

### `pr.yml` — Matrix strategy

```yaml
name: PR Checks
on:
  pull_request:
    branches: [develop, main]
env:
  DOCKER_HOST: ${{ vars.DAGGER_CI_HOST }}
  _EXPERIMENTAL_DAGGER_RUNNER_HOST: docker-container://dagger-engine-v0.20.1

jobs:
  semantic-pr:
    uses: ./.github/workflows/_job-semantic-pr.yml

  checks:
    name: ${{ matrix.name }}
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        include:
          - { name: Format,      command: check-fmt }
          - { name: Clippy,      command: check-clippy }
          - { name: Deny,        command: check-deny }
          - { name: Machete,     command: check-machete }
          - { name: Tests,       command: test-unit-coverage }
          - { name: Integration, command: test-integration }
    steps:
      - uses: actions/checkout@v4
      - uses: ./.github/actions/setup-dagger
        with:
          ssh-private-key: ${{ secrets.DAGGER_CI_SSH_KEY }}
          known-host: ${{ vars.DAGGER_CI_KNOWN_HOST }}
      - run: dagger call ${{ matrix.command }}
      # Coverage upload (only for test-unit-coverage)
      - if: matrix.command == 'test-unit-coverage'
        run: dagger call test-unit-coverage export --path lcov.info
      - if: matrix.command == 'test-unit-coverage'
        uses: codecov/codecov-action@v5
        with:
          files: lcov.info
          token: ${{ secrets.CODECOV_TOKEN }}

  release-validate:
    if: github.base_ref == 'main'
    uses: ./.github/workflows/_job-release-validate.yml
```

Each matrix entry = separate PR status check. Adding a check = one-line matrix entry.

---

## New Checks to Add

### cargo-machete (every PR)
Detects unused dependencies by scanning source. No compilation, runs in seconds. Lightweight container with just Rust + `cargo-machete`.

### cargo-mutants (scheduled, post-migration)
Mutation testing — CPU-intensive. Run weekly on `develop` (e.g., `cron: '0 3 * * 0'`). Results as GHA artifact or issue. Uses `dagger call test-mutants`.

---

## Integration Test in Dagger (replacing sleep 300)

Current approach: start Chopsticks + daemon in background, `sleep 300`, run examples.

Dagger approach:
1. **Chopsticks Service**: Build from `chopsticks/Dockerfile`, mount config files (`pd-ah.yml`, `pd-ah-2.yml`), expose ports 9000/9500. Start via `container.asService()`.
2. **Daemon Service**: Build daemon binary, bind Chopsticks services via `withServiceBinding("chopsticks-asset-hub", chopsSvc)`. Configure daemon JSON configs to use service hostname instead of localhost. Start as service with health check.
3. **Test Runner**: Container with compiled examples. Bind daemon service. Run `crud` and `webhook` examples.
4. **Health check polling** instead of sleep: a small `withExec` loop hitting `/health` with 2s intervals, 120s timeout. Replaces the 300s sleep.

### Service Startup Ordering

Explicitly `start()` the Chopsticks service before starting the daemon (which depends on it), and start the daemon before running test examples. Dagger's service binding handles DNS resolution between containers.

### Config File Adaptation

Daemon config files (`chains.json`, etc.) need hostname changes for Dagger:
- RPC endpoint: `ws://chopsticks-asset-hub:9000` (service hostname, not localhost)
- Web server binds `0.0.0.0` (already does this)

Mount adapted configs into the daemon container via `withFile()`.

---

## Remote Dagger Engine

Reuse the existing org-level GitHub variables/secrets:

| GitHub Config | Type | Already Exists |
|---|---|---|
| `DAGGER_CI_HOST` | Variable (org) | Yes |
| `DAGGER_CI_KNOWN_HOST` | Variable (org) | Yes |
| `DAGGER_CI_SSH_KEY` | Secret (org) | Yes |

No new infrastructure needed. The persistent remote engine accumulates cache across all org projects.

---

## Implementation Phases

### Phase 1: Scaffold + No-Compilation Checks

- [x] Run `dagger init --sdk=typescript` in `ci/` directory
- [x] Create `src/versions.ts` with all pinned tool versions from Makefile/Dockerfile
- [x] Create `src/index.ts` with `KalatoriCi` module class
- [x] Implement `checkFmt` in `src/index.ts`:
  - [x] Container: `rust:slim-bookworm` + rustup nightly + rustfmt component
  - [x] Mount full source directory
  - [x] Run `cargo +nightly fmt --all -- --check`
- [x] Implement `checkDeny` in `src/index.ts`:
  - [x] Container: `rust:${VERSIONS.rust}-slim-bookworm` + `cargo install cargo-deny`
  - [x] Mount full source directory
  - [x] Run `cargo deny check advisories` (non-blocking) and `cargo deny check bans licenses sources`
- [x] Implement `checkMachete` in `src/index.ts` (**new check**):
  - [x] Container: `rust:${VERSIONS.rust}-slim-bookworm` + `cargo install cargo-machete`
  - [x] Mount source directory
  - [x] Run `cargo machete`
- [x] Create `.github/actions/setup-dagger/action.yml` (SSH + Dagger CLI)
- [x] Add Dagger checks to PR workflow **alongside existing checks** (dual-run for validation)
- [x] Add CacheVolumes for cargo tool installations (registry, git, installed binaries)
- [ ] Verify result parity over 3-5 PRs
- [ ] Remove old `_job-fmt.yml` and `_job-cargo-deny.yml` usage from workflows

### Phase 2: Base Container (DAG) + cargo-chef + checkClippy

- [ ] Add `cargoChef` version to `src/versions.ts`
- [ ] Add `gitDir` constructor parameter with `defaultPath: "../.git"` in `src/index.ts`
- [ ] Implement `src/base.ts` with DAG branches:
  - [ ] **Branch A — Build Environment:**
    - [ ] `osBase()`: `debian:bookworm-slim` + build tools + SQLite 3.51.0 from source (CFLAGS)
    - [ ] `rustBase()`: extends `osBase` + Rust toolchain via rustup, sets `CARGO_INCREMENTAL=0`
    - [ ] `depCache(src)`: `rustBase` + cargo-chef prepare/cook (layer-cached per `recipe.json`)
  - [ ] **Branch B — subxt-cli:**
    - [ ] `subxtCli()`: `rustBase` + `withCargoCaches` + `cargo install subxt-cli` (cargo-tools volume)
  - [ ] **Branch C — metadata.scale:**
    - [ ] `metadataFile()`: uses `subxtCli()` → `subxt metadata --url ...` → returns `File` (cache-busted)
  - [ ] **Branch D — Kassette front-end:**
    - [ ] `kassetteFrontEnd()`: `dag.http()` + lightweight unzip container → returns `Directory`
  - [ ] **Convergence:**
    - [ ] `buildContainer(src, gitDir)`: `depCache` + `metadataFile` + `kassetteFrontEnd` (→ `/src/static/`) + full source + `.git` (→ `/src/.git`) → `cargo build --release`
    - [ ] `runtimeImage(src, gitDir)`: slim debian + binary + SQLite .so + `kassetteFrontEnd` (→ `/app/static/`)
- [ ] Implement `checkClippy` in `src/checks.ts`:
  - [ ] Uses dep-cached container from `depCache` + `metadataFile` (→ `/src/metadata.scale`) + `kassetteFrontEnd` (→ `/src/static/`) + full source (needs SQLite, metadata for subxt/sqlx macros, Kassette for `include_str!`)
  - [ ] `.gitDir` NOT needed — shadow-rs degrades gracefully with placeholders for non-release builds
  - [ ] Run `RUSTFLAGS="-Dwarnings" cargo clippy --all-targets --all-features`
- [ ] Run side-by-side with existing clippy workflow
- [ ] Benchmark timing: target within 2x of current GHA with rust-cache
- [ ] Remove old `_job-clippy.yml` usage

### Phase 3: Tests + Coverage

- [ ] Implement `testUnit` in `src/tests.ts`:
  - [ ] Uses base container + cargo-nextest
  - [ ] Run `cargo nextest run`
- [ ] Implement `testUnitCoverage` in `src/tests.ts`:
  - [ ] Uses base container + `llvm-tools-preview` component + `cargo-llvm-cov`
  - [ ] Run `cargo llvm-cov nextest -p kalatori --lcov --output-path lcov.info`
  - [ ] Return lcov.info as `File` for export
- [ ] Update GHA workflow: `dagger call test-unit-coverage export --path lcov.info` + Codecov upload
- [ ] Verify coverage numbers match existing workflow output
- [ ] Remove old `_job-cargo-test.yml` and `_job-cargo-test-coverage.yml` usage

### Phase 4: Integration Tests + Docker

- [ ] Implement Chopsticks as Dagger Service in `src/tests.ts`:
  - [ ] Build from `chopsticks/Dockerfile`
  - [ ] Mount `pd-ah.yml` and `pd-ah-2.yml` config files
  - [ ] Expose ports 9000 and 9500
  - [ ] Start via `container.asService()`
- [ ] Implement daemon as Dagger Service:
  - [ ] Build daemon binary from base container
  - [ ] Create adapted config files (service hostnames instead of localhost)
  - [ ] Bind Chopsticks service via `withServiceBinding`
  - [ ] Health check polling: loop on `/health` every 2s, 120s timeout
- [ ] Implement `testIntegration`:
  - [ ] Start Chopsticks → Start daemon → Run `cargo run --example crud` → Run `cargo run --example webhook`
  - [ ] Proper error reporting on failure
- [ ] Implement `dockerBuild` in `src/docker.ts`:
  - [ ] Programmatic image construction (not `docker build`)
  - [ ] Release binary from base container
  - [ ] Runtime: `debian:bookworm-slim` + SQLite lib + binary + front-end
  - [ ] Non-root user `kalatori:1000`, expose port 8080
- [ ] Implement `dockerSmoke`:
  - [ ] Run `/app/kalatori --help` in built image
- [ ] Implement `dockerPublish`:
  - [ ] Accept registry credentials as `Secret`
  - [ ] Publish with provided tags
- [ ] Run side-by-side with existing integration + Docker workflows
- [ ] Remove old `_job-integration-test.yml` and `_job-docker-build.yml` usage

### Phase 5: Consolidate + Clean Up

- [ ] Collapse `pr-to-dev.yml` + `pr-to-main.yml` → `pr.yml` with matrix strategy
- [ ] Simplify `merge-to-dev.yml` (test-coverage + integration + docker-publish via Dagger)
- [ ] Simplify `merge-to-main.yml` (test-coverage + integration via Dagger)
- [ ] Simplify `release.yml` (Dagger for test/build/publish, GHA for release creation)
- [ ] Delete replaced `_job-*.yml` files:
  - [ ] `_job-fmt.yml`
  - [ ] `_job-clippy.yml`
  - [ ] `_job-cargo-deny.yml`
  - [ ] `_job-cargo-test.yml`
  - [ ] `_job-cargo-test-coverage.yml`
  - [ ] `_job-integration-test.yml`
  - [ ] `_job-docker-build.yml`
- [ ] Delete replaced custom actions:
  - [ ] `.github/actions/setup-rust-build-env/`
  - [ ] `.github/actions/setup-sqlite/`
- [ ] Implement `all()` function for local dev (parallel execution of all checks + tests)
- [ ] Update `CLAUDE.md` — new CI commands, Dagger MCP usage
- [ ] Update `CONTRIBUTING.md` — new CI workflow, local dev with Dagger
- [ ] Update `Makefile` — keep local dev shortcuts, delegate to `dagger call` where appropriate
- [ ] Bump MSRV and Dagger Rust toolchain to latest stable (currently 1.93; Dagger pins its own toolchain via `versions.ts` so MSRV can advance independently once legacy `_job-*.yml` workflows are deleted)

---

## Verification Strategy

### Per-Phase (old + new run in parallel)
- **Result parity**: Same pass/fail for 3-5 consecutive PRs
- **Timing comparison**: Track with persistent engine cache warmth
- **Coverage parity**: Compare Codecov numbers

### Post-Migration
- [ ] `dagger call all` locally — full suite passes
- [ ] MCP integration: default path works from Claude Code
- [ ] Cache effectiveness: second run of same PR significantly faster
- [ ] Failure mode: intentional bad format/clippy/test caught correctly
- [ ] Engine failover: unreachable engine fails fast (not hangs)

### Integration Test Specific
- [ ] Health check polling replaces `sleep 300` — faster and more reliable
- [ ] Chopsticks service bindings resolve correctly
- [ ] Daemon connects to Chopsticks via service hostname
- [ ] Test examples (crud, webhook) pass against Dagger-hosted services

---

## Critical Files to Reference During Implementation

| File | Role |
|---|---|
| `Makefile` | Source of all tool versions → replicated in `versions.ts` |
| `Dockerfile` | Reference for SQLite build flags, release profile |
| `.dockerignore` | Dagger respects this for `defaultPath` — excludes `.git`, `static/`, `target/` |
| `.github/actions/setup-sqlite/action.yml` | Exact SQLite CFLAGS and env vars to reproduce |
| `.github/actions/setup-rust-build-env/action.yml` | subxt-cli install, metadata download, front-end download |
| `.github/workflows/_job-integration-test.yml` | Chopsticks + daemon orchestration → Dagger Services |
| `.github/workflows/_job-cargo-test-coverage.yml` | Coverage + Codecov pattern → split Dagger/GHA |
| `.github/workflows/_job-docker-build.yml` | Docker build + GHCR push → Dagger native |
| `chopsticks/docker-compose.yml` + `chopsticks/pd-ah.yml` | Chopsticks config → mount into Dagger Service |
| `daemon/configs/` | Config templates to adapt for service hostnames |
| `daemon/build.rs` | shadow-rs build script — needs `.git` for commit hash, tags, etc. |
| `daemon/src/chain_client/asset_hub.rs:57` | `runtime_metadata_path = "../metadata.scale"` — confirms metadata only needed at source compile |
| `deny.toml`, `rustfmt.toml` | Mounted into respective check containers |

---

## Key Design Decisions

1. **TypeScript for Dagger module** — official SDK, good for CI scripting.

2. **cargo-chef for dependency caching** — produces a stable Dagger layer per `recipe.json` that invalidates only on `Cargo.toml`/`Cargo.lock` changes. Combined with automatic layer caching on the persistent engine, this gives reliable dep caching without sccache complexity. CacheVolumes handle registry/git downloads across layer invalidations (e.g., Rust version bumps).

3. **metadata.scale regenerated each run** — takes ~5s, avoids stale metadata from chain runtime upgrades. Shared as `File` across all functions within a single pipeline invocation.

4. **Existing remote engine** — org-level secrets already configured. No new infrastructure.

5. **Matrix strategy** (not consolidated job) — per-check GitHub status, focused logs, one-line to add new check. `dagger call all` for local dev consolidation.

6. **Codecov stays in GHA** — Dagger generates lcov.info, GHA uploads. Preserves PR comment integration.

7. **Release validation stays in GHA** — needs git tags/branch context that Dagger source directories don't include.

8. **Full Dagger services for integration** — Chopsticks + daemon as Dagger Services with health check polling. Eliminates the 300s sleep, fully reproducible locally.
