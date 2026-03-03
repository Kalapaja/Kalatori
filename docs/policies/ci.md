# CI Policies

1. Quality gates that must pass before merging into `dev`:
    - Commit message format validation (conventional commits)
    - Branch name format validation
    - Code formatting checks
    - Linting
    - Compilation / type checking
    - Unit tests
    - Integration / end-to-end tests
    - Dependency audit (license compliance, security advisories)

2. Additional quality gates before merging into `main` (on top of all `dev` checks):
    - Source branch validation (only `dev` and `hotfix-*`)
    - Version validation (see point 3)
    - Changelog validation (see point 4)

3. Version is read from the project manifest (e.g., `Cargo.toml`, `package.json`). Before merge into `main`, the version must:
    - Be valid semver.
    - Not already exist as a git tag.
    - Increment major, minor, or patch over the latest existing tag.
    - If no tags exist yet, be `0.1.0` or `1.0.0`.

4. `CHANGELOG.md` must contain a section heading matching the version being released (e.g., `## [0.8.2]`). Changelog is semi-automatic:
    - Generated via tooling (e.g., `git-cliff`).
    - Reviewed and edited by a human before committing.

5. On merge into `main`, CI runs tests (with coverage) and integration tests. Release is a separate step triggered by a tag push:
    - The project owner manually creates a signed `vX.Y.Z` tag on the merge commit.
    - On tag push, CI runs tests, builds release artifacts with the release version.
    - CI extracts the relevant changelog section.
    - CI creates a GitHub release with changelog as release notes and artifacts attached.
    - Release artifacts may also be tagged as `latest` if the artifact type supports it (e.g., Docker images).

6. On merge into `dev`, CI automatically:
    - Runs tests (with coverage) and integration tests.
    - Builds artifacts with version `dev-${COMMIT_SHA}` (full commit hash).
    - Each artifact must be able to report this exact version at runtime (e.g., `--version` flag, `/health` endpoint, embedded metadata).
    - Dev artifacts use a separate image name (e.g., `kalatori-dev`) and are also tagged as `latest`.

7. Projects should pin their build toolchain versions to ensure reproducible builds across local development, CI, and Docker environments. Examples:
    - **Rust**: `rust-toolchain.toml`
    - **Node.js**: `.node-version` or `.nvmrc`
    - **Python**: `.python-version`
    - **Ruby**: `.ruby-version`

8. Test coverage reports with degradation tracking. Currently integrated via Codecov on PR, merge-to-dev, and merge-to-main workflows.

9. (Optional) Benchmark tracking with degradation alerts. Not required but encouraged.

10. (Optional) Metadata file attached to artifacts containing:
    - Commit hash
    - Pipeline ID
    - Build branch
    - Build toolchain version
    - Build timestamp

    Not required but encouraged.

11. (Optional) Test result reports for debugging failed runs. Not required but encouraged.


# Implementation Priorities

## High Priority

- Protected branches (Git 1)
- Build release artifacts on merge to `main` with version from manifest (CI 5)
- Create GitHub release automatically (CI 5)
- Create git tag automatically (CI 5)
- Run unit tests as a quality gate (CI 1)

## Medium Priority

- Version validation before merge to `main` (CI 3)
- Changelog validation before merge to `main` (CI 4)
- Attach changelog section to GitHub release (CI 5)
- Build artifacts on `dev` with `dev-${SHORT_SHA}` versioning (CI 6)
- Branch merge rules enforcement — only `dev` and `hotfix-*` into `main` (Git 2, CI 2)
- Branch name format validation (Git 5, CI 1)
- Commit message format validation (Git 7, CI 1)
- Run integration / end-to-end tests as a quality gate (CI 1)
- Dependency audit as a quality gate (CI 1)
- Code formatting and linting checks (CI 1)
- Build toolchain pinning (CI 7)

## Low Priority

- Test coverage reports (CI 8)
- Benchmark tracking (CI 9)
- Metadata file attached to artifacts (CI 10)
- Test result reports (CI 11)
- Automated deployment of `dev` artifacts to staging (Future)

## Future Considerations

- Multiple approvals required for merging into `main`.
- Automated deployment of `dev` artifacts to staging.
- Multi-package / monorepo support (independent versioning, tags, changelogs, and releases per package).
