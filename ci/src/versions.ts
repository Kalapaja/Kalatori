/**
 * Centralized version pinning for all tools used in CI.
 *
 * Single source of truth — update versions here, not scattered across
 * Makefile, Dockerfile, or GitHub Actions workflows.
 */
export const VERSIONS = {
  /** Stable Rust toolchain version — must be >= MSRV in daemon/Cargo.toml */
  rust: "1.93",
  /** Nightly toolchain for rustfmt — tracks latest; pin to a specific date if formatting becomes inconsistent */
  rustNightly: "nightly",
  /** subxt-cli version — keep in sync with subxt in Cargo.toml */
  subxtCli: "0.44.0",
  /** SQLite version compiled from source */
  sqlite: "3.51.0",
  /** SQLite source tarball URL */
  sqliteSourceUrl:
    "https://www.sqlite.org/2025/sqlite-autoconf-3510000.tar.gz",
  /** sqlx-cli version — keep in sync with sqlx in Cargo.toml */
  sqlxCli: "0.8.6",
  /** cargo-nextest version */
  nextest: "0.9.129",
  /** cargo-llvm-cov version */
  llvmCov: "0.8.4",
  /** cargo-mutants version */
  mutants: "26.2.0",
  /** cargo-deny version */
  cargoDeny: "0.19.0",
  /** cargo-machete version */
  cargoMachete: "0.9.1",
  /** Kassette front-end release version */
  kassette: "0.0.4",
  /** Metadata RPC endpoint for subxt */
  metadataRpcUrl: "wss://asset-hub-polkadot-rpc.n.dwellir.com",
} as const
