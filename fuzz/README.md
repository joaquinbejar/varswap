# Fuzz targets

cargo-fuzz targets for the on chain code paths (`docs/PLAN-improvements.md`, milestone I3).
This crate is its own workspace, so the repository workspace never builds it on stable; it
needs a nightly toolchain and `cargo install --locked cargo-fuzz`.

| Target | Code under test | Invariants |
| --- | --- | --- |
| `pyth_update` | `pyth_lite::PriceUpdate::{load, decode, verify}` on real mainnet accounts with byte edits, and on raw bytes | no panic; an accepted account re-encodes to its bytes; a wrong owner or any changed discriminator byte is rejected; `verify` agrees with a reference predicate |
| `window_sample` | `varswap_core::Window::{sample, finalize}` with `LnKernel` on arbitrary windows and prints | a rejected print writes nothing; filled buckets never change; the missing count; `sum_sq` equals the sum of the returns over consecutive filled buckets; `Finalized` exactly when full, `Degraded` only after the deadline |
| `swap_conservation` | `varswap_core::SwapTerms` | collateral is the exact ceiling; `long + short == total` for every settlement; `Unwind` returns each collateral; the long payout is monotone, equal to its collateral at the strike, flat above the cap |
| `sample_ix` | the program's `sample` in LiteSVM, with arbitrary price update accounts and times | the transaction succeeds exactly when the off chain path (`pyth-lite`, `accept_print`, `Window::sample_accepted`) accepts, and the window account then equals the off chain one byte for byte; on failure it is unchanged |

```bash
make program                          # sample_ix loads target/deploy/varswap_program.so
make fuzz FUZZ_SECONDS=120            # every target on its corpus (fuzz/corpus/<target>)
cargo +nightly fuzz run -O window_sample -- -max_total_time=1800
make fuzz-control                     # the deterministic fault controls
```

The harness asserts by panicking: it is test code.

## Fault controls

`fuzz/controls/<name>.patch` plants one bug and `<name>.input` is a committed input that
exposes it. `make fuzz-control` (`fuzz/controls/run.sh`) copies the working tree to
`target/fuzz-control/run`, runs each input on the unpatched code (it must pass), applies the
patch (rebuilding the program for `sample_ix`) and runs the input again (it must panic).
Running one input executes it once, without mutation, so the controls are deterministic.

| Control | Target | Planted bug |
| --- | --- | --- |
| `pyth_partial_accepted` | `pyth_update` | `verify` accepts a `Partial` verification level |
| `swap_long_collateral_rounds_down` | `swap_conservation` | the long collateral rounds down instead of up |
| `window_writes_before_refill_check` | `window_sample` | `fill` writes the first bucket before rejecting a refill |
| `program_skips_confidence` | `sample_ix` | the program skips the confidence bound |

CI (`.github/workflows/ci.yml`, job `fuzz`) runs every target 120 s on pull requests and
30 minutes each on a weekly schedule, then the controls, on a pinned nightly and cargo-fuzz.
