# Makefile for the varswap workspace. Targets mirror matchbook's; `pre-push` follows
# rules/global_rules.md (fmt check, clippy -D warnings, lint control, tests, docs -D warnings).
# `program` and `bench` (LiteSVM CU table) join `pre-push` with milestone 3.

# sBPF target version (cargo-build-sbf's default is v3).
SBF_ARCH ?= v3

.PHONY: all
all: fmt-check lint test

# Build the workspace
.PHONY: build
build:
	cargo build --workspace

# Build the on-chain crates for SBF (proves they stay no_std with no host-only dependency)
.PHONY: sbf
sbf:
	cargo build-sbf --manifest-path pyth-lite/Cargo.toml --arch $(SBF_ARCH)
	cargo build-sbf --manifest-path varswap-core/Cargo.toml --arch $(SBF_ARCH)

# Run every test
.PHONY: test
test:
	cargo test --workspace --all-features

# Format the code
.PHONY: fmt
fmt:
	cargo +stable fmt --all

# Check formatting
.PHONY: fmt-check
fmt-check:
	cargo +stable fmt --all --check

# Clippy with every warning denied
.PHONY: lint
lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

# Failing control: clippy must reject every banned macro in lint-control's library code, on
# each probe line with the exact definition path (see lint-control/check.sh).
.PHONY: lint-control
lint-control:
	@sh lint-control/check.sh

# Rustdoc with every warning denied
.PHONY: doc
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# Re-fetch the PriceUpdateV2 fixtures from mainnet (overwrites pyth-lite/fixtures)
.PHONY: fixtures
fixtures:
	python3 pyth-lite/fixtures/fetch.py

# Pre-push checks: must pass with zero warnings
.PHONY: pre-push
pre-push: fmt-check lint lint-control sbf test doc

.PHONY: clean
clean:
	cargo clean
