## Issues

You can find issues waiting to be solved or create a new one in the [issues](https://github.com/Kalapaja/Kalatori/issues) section.

## Prerequisites

- Rust: stable version (MSRV 1.88), nightly for rustfmt
- SQLite: >= 3.47.0 (see README.md for build-from-source instructions on Linux)
- Docker: to run tests and spawn Chopsticks instances
- subxt-cli: install via `make install-subxt-cli`
- sqlx-cli: install via `make install-sqlx-cli`

For AI agents and detailed architecture, see [AGENTS.md](AGENTS.md) and `docs/`.

## Preparing development environment

It's possible to mimic to spawn chopsticks instances in parallel for development purposes. 
Chopsticks Dockerfile exposes 4 ports (8000, 8500, 9000, 9500), so you can spawn up to 4 instances of chopsticks and each one of them will look at different RPC (note that those will be different chains).
Note that the RPCs are not real, so the changes made on one chopsticks instance will not affect the others.

1. `cd chopsticks`
2. Create docker network (do once): `docker network create kalatori-network`
3. `docker compose up` (edit docker-compose.yml to adjust instance count)
4. Copy example configs: `make copy-configs`
5. Start the daemon: `make run`

## Running tests locally

Unit tests:
```bash
make cargo-test
```

Integration tests (requires a running daemon with Chopsticks):
```bash
# Terminal 1: Start daemon with Chopsticks
make run

# Terminal 2: Run integration examples
make run-test-examples
```

## Branching and Pull Requests

`main` is the only long-lived branch. All changes — features, fixes, version bumps — land via pull request:

1. Branch off `main` (e.g. `feat/<short-name>`, `fix/<short-name>`, `chore/release-<version>`).
2. Open a PR targeting `main`. The `PR to main` workflow runs semantic-PR-title validation, `cargo fmt`, `cargo clippy`, `cargo deny`, unit tests (with coverage), and integration tests. All must pass before merging.
3. After merge, the `Merge to main` workflow re-runs tests on the merge commit and publishes a `ghcr.io/<owner>/kalatori-dev:<sha>` + `:latest` image.

There is no `develop` or release branch — release happens directly from `main` via a tag (see below).

## Version Bumping and Release Process

Releases are triggered by pushing a `v<version>` tag. The release workflow asserts that the tagged commit is internally consistent (Cargo.toml version matches the tag, CHANGELOG entry exists, version increments over the previous tag) — if those don't line up, the release fails fast.

The flow is a normal PR followed by a tag:

1. **Open a release PR off `main`**:
    - Update version in `daemon/Cargo.toml`.
    - Regenerate `Cargo.lock` (`cargo check` is enough).
    - Generate the changelog entry. Example (replace `2.1.2` with the new version, and `origin/main` with whatever remote you track):
      ```bash
      git cliff origin/main..HEAD --tag 2.1.2 -p CHANGELOG.md
      ```
    - Review the generated `## [2.1.2]` section in `CHANGELOG.md` and edit for clarity.
    - Commit and push:
      ```bash
      git add daemon/Cargo.toml Cargo.lock CHANGELOG.md
      git commit -m "chore: bump version to 2.1.2"
      git push origin chore/release-2.1.2
      ```
    - Open a PR and merge once CI is green.

2. **Tag the merged commit on `main`**:
    ```bash
    git checkout main && git pull
    git tag -a v2.1.2 -m "Release version 2.1.2"
    git push origin v2.1.2
    ```

   Pushing the tag triggers the `Release` workflow:
   - `release-validate` checks that `daemon/Cargo.toml` version (`2.1.2`) matches the pushed tag (`v2.1.2`), that `v2.1.2` is the highest existing tag, and that `CHANGELOG.md` has a `## [2.1.2]` heading.
   - On success, the workflow runs the test suite, builds and pushes `ghcr.io/<owner>/kalatori:2.1.2` + `:latest`, and publishes a GitHub release with the changelog body.

   If `release-validate` fails, delete the tag (`git push origin :v2.1.2 && git tag -d v2.1.2`), fix the inconsistency on `main` via another PR, and retag.

