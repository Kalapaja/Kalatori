/**
 * Kalatori CI pipeline — Dagger module for build, test, and check automation.
 *
 * Run all checks locally: dagger call all
 * Run a single check:     dagger call check-fmt
 */
import {
  dag,
  Container,
  Directory,
  object,
  func,
  argument,
} from "@dagger.io/dagger"
import { VERSIONS } from "./versions.js"

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
    const base = dag
      .container()
      .from(`rust:${VERSIONS.rust}-slim-bookworm`)
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

    // Both run in parallel via Dagger's DAG.
    // Advisories failures are logged but don't fail the pipeline.
    let output = ""
    try {
      output += await advisories
    } catch (e) {
      output += `[advisory warnings — non-blocking]\n${e}\n`
    }
    output += await bansLicensesSources
    return output
  }

  /**
   * Detect unused dependencies with cargo-machete
   */
  @func()
  async checkMachete(): Promise<string> {
    return await dag
      .container()
      .from(`rust:${VERSIONS.rust}-slim-bookworm`)
      .withExec(["cargo", "install", "cargo-machete", "--version", VERSIONS.cargoMachete, "--locked"])
      .withMountedDirectory("/src", this.source)
      .withWorkdir("/src")
      .withExec(["cargo", "machete"])
      .stdout()
  }
}
