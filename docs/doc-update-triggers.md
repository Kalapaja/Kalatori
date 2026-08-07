# Documentation Update Triggers

After completing code changes, check this table and update any affected docs. Also search `docs/` for references to affected concepts and update stale descriptions.

| Change Type | What to Update |
|---|---|
| **New module or major file** | `AGENTS.md` repo layout, `docs/architecture.md` component list |
| **New/changed error types** | `docs/error-handling.md` if principle applies, `docs/architecture.md` if new domain |
| **New/changed API endpoint** | `docs/architecture.md` API section, API spec repo |
| **Config file changes** | `AGENTS.md` tech stack (if new config type), `docs/architecture.md` config table |
| **Database migration** | `docs/DATABASE.md` schema and status transitions |
| **CI pipeline changes** | `docs/testing-strategy.md` CI pipeline section |
| **New test pattern or tool** | `docs/testing-strategy.md` |
| **Clippy lint changes** | `docs/conventions.md` lints section |
| **New chain support** | `AGENTS.md`, `docs/architecture.md` throughout |
| **MSRV or toolchain change** | `rust-version` in root `Cargo.toml` `[workspace.package]` (single source, inherited by all members), then `AGENTS.md` tech stack, `docs/conventions.md` code style, `CONTRIBUTING.md` prerequisites |
| **New dependency (major)** | `AGENTS.md` tech stack |
| **subxt/sqlx version bump** | `AGENTS.md` pitfalls (version sync), `[workspace.metadata.bin]` in the root `Cargo.toml`, and — for subxt only — the duplicate `subxt-cli` pin in `Dockerfile` |
| **MCP config changes** | `docs/mcp-tooling.md` |
| **New docs file created** | `AGENTS.md` Documentation Map table |
| **Makefile target changes** | `AGENTS.md` commands section, `docs/testing-strategy.md` if test-related |
