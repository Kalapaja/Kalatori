## A Gateway Daemon for Kalatori

!!! KALATORI IS IN PUBLIC BETA !!!

Kalatori is an open-source daemon designed to enable secure and scalable blockchain payment processing. Licensed under GPLv3 ([LICENSE](LICENSE)), Kalatori currently supports assets on the Polkadot relay chain and its parachains.

The daemon derives unique accounts for each payment using a provided seed phrase and outputs all payments to a specified recipient wallet. It also offers limited transaction tracking for order management. Kalatori operates in a multithreaded mode and supports multiple currencies configured in a simple TOML-based configuration file.

Client facing frontends can communicate with Kalatori leveraging exposed API described in the [API documentation](https://alzymologist.github.io/kalatori-api).

---
### Download

Download the latest Docker container or x86-64 release from the [GitHub releases page](https://github.com/Kalapaja/Kalatori/releases/latest).

---

### Compile from Source

To compile the daemon, ensure you have the latest stable version of the Rust compiler installed. Then, run:

```sh
cargo build --release --workspace
```
The compiled binaries will be located in the `target/release` path.

### Project Structure

- `chopsticks`: Contains configuration files for the Chopsticks tool and a Docker Compose setup for spawning Polkadot and AssetHub test chains.
- `configs`: Contains configuration files for supported chains and assets.
- `docs`: Includes project documentation.
- `src`: The source code for the Kalatori daemon.
- `tests`: Black-box test suite with a Docker Compose setup for testing the daemon.
- `Dockerfile`: Instructions for building a Docker image of the daemon.

### Configuration File Example

For Polkadot and Asset Hub chains, the configuration file should look like this:

```toml
account-lifetime = 604800000 # 1 week.
debug = true
depth = 86400000 # 1 day.

[[chain]]
name = "polkadot"
native-token = "DOT"
decimals = 10
endpoints = [
    "wss://rpc.polkadot.io",
    "wss://1rpc.io/dot",
]

[[chain]]
name = "statemint"
endpoints = [
    "wss://polkadot-asset-hub-rpc.polkadot.io",
    "wss://statemint-rpc.dwellir.com",
]

[[chain.asset]]
name = "USDC"
id = 1337

[[chain.asset]]
name = "USDt"
id = 1984
```

### Environment variables

Kalatori requires the following environment variables for configuration:
- `KALATORI_HOST`: Address for the daemon's TCP socket server.
- `KALATORI_SEED`: Seed phrase for account derivation.
- `KALATORI_CONFIG`: Path to the chain configuration file in the configs directory.
- `KALATORI_RECIPIENT`: The hexadecimal address to which received payments will be transferred.
- `KALATORI_REMARK`: A string added to the transaction's remark field.

### Usage Example

Run Kalatori for the Polkadot chain:

```sh
KALATORI_HOST="127.0.0.1:16726" \
KALATORI_CONFIG="configs/polkadot.toml" \
KALATORI_SEED="bottom drive obey lake curtain smoke basket hold race lonely fit walk" \
KALATORI_RECIPIENT="5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY" \
KALATORI_REMARK="test" \
kalatori
````

### Testing

The black-box test suite verifies the daemon's functionality by interacting with a running instance. Use the following steps to set it up:
1. Start the daemon and test environment:
   ```sh
   cd tests
   docker-compose up
   ```
2. Run the tests manually using Yarn:
   ```sh
   ct tests/kalatori-api-test-suite
   yarn
   yarn test
   ```

Ensure the `DAEMON_HOST` environment variable points to the running daemon (default: `localhost:16726`).

For more details, refer to the [testing suite README.md](tests/kalatori-api-test-suite/README.md).

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

