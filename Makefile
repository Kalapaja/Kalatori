.PHONY: help

# absolute path to this makefile
mkfile_path := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

# Keep in sync with subxt version in Cargo.toml
subxt_cli_version := 0.44.0

# Keep in sync with sqlx version in Cargo.toml
sqlx_cli_version := 0.8.6

# Front end release version compatible with current daemon version
front_end_version := 0.0.4

help: # Show help for each of the Makefile recipes
	@grep -E '^[a-zA-Z0-9 -]+:.*#'  Makefile | sort | while read -r l; do printf "\033[1;32m$$(echo $$l | cut -f 1 -d':')\033[00m:$$(echo $$l | cut -f 2- -d'#')\n"; done

#####################
### Setup Project ###
#####################

install-subxt-cli: # Install subxt-cli into the project directory
	cargo install --root $(mkfile_path) --version $(subxt_cli_version) --locked subxt-cli

install-sqlx-cli: # Install sqlx-cli into the project directory
	cargo install --root $(mkfile_path) --version $(sqlx_cli_version) --locked sqlx-cli --no-default-features --features sqlite,completions

# TODO: read URL from json config and/or env var instead of hardcode
download-node-metadata: # Download metadata of configured Asset Hub node. Required for subxt compilation. By default use ws://localhost:9000 url.
	PATH="${PWD}/bin:${PATH}" subxt metadata -f bytes --url wss://asset-hub-polkadot-rpc.n.dwellir.com > metadata.scale

# TODO: read alternative value from env
download-node-metadata-ci: # Download metadata of Asset Hub node. Required for subxt compilation. By default use wss://polkadot-asset-hub-rpc.polkadot.io url.
	PATH="${PWD}/bin:${PATH}" subxt metadata -f bytes --url wss://asset-hub-polkadot-rpc.n.dwellir.com > metadata.scale

copy-configs: # Copy .example configs to actual configs
	cd configs; \
	for i in ./*.example; \
	do \
		cp "$$i" "$${i%.*}"; \
	done

download-front-end: # Download front-end release and unpack it into static folder
	mkdir -p static; \
	cd static; \
	curl -LfO https://github.com/Kalapaja/Kassette/releases/download/v$(front_end_version)/payment-page-v$(front_end_version).zip; \
	unzip payment-page-v$(front_end_version).zip; \
	mkdir -p assets; \
	mv dist/index.html .; \
	cp -r dist/* assets/; \
	rm -r dist; \
	rm payment-page-v$(front_end_version).zip

setup: install-subxt-cli download-node-metadata copy-configs # Sets up the project for local run
	echo "Make sure you have SQLite installed. Check README.md for the instructions"

#####################
### Build and run ###
#####################

sqlx-create-db: # Create an empty SQLite database file
	PATH="${PWD}/bin:${PATH}" sqlx db create --database-url sqlite:./database/kalatori_db.sqlite

sqlx-migrate: # Run database migrations using sqlx-cli
	PATH="${PWD}/bin:${PATH}" sqlx migrate run --database-url sqlite:./database/kalatori_db.sqlite

sqlx-prepare: # Prepare sqlx for compile-time verification of SQL queries
	PATH="${PWD}/bin:${PATH}" cargo sqlx prepare --database-url sqlite:./database/kalatori_db.sqlite

build-release: # Build the daemon with --release flag
	cargo build --release

start-chopsticks: # Start chopsticks for Asset Hub in docker compose with port-forwarding
	cd chopsticks; \
	docker compose up -d

stop-chopsticks: # Stop chopsticks for Asset Hub in docker compose
	cd chopsticks; \
	docker compose down

# TODO: add some health check for chopsticks to avoid errors on connection while it's not initialized
run: start-chopsticks # Ensure that chopsticks is started and run kalatori daemon locally
	cargo run

run-release: # Run kalatori daemon with --release flag without starting chopsticks
	cargo run --release

run-test-examples:
	cargo run --example crud; \
    cargo run --example webhook

##############
### Checks ###
##############

cargo-check: # Run cargo check for all targets
	cargo check --all-targets --all-features

# Keep same as in CI
cargo-clippy: # Run cargo clippy checks
	RUSTFLAGS="-Dwarnings" cargo clippy --all-targets --all-features

cargo-fmt: # Run cargo fmt checks
	cargo +nightly fmt --all -- --check

cargo-deny: # Run cargo deny checks
	cargo deny -L error check

#############
### Tools ###
#############

generate-hmac-test-vectors: # Generate HMAC test vectors for the webhook simulator
	cargo run --example generate_hmac_test_vectors -p kalatori-client
