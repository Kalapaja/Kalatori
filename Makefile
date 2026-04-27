.PHONY: help

# absolute path to this makefile (computed before includes so MAKEFILE_LIST has
# only the top-level Makefile)
mkfile_path := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

# Tool versions live in dedicated files so CI can hash each one independently
# as a cache key (tool bumps and front-end bumps invalidate different caches).
include tools.mk
include front-end.mk

help: # Show help for each of the Makefile recipes
	@grep -E '^[a-zA-Z0-9 -]+:.*#'  Makefile | sort | while read -r l; do printf "\033[1;32m$$(echo $$l | cut -f 1 -d':')\033[00m:$$(echo $$l | cut -f 2- -d'#')\n"; done

#####################
### Setup Project ###
#####################

install-cargo-binstall: # Install cargo-binstall (used by other install targets to fetch prebuilt binaries)
	@if ! command -v cargo-binstall >/dev/null 2>&1; then \
		curl -L --proto '=https' --tlsv1.2 -sSf \
			https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash; \
	fi

install-subxt-cli: install-cargo-binstall # Install subxt-cli into the project directory
	cargo binstall --root $(mkfile_path) --version $(subxt_cli_version) --locked --no-confirm subxt-cli

install-sqlx-cli: install-cargo-binstall # Install sqlx-cli into the project directory
	cargo binstall --root $(mkfile_path) --version $(sqlx_cli_version) --locked --no-confirm --no-default-features --features sqlite,completions sqlx-cli

install-nextest: install-cargo-binstall # Install cargo-nextest into the project directory
	cargo binstall --root $(mkfile_path) --version $(nextest_version) --locked --no-confirm cargo-nextest

install-llvm-cov: install-cargo-binstall # Install llvm-cov into the project directory
	cargo binstall --root $(mkfile_path) --version $(llvm_cov_version) --locked --no-confirm cargo-llvm-cov

install-mutants: install-cargo-binstall # Install cargo-mutants into the project directory
	cargo binstall --root $(mkfile_path) --version $(mutants_version) --locked --no-confirm cargo-mutants

install-insta: install-cargo-binstall # Install cargo-insta for snapshot test review
	cargo binstall --root $(mkfile_path) --version $(insta_version) --locked --no-confirm cargo-insta

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
	mv dist/* .; \
	rm -r dist; \
	rm payment-page-v$(front_end_version).zip

setup: install-subxt-cli download-node-metadata copy-configs # Sets up the project for local run
	echo "Make sure you have SQLite installed. Check README.md for the instructions"

setup-utils: install-nextest install-llvm-cov install-insta install-mutants # Sets up different utilities for running tests, coverage etc which are not required for the project run
	echo "Installed nextest, llvm-cov, insta and cargo-mutants"

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

run-dev: start-chopsticks # Run kalatori daemon with dev_api feature (enables /dev endpoints and auto-auth)
	cargo run --features dev_api

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

insta-review: # Interactively review pending snapshots
	PATH="${PWD}/bin:${PATH}" cargo insta review

insta-accept: # Accept all pending snapshots
	PATH="${PWD}/bin:${PATH}" cargo insta accept

insta-test: # Run tests and review pending snapshots
	PATH="${PWD}/bin:${PATH}" cargo insta test --review

generate-hmac-test-vectors: # Generate HMAC test vectors for the webhook simulator
	cargo run --example generate_hmac_test_vectors -p kalatori-client

generate-coverage-report: # Generate test coverage report as lcov.info
	PATH="${PWD}/bin:${PATH}" cargo llvm-cov nextest -p kalatori --lcov --output-path lcov.info

open-coverage-report: # Generate and open test coverage report
	PATH="${PWD}/bin:${PATH}" cargo llvm-cov nextest -p kalatori --open
