# Makefile for the varswap workspace. Targets mirror matchbook's; `pre-push` follows
# rules/global_rules.md (fmt check, clippy -D warnings, lint control, tests, docs -D warnings).

# sBPF target version (cargo-build-sbf's default is v3).
SBF_ARCH ?= v3

# Host toolchain for rustfmt (`+$(RUST_TOOLCHAIN)`): `stable` locally, the exact pin in CI,
# since RUSTUP_TOOLCHAIN does not override an explicit `+stable`.
RUST_TOOLCHAIN ?= stable

# Extra cargo flags for every cargo command: empty locally, `--locked` in CI. Exported so
# scripts/sbf-build.sh and lint-control/check.sh pass them too.
CARGO_FLAGS ?=
export CARGO_FLAGS

.PHONY: all
all: fmt-check lint test

# Build the workspace
.PHONY: build
build:
	cargo build $(CARGO_FLAGS) --workspace

# Build the on-chain crates for SBF (proves they stay no_std with no host-only dependency);
# fails on any stack frame over 4 KiB (scripts/sbf-build.sh).
.PHONY: sbf
sbf:
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh pyth-lite/Cargo.toml
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh varswap-core/Cargo.toml
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh program/Cargo.toml

# Build the program into target/deploy/varswap_program.so (the LiteSVM tests load it)
.PHONY: program
program:
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh program/Cargo.toml

# Run the program in LiteSVM and regenerate docs/benchmarks.md
.PHONY: bench
bench:
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh program/Cargo.toml --no-default-features
	cp target/deploy/varswap_program.so target/deploy/varswap_program_no_events.so
	@SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh program/Cargo.toml
	VARSWAP_NO_EVENTS_SO=$(CURDIR)/target/deploy/varswap_program_no_events.so \
	VARSWAP_SBF_TOOLCHAIN="$$(cargo build-sbf --version 2>/dev/null | grep -v 'installed with' | paste -sd ',' - | sed 's/,/, /g'), sBPF $(SBF_ARCH)" \
	VARSWAP_BENCH_OUT=$(CURDIR)/docs/benchmarks.md \
	cargo test $(CARGO_FLAGS) -p varswap-program --release --test cu -- --nocapture

# Failing controls for scripts/sbf-build.sh: it must reject lint-control/stack-probe (a 6 KB
# frame, which cargo build-sbf only warns about) and a failing cargo (a missing manifest).
.PHONY: sbf-control
sbf-control:
	@out=$$(SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh lint-control/stack-probe/Cargo.toml 2>&1); \
	if [ $$? -eq 0 ] || ! printf '%s\n' "$$out" | grep -q 'stack frame over 4 KiB'; then \
		echo "sbf-control: the stack check accepted a 6 KB frame"; exit 1; fi
	@out=$$(SBF_ARCH=$(SBF_ARCH) sh scripts/sbf-build.sh lint-control/no-such-crate/Cargo.toml 2>&1); \
	if [ $$? -eq 0 ]; then echo "sbf-control: a failing cargo passed"; exit 1; fi
	@echo "sbf-control: rejects an oversized frame and a failing cargo"

# Run every test (the program tests need `make program` first; `pre-push` runs `sbf` before)
.PHONY: test
test:
	cargo test $(CARGO_FLAGS) --workspace --all-features

# Format the code
.PHONY: fmt
fmt:
	cargo +$(RUST_TOOLCHAIN) fmt --all

# Check formatting
.PHONY: fmt-check
fmt-check:
	cargo +$(RUST_TOOLCHAIN) fmt --all --check

# Clippy with every warning denied
.PHONY: lint
lint:
	cargo clippy $(CARGO_FLAGS) --workspace --all-targets --all-features -- -D warnings

# Failing control: clippy must reject every banned macro in lint-control's library code, on
# each probe line with the exact definition path (see lint-control/check.sh).
.PHONY: lint-control
lint-control:
	@sh lint-control/check.sh

# Rustdoc with every warning denied
.PHONY: doc
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc $(CARGO_FLAGS) --workspace --no-deps --all-features

# Run the milestone 5 outage and pusher study and regenerate docs/study.md (fixed seeds)
.PHONY: study
study:
	cargo run $(CARGO_FLAGS) --release -p varswap-study > docs/study.md

# Re-fetch the PriceUpdateV2 fixtures from mainnet (overwrites pyth-lite/fixtures)
.PHONY: fixtures
fixtures:
	python3 pyth-lite/fixtures/fetch.py

# Verifiable build (docs/verifiable.md, docs/PLAN-improvements.md I2): `solana-verify build` in
# the pinned image (platform-tools v1.57, Rust 1.95.0: the SBF compiler CI pins), then the
# executable hash. `verifiable-check` fails unless it equals the build hash recorded in
# docs/verifiable.md. Needs Docker and `cargo install --locked solana-verify --version 0.5.2`.
# It writes target/deploy/varswap_program.so: run `make program` again before the tests.
VERIFY_IMAGE ?= solanafoundation/solana-verifiable-build@sha256:12fd4c0a0790f0fc41ef74b0cdb6bccc167ba6137eb1adb749bac649481c86bd
.PHONY: verifiable
verifiable:
	solana-verify build -b $(VERIFY_IMAGE) --library-name varswap_program --arch $(SBF_ARCH)
	@solana-verify get-executable-hash target/deploy/varswap_program.so

.PHONY: verifiable-check
verifiable-check: verifiable
	@got=$$(solana-verify get-executable-hash target/deploy/varswap_program.so); \
	want=$$(sed -n 's/^| Build hash | `\([0-9a-f]*\)` |$$/\1/p' docs/verifiable.md); \
	if [ -z "$$want" ] || [ "$$got" != "$$want" ]; then \
		echo "verifiable-check: build hash $$got, docs/verifiable.md records '$$want'"; exit 1; fi; \
	echo "verifiable-check: build hash $$got matches docs/verifiable.md"

# Fuzz every target for FUZZ_SECONDS each on its committed corpus (fuzz/, cargo-fuzz, nightly;
# docs/PLAN-improvements.md I3). sample_ix loads the program: `make program` first.
FUZZ_TOOLCHAIN ?= nightly
FUZZ_SECONDS ?= 120
FUZZ_TARGETS = pyth_update window_sample swap_conservation sample_ix
.PHONY: fuzz
fuzz: program
	@for t in $(FUZZ_TARGETS); do \
		echo "fuzz: $$t for $(FUZZ_SECONDS) s"; \
		cargo +$(FUZZ_TOOLCHAIN) fuzz run -O $$t -- -max_total_time=$(FUZZ_SECONDS) || exit 1; \
	done

# Formatting of the fuzz crate (its own workspace, so `fmt-check` does not reach it).
.PHONY: fuzz-fmt-check
fuzz-fmt-check:
	cargo +$(RUST_TOOLCHAIN) fmt --manifest-path fuzz/Cargo.toml -- --check

# Deterministic fault controls: each planted bug in fuzz/controls must crash its target on its
# committed input, and the input must pass without the bug (fuzz/controls/run.sh).
.PHONY: fuzz-control
fuzz-control:
	@FUZZ_TOOLCHAIN=$(FUZZ_TOOLCHAIN) sh fuzz/controls/run.sh

# Pre-push checks: must pass with zero warnings
.PHONY: pre-push
pre-push: fmt-check lint lint-control sbf sbf-control test doc

.PHONY: clean
clean:
	cargo clean
