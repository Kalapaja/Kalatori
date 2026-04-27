# Tool versions for CI and local dev.
#
# This file is included from the top-level Makefile. CI hashes it as a cache key
# for the project-local `bin/` directory, so bumping any version here busts the
# tool cache while leaving unrelated caches (front-end, etc.) untouched.

# Keep in sync with subxt version in Cargo.toml
subxt_cli_version := 0.44.0

# Keep in sync with sqlx version in Cargo.toml
sqlx_cli_version := 0.8.6

nextest_version := 0.9.133

llvm_cov_version := 0.8.4

mutants_version := 26.2.0

insta_version := 1.46.3
