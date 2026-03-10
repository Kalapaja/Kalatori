## Issues

You can find issues waiting to be solved or create a new one in the [issues](https://github.com/Kalapaja/Kalatori/issues) section.

## Prerequisites

- Rust: stable version (MSRV 1.88), nightly for rustfmt
- SQLite: >= 3.47.0 (see README.md for build-from-source instructions on Linux)
- Docker: to run tests and spawn Chopsticks instances
- Node.js and Yarn: to run integration tests
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

## Version Bumping and Release Process

When you make changes that require a new version of the project, follow these steps to bump the version:

1. **Update the Version Number**:
    - Update version in `Cargo.toml`

2. **Update the Changelog**:
    - Run `git cliff <range> --tag <new-version>` to generate the changelog for the new version.
   ```bash
   # For example in my case the origin of main repository marked as main,
   # som main/main is the main branch of the main repository.
   # 2.1.2 is version example.  
    git cliff main/main..HEAD --tag 2.1.2 -p CHANGELOG.md 
   ```
    - Review the changelog to ensure that the description is meaningful

3. **Add version related changes to commit**:
   ```bash
   git add CHANGELOG.md Cargo.toml Cargo.lock
   git commit -m "chore: bump version to 2.1.2"
   git push origin <branch-name>
    ```

4. **Tag the version at main branch**:
    ```bash
    git tag -a v2.1.2 -m "Release version 2.1.2"
    git push origin v2.1.2
    ```

