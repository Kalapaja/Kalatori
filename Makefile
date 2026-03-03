.PHONY: help

# absolute path to this makefile
mkfile_path := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

# Keep in sync with subxt version in Cargo.toml
subxt_cli_version := 0.44.0

# Keep in sync with sqlx version in Cargo.toml
sqlx_cli_version := 0.8.6

nextest_version := 0.9.129

llvm_cov_version := 0.8.4

mutants_version := 26.2.0

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

install-nextest: # Install cargo-nextest into the project directory
	cargo install --root $(mkfile_path) --version $(nextest_version) --locked cargo-nextest

install-llvm-cov: # Install llvm-cov into the project directory
	cargo install --root $(mkfile_path) --version $(llvm_cov_version) --locked cargo-llvm-cov

install-mutants: # Install cargo-mutants into the project directory
	cargo install --root $(mkfile_path) --version $(mutants_version) --locked cargo-mutants

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

setup-utils: install-nextest install-llvm-cov install-mutants # Sets up different utilities for running tests, coverage etc which are not required for the project run
	echo "Installed nextest, llvm-cov and cargo-mutants"

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

cargo-test: # Run cargo tests using nextest
	PATH="${PWD}/bin:${PATH}" cargo nextest run

cargo-check: # Run cargo check for all targets
	cargo check --all-targets --all-features

# Keep same as in CI
cargo-clippy: # Run cargo clippy checks
	RUSTFLAGS="-Dwarnings" cargo clippy --all-targets --all-features

cargo-fmt: # Run cargo fmt checks
	cargo +nightly fmt --all -- --check

cargo-deny: # Run cargo deny checks
	cargo deny -L error check

cargo-mutants-for-diff: # Run cargo mutants for git diff
	git diff | PATH="${PWD}/bin:${PATH}" cargo mutants --test-tool=nextest --in-diff /dev/stdin

#############
### Tools ###
#############

cargo-fmt-apply: # Apply cargo fmt style changes
	cargo +nightly fmt --all

generate-hmac-test-vectors: # Generate HMAC test vectors for the webhook simulator
	cargo run --example generate_hmac_test_vectors -p kalatori-client

generate-coverage-report: # Generate test coverage report as lcov.info
	PATH="${PWD}/bin:${PATH}" cargo llvm-cov nextest -p kalatori --lcov --output-path lcov.info

open-coverage-report: # Generate and open test coverage report
	PATH="${PWD}/bin:${PATH}" cargo llvm-cov nextest -p kalatori --open

######################
### Documentation  ###
######################

install-mkdocs: # Install mkdocs with material theme and mike into local .venv
	python3 -m venv .venv
	.venv/bin/pip install mkdocs-materialx mike

docs-serve: # Serve documentation locally with live reload
	.venv/bin/mkdocs serve --livereload -o

docs-build: # Build documentation locally
	.venv/bin/mkdocs build
