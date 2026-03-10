/**
 * Kalatori CI pipeline — Dagger module for build, test, and check automation.
 *
 * Run all checks locally: dagger call all
 * Run a single check:     dagger call check-fmt
 */
import {
  dag,
  CacheSharingMode,
  Container,
  Directory,
  object,
  func,
  argument,
} from "@dagger.io/dagger"
import { VERSIONS } from "./versions.js"

/**
 * Mount cargo caches for tool installations.
 *
 * - registry/git: avoids re-downloading crate sources
 * - /cargo-tools: persists installed binaries so `cargo install` is a no-op on warm cache
 *
 * CacheVolumes are module-scoped by Dagger — no collision with other
 * modules on the shared remote engine.
 */
function withCargoCaches(ctr: Container): Container {
  return ctr
    .withMountedCache(
      "/usr/local/cargo/registry",
      dag.cacheVolume("cargo-registry"),
      { sharing: CacheSharingMode.Locked },
    )
    .withMountedCache(
      "/usr/local/cargo/git/db",
      dag.cacheVolume("cargo-git"),
      { sharing: CacheSharingMode.Locked },
    )
    .withEnvVariable("CARGO_INSTALL_ROOT", "/cargo-tools")
    .withMountedCache("/cargo-tools", dag.cacheVolume("cargo-tools"))
    .withEnvVariable("PATH", "/cargo-tools/bin:$PATH", { expand: true })
}

@object()
export class KalatoriCi {
  source: Directory

  constructor(
    @argument({ defaultPath: ".." })
    source: Directory,
  ) {
    this.source = source
  }

  // ---------------------------------------------------------------------------
  // Checks (no compilation needed)
  // ---------------------------------------------------------------------------

  /**
   * Check Rust source formatting using nightly rustfmt
   */
  @func()
  async checkFmt(): Promise<string> {
    return await dag
      .container()
      .from("rust:slim-bookworm")
      .withExec([
        "rustup",
        "toolchain",
        "install",
        VERSIONS.rustNightly,
        "--profile",
        "minimal",
        "--component",
        "rustfmt",
      ])
      .withMountedDirectory("/src", this.source)
      .withWorkdir("/src")
      .withExec([
        "cargo",
        `+${VERSIONS.rustNightly}`,
        "fmt",
        "--all",
        "--",
        "--check",
      ])
      .stdout()
  }

  /**
   * Run cargo-deny checks (advisories + bans/licenses/sources)
   */
  @func()
  async checkDeny(): Promise<string> {
    const base = withCargoCaches(
      dag.container().from(`rust:${VERSIONS.rust}-slim-bookworm`),
    )
      .withExec(["cargo", "install", "cargo-deny", "--version", VERSIONS.cargoDeny, "--locked"])
      .withMountedDirectory("/src", this.source)
      .withWorkdir("/src")

    // Run both check groups — advisories is non-fatal (may have new unfixed CVEs),
    // bans/licenses/sources is strict
    const advisories = base
      .withExec(["cargo", "deny", "-L", "error", "check", "advisories"])
      .stdout()

    const bansLicensesSources = base
      .withExec([
        "cargo",
        "deny",
        "-L",
        "error",
        "check",
        "bans",
        "licenses",
        "sources",
      ])
      .stdout()

    // Both branches fork from `base` — Dagger's engine runs them in parallel.
    // Advisories failures are logged but don't fail the pipeline.
    const [advisoriesResult, bansResult] = await Promise.allSettled([
      advisories,
      bansLicensesSources,
    ])

    let output = ""
    if (advisoriesResult.status === "rejected") {
      output += `[advisory warnings — non-blocking]\n${advisoriesResult.reason}\n`
    } else {
      output += advisoriesResult.value
    }
    if (bansResult.status === "rejected") {
      throw bansResult.reason
    }
    output += bansResult.value
    return output
  }

  /**
   * Detect unused dependencies with cargo-machete
   */
  @func()
  async checkMachete(): Promise<string> {
    return await withCargoCaches(
      dag.container().from(`rust:${VERSIONS.rust}-slim-bookworm`),
    )
      .withExec(["cargo", "install", "cargo-machete", "--version", VERSIONS.cargoMachete, "--locked"])
      .withMountedDirectory("/src", this.source)
      .withWorkdir("/src")
      .withExec(["cargo", "machete"])
      .stdout()
  }
}
