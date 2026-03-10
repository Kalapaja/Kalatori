## A Gateway Daemon for Kalatori

!!! KALATORI IS IN PUBLIC BETA !!!

Kalatori is an open-source daemon designed to enable secure and scalable blockchain payment processing. Licensed under GPLv3 ([LICENSE](LICENSE)), Kalatori supports assets on Polkadot's Asset Hub parachain and Polygon.

The daemon derives unique accounts for each payment using a provided seed phrase and outputs all payments to a specified recipient wallet. It also offers transaction tracking for order management. Kalatori operates in a multithreaded mode and supports multiple currencies configured in JSON configuration files.

Client facing frontends can communicate with Kalatori leveraging exposed API described in the [API documentation](https://kalapaja.github.io/kalatori-api).

---
### Download

Download the latest Docker container or x86-64 release from the [GitHub releases page](https://github.com/Kalapaja/Kalatori/releases/latest).

---

### Compile from Source

#### Database Setup

The daemon relies on SQL syntax that is supported starting from SQLite `3.47.0`. At the moment, `sqlx` allows using the bundled (built-in) SQLite with `sqlite` feature enabled, but it enforces an older SQLite dependency. Once selecting the SQLite version is supported by `sqlx` (expected around `sqlx 0.9`), the bundled SQLite will be used, and a local SQLite installation will no longer be required.

If you plan to run the daemon on Linux, it is recommended to build SQLite from source, as the version provided by the package manager may be outdated. Build instructions can be found [here](https://sqlite.org/src/doc/trunk/doc/compile-for-unix.md), the latest version can be downloaded from [this page](https://www.sqlite.org/download.html).

There is a setup example for MacOS, which may be usefull for tests and local development.

1. Install SQLite via `brew`:
```sh
brew install sqlite3
```

2. Export the following environmental variables:
```sh
export PATH="/opt/homebrew/opt/sqlite/bin:$PATH"
export SQLITE3_LIB_DIR=/opt/homebrew/opt/sqlite/lib
export SQLITE3_INCLUDE_DIR=/opt/homebrew/opt/sqlite/include
```

#### Compilation

To compile the daemon, ensure you have the latest stable version of the Rust compiler installed. In order to compile
the daemon it also required to have blockchain node's metadata which can be fetched using `subxt-cli`. Step by step
workflow to compile the project will be following:

1. Install `subxt-cli` locally, into the `bin` folder:
```sh
make install-subxt-cli
```
2. Download Asset Hub's node metadata:
```sh
make download-node-metadata-ci
```
3. Build the daemon:
```sh
make build-release
```

The compiled binaries will be located in the `target/release` folder.

### Project Structure

- `daemon/`: Source code for the Kalatori daemon (Rust workspace member).
- `client/`: Public Rust client library for integrating with Kalatori.
- `migrations/`: SQLite database migration files.
- `configs/`: Example JSON configuration files for supported chains and assets.
- `docs/`: Project documentation (architecture, conventions, error handling, testing, etc.).
- `chopsticks/`: Configuration files for the Chopsticks tool and Docker Compose setup for spawning test chains.
- `daemon/examples/`: Integration test examples (`crud`, `webhook`) run against a live daemon.
- `Dockerfile`: Instructions for building a Docker image of the daemon.

For AI agents and detailed architecture, see [AGENTS.md](AGENTS.md) and `docs/`.

### Configuration File Example

You can use `.json` files or environment variables for daemon configuration.
Required configs are:
- `chain.json`: `name`, `endpoints` and `assets` fields are mandatory. `assets` can not be reconfigured over env vars;
- `payments.json`: only `recipient` field is mandatory;
- `seed.json`: only `seed` field is mandatory.

Non-required configs are optional. If you don’t set them, default values will be used.

All config examples can be found in `configs` folder of this project.
Any config field (except `chain.json`'s `assets`) can be overridden using environment variables. If both value in `.json` file and env var present,
daemon will use the one from env var.
If any value is already set in env var it's not required to be present in config file.

In order to make daemon read some field from env var, var's name should be named in convenient `{ENV_PREFIX}{CONFIG_FILE_NAME}{CONFIG_FIELD_NAME}={CONFIG_VALUE}`.

Default `ENV_PREFIX` is `KALATORI`, so to set `recipient` field of `payments` config you can use the following sentence:
```sh
export KALATORI_PAYMENTS_RECIPIENT=your_recipient_here
```
`ENV_PREFIX` also can be overridden using env var `KALATORI_APP_ENV_PREFIX`. For example if you set the prefix to `MY_SUPER_KALATORI` using `export KALATORI_APP_ENV_PREFIX=MY_SUPER_KALATORI` then you can use following sentence to override config from previous example:
```sh
export MY_SUPER_KALATORI_RECIPIENT=your_recipient_here
```

### Usage Example

For development and testing purposes Kalatori can be configured to connect to `chopsticks` instead of real chain.
In order to run Kalatori with `chopsticks` connection follow next steps:
1. Copy configs from example files:
```sh
make copy-configs
```
2. Run `chopsticks` in docker and build and run Kalatori daemon locally:
```sh
make run
```
3. When you finished, clean up `chopsticks` containers:
```sh
make stop-chopsticks
```

Another way is to run Kalatori for the Asset Hub parachain (without chopsticks):
1. We still can copy example configs but also use real chain RPC nodes:
```sh
make copy-configs
make copy-ah-production-config
```
2. Feel free to update any configs you need. After that we're ready to run Kalatori daemon:
```sh
make run-release
```

### Testing

Integration tests verify the daemon's functionality by running Rust examples against a live instance:

1. Start the daemon with Chopsticks:
   ```sh
   make run
   ```
2. Run integration examples:
   ```sh
   make run-test-examples
   ```

The daemon listens on `localhost:16726` by default.

### Contributing

We welcome contributions! Please refer to the [CONTRIBUTING.md](CONTRIBUTING.md) file for guidelines on contributing and submitting pull requests.

### License

Kalatori is open-source software licensed under the GPLv3 License. See the [LICENSE](LICENSE) file for more details.

### Community and Support

Join the discussion and get support on:
- [Kalatori Matrix](https://matrix.to/#/#Kalatori-support:matrix.zymologia.fi)
- [GitHub Discussions](https://github.com/Kalapaja/Kalatori/discussions)

### Roadmap

Refer to the Kalatori project [board](https://github.com/orgs/Kalapaja/projects/2) and [milestones](https://github.com/Kalapaja/Kalatori/milestones) for the current roadmap and upcoming features.

### Acknowledgments

- Polkadot community
- Liberland team
