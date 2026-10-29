# Makefile for the varswap workspace. Targets mirror matchbook's; `pre-push` follows
# rules/global_rules.md (fmt check, clippy -D warnings, lint control, tests, docs -D warnings).

# sBPF target version (cargo-build-sbf's default is v3).
SBF_ARCH ?= v3

# Milestone 3a: the program only builds with the integer stand-in kernel, a test-only feature
# (fail closed: the default build is a compile error, checked by sbf-control).
PROGRAM_FEATURES = --features insecure-test-kernel

.PHONY: all
all: fmt-check lint test

# Build the workspace (all features: the program needs its test kernel until milestone 1b)
.PHONY: build
build:
	cargo build --workspace --all-features

# Build the on-chain crates for SBF (proves they stay no_std with no host-only dependency);
# fails on any stack frame over 4 KiB (scripts/sbf-build.sh).
.PHONY: sbf
sbf:
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh pyth-lite/Cargo.toml
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh varswap-core/Cargo.toml
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh program/Cargo.toml $(PROGRAM_FEATURES)

# Build the program into target/deploy/varswap_program.so (the LiteSVM tests load it)
.PHONY: program
program:
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh program/Cargo.toml $(PROGRAM_FEATURES)

# Run the program in LiteSVM and regenerate docs/benchmarks.md
.PHONY: bench
bench:
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh program/Cargo.toml --no-default-features $(PROGRAM_FEATURES)
	cp target/deploy/varswap_program.so target/deploy/varswap_program_no_events.so
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh program/Cargo.toml $(PROGRAM_FEATURES)
	VARSWAP_NO_EVENTS_SO=$(CURDIR)/target/deploy/varswap_program_no_events.so \
	VARSWAP_SBF_TOOLCHAIN="$$(cargo build-sbf --version 2>/dev/null | grep -v 'installed with' | paste -sd ',' - | sed 's/,/, /g'), sBPF $(SBF_ARCH)" \
	VARSWAP_BENCH_OUT=$(CURDIR)/docs/benchmarks.md \
	cargo test -p varswap-program --release --test cu -- --nocapture

# Failing controls for scripts/sbf-build.sh: it must reject lint-control/stack-probe (a 6 KB
# frame, which cargo build-sbf only warns about) and a failing cargo (a missing manifest).
.PHONY: sbf-control
sbf-control:
	@out=$$(SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh lint-control/stack-probe/Cargo.toml 2>&1); \
	if [ $$? -eq 0 ] || ! printf '%s\n' "$$out" | grep -q 'stack frame over 4 KiB'; then \
		echo "sbf-control: the stack check accepted a 6 KB frame"; exit 1; fi
	@out=$$(SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh lint-control/no-such-crate/Cargo.toml 2>&1); \
	if [ $$? -eq 0 ]; then echo "sbf-control: a failing cargo passed"; exit 1; fi
	@out=$$(SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh program/Cargo.toml 2>&1); \
	if [ $$? -eq 0 ] || ! printf '%s\n' "$$out" | grep -q 'no production kernel'; then \
		echo "sbf-control: the default program build did not fail closed"; exit 1; fi
	@echo "sbf-control: rejects an oversized frame, a failing cargo and a default program build"

# Run every test (the program tests need `make program` first; `pre-push` runs `sbf` before)
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
pre-push: fmt-check lint lint-control sbf sbf-control test doc

.PHONY: clean
clean:
	cargo clean
