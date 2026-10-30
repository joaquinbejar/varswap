# VarSwap

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/joaquinbejar/varswap/blob/main/LICENSE)
[![Stars](https://img.shields.io/github/stars/joaquinbejar/varswap.svg)](https://github.com/joaquinbejar/varswap/stargazers)
[![Issues](https://img.shields.io/github/issues/joaquinbejar/varswap.svg)](https://github.com/joaquinbejar/varswap/issues)
[![PRs](https://img.shields.io/github/issues-pr/joaquinbejar/varswap.svg)](https://github.com/joaquinbejar/varswap/pulls)

## VarSwap v0.1.0: Realized Variance and Variance Swaps on Solana

### Table of Contents
1. [Introduction](#introduction)
2. [Status](#status)
3. [Features](#features)
4. [Workspace Crates](#workspace-crates)
5. [Install](#install)
6. [On Devnet](#on-devnet)
7. [Accounts](#accounts)
8. [Instructions](#instructions)
9. [Variance Math and Rounding](#variance-math-and-rounding)
10. [Oracle Checks](#oracle-checks)
11. [Manipulation and Outage Study](#manipulation-and-outage-study)
12. [Setup Instructions](#setup-instructions)
13. [Usage Examples](#usage-examples)
14. [Testing and Benchmarks](#testing-and-benchmarks)
15. [Contribution and Contact](#contribution-and-contact)

### Introduction

VarSwap is a [Pinocchio](https://github.com/anza-xyz/pinocchio) program that does two things on Solana:

1. **A realized variance accumulator**: an account per (feed, window) that turns canonical Pyth
   pull oracle prints into a running sum of squared log returns over a fixed grid (for example
   30 days hourly), with explicit rules for sampling, confidence and gaps.
2. **Variance swaps** settled against a finished accumulator: notional, variance strike and cap,
   payoff `notional × (min(realized_var, cap_var) − strike_var)`, fully collateralized on both
   sides in USDC and settled at window end.

It also ships `pyth-lite`, a `no_std` zero copy reader for Pyth `PriceUpdateV2` accounts that
needs no Anchor. The design, every decision and the numbers behind them are in
[`docs/PLAN.md`](docs/PLAN.md).

Non goals for v1: margin calls, liquidations or mark to market (swaps are fully collateralized
instead); an order book or AMM (the two parties agree off chain and sign one `open_swap`
together); corridor, conditional, gamma or volatility swaps; pushing prices ourselves (anyone
posts Pyth updates through the Pyth receiver, varswap only reads them).

### Status

Milestones 1 to 5 are done (accumulator, `pyth-lite`, window program, swaps, CU benchmarks and
the outage study), and so are the three parts of milestone 6 that run off chain: `varswap-sdk`,
`varswap-keeper` and the `varswap-admin` CLI. What remains is the devnet run (a keeper pushing
samples for a live SOL/USD window until it finalizes, and a replay of 24 real canonical prints).
The program is deployed on **devnet only**, at `BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9`
([docs/devnet.md](docs/devnet.md): config, the SOL/USD feed config and one hourly window;
live sampling waits on the Hermes key). It is not on mainnet, and no crate is published on
crates.io.

### Features

- **Canonical sampling**: the sample for boundary `b_k = start + k × interval` is the unique Pyth
  print with `prev_publish_time < b_k <= publish_time`, so whoever pushes it has no choice.
- **Permissionless liveness**: anyone can create windows on vetted feeds, push samples, finalize
  windows, settle swaps and pay out claims.
- **Out of order backfill**: buckets are a bitmap filled in any order until
  `end + backfill_deadline`; each fill adds the returns to its filled neighbours (at most two
  `ln` calls per sample, none at finalize).
- **Strict missing data policy**: if any bucket is empty at the deadline the window is
  `Degraded` and every swap on it unwinds (each side gets its collateral back).
- **Fixed point math** on [optfix](https://github.com/joaquinbejar/optfix) `ln` (Q64.64) with a
  proven error bound applied in the short's favour, so the long is never overpaid.
- **Exact conservation**: `long_payout + short_payout == total collateral`; the vault never goes
  negative and keeps no dust.
- **Hardened code**: `unsafe_code`, `panic`, `unwrap`, `expect`, indexing, unchecked arithmetic,
  float arithmetic and integer division are all denied by workspace lints, with a failing
  control (`make lint-control`) that proves the lints fire.
- **Bounded compute**: every instruction under 17,000 CU (see [Testing and Benchmarks](#testing-and-benchmarks)).

### Workspace Crates

| Crate | What |
| --- | --- |
| `varswap-core` | `no_std` window grid, canonical print acceptance (`accept_print`, shared by the program and the keeper), bucket storage, the optfix `ln` kernel (`LnKernel`) and its error bound, swap terms (`SwapTerms`) and settlement, every account and payload layout |
| `pyth-lite` | `no_std` reader for Pyth `PriceUpdateV2` accounts: owner, discriminator, layout, Full verification, feed, price, staleness, confidence |
| `pyth-post` | off chain poster of Pyth prices from Hermes updates: the `PNAU` parser, Wormhole encoded VAA and receiver builders, and `post` (VAA written and verified, `Full` `PriceUpdateV2` posted, your instructions, every account closed) over a `Transport`; `RpcTransport` over `solana-rpc-client`. Tested against the devnet receiver and Wormhole programs with real Hermes updates |
| `program` (`varswap-program`) | the on chain Pinocchio program: config, feed configs, windows, swaps ([`docs/errors.md`](docs/errors.md), [`docs/benchmarks.md`](docs/benchmarks.md)) |
| `sdk` (`varswap-sdk`) | a builder per instruction (`ix`), PDAs (`pda`) and account decoders (`decode`); every builder matches the test harness encoding byte for byte |
| `keeper` (`varswap-keeper`) | `Keeper::tick` over the `Chain` and `PriceSource` traits, a Hermes source (`HermesSource`), the production `ReceiverChain` (posts through `pyth-post`, samples in the posting transaction) and the `varswap-keeper` binary |
| `cli` (`varswap-cli`) | the `varswap-admin` binary: thin admin CLI over the SDK |
| `study` (`varswap-study`) | off chain simulation study (`make study` writes [`docs/study.md`](docs/study.md)) |
| `lint-control` | failing controls for the lint set and the SBF stack check |

### Install

`pyth-lite`, `varswap-core` and `varswap-sdk` are on crates.io (0.1.0); the program, the
keeper and the CLI are built from this repository.

```toml
[dependencies]
pyth-lite = "0.1"      # no_std PriceUpdateV2 reader, no Anchor
varswap-core = "0.1"   # window grid, canonical prints, kernel, swap terms, account layouts
varswap-sdk = "0.1"    # instruction builders, PDAs and decoders for clients
```

`pyth-lite` and `varswap-core` are `no_std` and build for SBF. `varswap-sdk` builds
instructions with the `solana-address` 2.x and `solana-instruction` 3.x generation (the one
LiteSVM 0.18 uses).

### On Devnet

The program runs on devnet only; nothing is on mainnet. Full record, with every signature:
[`docs/devnet.md`](docs/devnet.md).

| What | Address |
| --- | --- |
| Program | [`BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9`](https://explorer.solana.com/address/BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9?cluster=devnet) |
| `Config` | [`H2H2gn4mwkWoJ8ftQf97hpKJuunRzhuVPn84ZzW58uLB`](https://explorer.solana.com/address/H2H2gn4mwkWoJ8ftQf97hpKJuunRzhuVPn84ZzW58uLB?cluster=devnet) |
| SOL/USD `FeedConfig` | [`85wokHgbLDTDqNeWhgBgD3hu2ZBM3r8qc1UnCUFixmEF`](https://explorer.solana.com/address/85wokHgbLDTDqNeWhgBgD3hu2ZBM3r8qc1UnCUFixmEF?cluster=devnet) |
| Window (SOL/USD, 2026-10-31, hourly) | [`7s5KJ79XRwBzeL8t42ut6RkQ1yLhrRMg8z5dHQtNXTeA`](https://explorer.solana.com/address/7s5KJ79XRwBzeL8t42ut6RkQ1yLhrRMg8z5dHQtNXTeA?cluster=devnet) |

Example transactions:
[`init_config`](https://explorer.solana.com/tx/5HV3i3dFWJMKPZBMJxViWpsjpB6GkdzQUp8Vg1SH4duRyKL1JsjsgHGgCa5hA7dFugvpEuMhRFcrTPvGsXLeQaEf?cluster=devnet),
[`set_feed_config`](https://explorer.solana.com/tx/5c2ssG2VB53XiugwEKNXbQVWRtTbyNKwZBAb979yrZXE6DMijLtHkSpcJxzLtDdwLUBXnvDi2PiiAG9t95SWaCDm?cluster=devnet),
[`create_window`](https://explorer.solana.com/tx/4sudzjBz4ZU6Bw5QNq9pVJPhVUoyoaN3mg2gPMW36GKL5PymMeS5MWeSbs6UJiFR8tKBF9Wi44MhEr6UwtRJMdY9?cluster=devnet).
A demo window
([`9P3vhP8pe8WcebFXZ77njPHddnCtLLFGFHLiAucQ2Wz8`](https://explorer.solana.com/address/9P3vhP8pe8WcebFXZ77njPHddnCtLLFGFHLiAucQ2Wz8?cluster=devnet),
ten 60 s intervals) was sampled live by `varswap-keeper` and finalized: one
[post with `sample`](https://explorer.solana.com/tx/3QaW1FW6EkTR3AZmeqwnFmbXLH8eBTNuRrBmh3oEKBSaXjFPqcDjv6BwE8QutoL4rzaJQM1fNNzSUvnyX2g6pcNF?cluster=devnet)
(`post_update`, `sample`, `reclaim_rent`, `close_encoded_vaa`) and the
[`finalize`](https://explorer.solana.com/tx/4HoUsuSYMEvY4ydVsapcciXT2M9rXEk8e4mHRu45WnYg3EdiNUotMHnGhs3zCUodGmUiq4dNFjsM2F1idcrhXU3t?cluster=devnet).

To reproduce against `api.devnet.solana.com` (a funded keypair; the program id keypair is
yours):

```shell
export SOLANA_URL=https://api.devnet.solana.com
export VARSWAP_KEYPAIR=~/.config/solana/id.json         # upgrade authority, then admin
make program
solana program deploy --url devnet --upgrade-authority "$VARSWAP_KEYPAIR" \
  --program-id <program-id-keypair> target/deploy/varswap_program.so
export VARSWAP_PROGRAM_ID=<program id>
cargo run -p varswap-cli --release -- init-config --collateral-mint 4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU
cargo run -p varswap-cli --release -- set-feed-config \
  --feed ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d \
  --min-interval 60 --tolerance 21600 --backfill-deadline 86400 --max-buckets 1105 \
  --max-conf 1000 --exponent -8
cargo run -p varswap-cli --release -- create-window \
  --feed ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d \
  --start <future b_0> --end <b_N> --interval 3600
cargo run -p varswap-cli --release -- show-window <window address>
```

The keeper reads the Hermes API key from `VARSWAP_HERMES_API_KEY` only; it is never passed
on the command line, written to a file or printed.

### Accounts

| Account | Seeds | Size | Written by |
| --- | --- | --- | --- |
| `Config` | `["config"]` | header + admin + collateral mint | `init_config` (admin); the collateral mint is immutable |
| `FeedConfig` | `["feed", feed_id]` | 96 B | admin: receiver program id, `min_interval`, `tolerance`, `max_conf`, `backfill_deadline`, `max_buckets`, pinned exponent |
| `Window` | `["window", feed_id, start_le, end_le, interval_le]` | `144 + 8 × ceil(B / 64) + 9 × B` for `B = N + 1` buckets: 6,729 B for 721 buckets | `create_window`, `sample`, `finalize` (anyone) |
| `Swap` | `["swap", window, long, short, swap_index_le]` | 288 B | `open_swap`, `settle_swap`, `claim_*` |
| Collateral vault | `["vault", swap]` (SPL Token account) | 165 B | token CPIs only |

A window copies its feed's risk parameters at creation and a swap stores its collateral mint and
token program; `sample`, `settle_swap` and `claim_*` read only those copies, never mutable global
config. Accounts created through a CPI are capped at 10,240 bytes, so `MAX_BUCKETS = 1_105`
(10,233 bytes).

### Instructions

| Tag | Instruction | Signers | What it does |
| ---: | --- | --- | --- |
| 0 | `init_config` | admin (the program's upgrade authority), payer | creates `Config`; checks the real `ProgramData` so it cannot be front run |
| 1 | `set_feed_config` | admin, payer on create | creates or overwrites a feed's risk parameters |
| 2 | `create_window` | payer | creates a window on a vetted feed; `start` in the future, `(end − start) % interval == 0`, `interval >= min_interval`, `N + 1 <= max_buckets` |
| 3 | `sample` | anyone | fills the run of buckets a canonical print covers; logs an event per accepted sample |
| 4 | `finalize` | anyone | `Finalized` at once when every bucket is filled, otherwise `Degraded` only after `end + backfill_deadline` |
| 5 | `open_swap` | long, short, payer | both sides deposit collateral before `window.start`; amounts recomputed on chain |
| 6 | `settle_swap` | anyone | computes and stores both payouts; moves no tokens |
| 7 | `claim_long` | anyone | pays the long side only |
| 8 | `claim_short` | anyone | pays the short side only, so a frozen account on one side never blocks the other |
| 9 | `close_swap` | anyone, after both claims | closes the vault and the swap, rent back to the payer |

Error codes: 100 to 139 program validation, `140 +` `PythError`, `160 +` `CoreError`, `220 +`
kernel payload; the full table is [`docs/errors.md`](docs/errors.md).

### Variance Math and Rounding

- Each squared log return is `r = ln(p_to) − ln(p_from) + (e_to − e_from) × ln 10`, each `ln`
  on a price mantissa (so no ratio can overflow), with `r²` rounded down.
- `sum_sq` is accumulated at `2^-96` (each term `floor(|r_raw|² / 2^32)` through the full
  256 bit product, `optfix::wide::mul_shr`).
- A complete window annualizes on elapsed time: `realized_var = sum_sq × YEAR_SECONDS / (b_N − b_0)`,
  `YEAR_SECONDS = 31_536_000`, rounded down.
- Error bound: over a complete window the realized variance error is at most
  `ε(interval, cap) = ceil(2 δr √(cap × Y) / 2^32) + ceil(3 Y / 2^32) + 2` Q64.64 raw units,
  `Y = ceil(YEAR_SECONDS / interval)`, `δr = 4` with the exponent pinned; it does not depend on
  the number of buckets. With `cap_var = 25` (500% vol), ε is 3,747 raw units at 3,600 s
  (`2.0 × 10^-16` variance).
- Payouts settle on the conservative variance `V_cons = max(0, V − ε)`, taken before the cap and
  the floor, so **the long is never overpaid**; the short's advantage is under three atoms per
  swap. `open_swap` rejects `notional × ε >= 2^64`.
- Collateral: `long_collateral = notional × strike_var` and
  `short_collateral = notional × (cap_var − strike_var)`, both rounded up, with
  `cap_var > strike_var > 0`. `long_payout = long_collateral + pnl_long`, rounded down and
  clamped to `[0, total]`; `short_payout = total − long_payout`. Rounding dust (at most 3 atoms)
  ends with the short.
- Strikes and caps are Q64.64 annualized variance on chain (0.04 is 20% vol); the study
  tables quote variance points (variance × 10,000).

### Oracle Checks

`sample` accepts a `PriceUpdateV2` account only if:

- it is owned by the Pyth receiver program pinned in the window (mainnet
  `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`, `pyth_lite::RECEIVER_PROGRAM_ID`) and carries
  the `PriceUpdateV2` discriminator;
- `verification_level == Full`;
- `feed_id` equals the window's feed and the exponent equals the one pinned in its `FeedConfig`;
- `price > 0` and `conf × 10_000 <= price × max_conf` (computed in `u128`; a loose sanity bound,
  v1 uses 10%);
- `prev_publish_time > 0` (the first print after a Pyth `init_price` is rejected);
- the print is canonical for at least one empty bucket: it fills every `k` with
  `prev_publish_time < b_k <= publish_time <= b_k + tolerance` (the outage guard, v1 6 h);
- `now <= end + backfill_deadline` and the window is not final.

There is no staleness bound: canonical prints can be posted late (Hermes serves historical
updates by timestamp), so there is no hindsight to exploit.

### Manipulation and Outage Study

### Why canonical prints

The sample for each grid boundary is the unique Pyth print with `prev_publish_time < b <=
publish_time`, so the pusher has no choice. Without that rule a pusher accepting any print
within `w` seconds after each boundary could steer the payoff (hindsight optimum over 7
candidate prints per boundary, exact by dynamic programming, on one coherent GBM path, σ = 80%, 30 days hourly; the control
column, a fixed shift of every print by `w`, is zero as it should be; `docs/study.md`):

| w | Long gain mean (vp) | Long gain p95 (vp) | Short gain mean (vp) | Short gain p95 (vp) | Long gain mean (USDC) | Control: fixed shift mean (vp) |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 s | +463.77 | +507.17 | +439.76 | +484.39 | +46377 | +0.25 ± 1.11 |
| 60 s | +1174.85 | +1290.73 | +1038.61 | +1143.66 | +117485 | -0.26 ± 2.76 |
| 300 s | +2775.74 | +3029.09 | +2130.50 | +2338.38 | +277574 | +0.24 ± 5.91 |

(vp: variance points, variance × 10,000; USDC per 1,000,000 USDC of notional per unit of
variance; the latent variance is about 6,400 vp.)

### Outages

A Pyth halt makes the first print after it canonical for every boundary it crossed, so the
halt's move settles as one return (the contract's payoff), which differs from the variance the
full grid would have seen; a boundary waiting longer than the outage guard has no canonical
print and the window unwinds. Unwind frequency / mean error (vp) per guard, 4,000 windows per
model, σ = 80%, assumed outage regime (`docs/study.md` has the full distributions):

| Model | 1 h | 3 h | 6 h | 12 h | 24 h |
| --- | ---: | ---: | ---: | ---: | ---: |
| GBM, μ = 0 | 64.40% / +1.08 | 12.40% / -0.01 | 0.62% / +0.34 | 0.00% / +0.28 | 0.00% / +0.28 |
| GBM, μ = +100% | 62.00% / +0.39 | 12.38% / +1.11 | 0.53% / +1.24 | 0.00% / +1.44 | 0.00% / +1.44 |
| AR(1), ρ = +0.3 | 63.70% / +3.91 | 12.20% / +13.38 | 0.62% / +16.14 | 0.00% / +16.53 | 0.00% / +16.53 |
| AR(1), ρ = −0.3 | 63.58% / -4.55 | 12.65% / -10.42 | 0.83% / -12.30 | 0.00% / -12.54 | 0.00% / -12.54 |
| Halts, continuation | 89.28% / +16.24 | 43.43% / +95.35 | 10.30% / +211.95 | 0.53% / +300.79 | 0.00% / +312.35 |
| Halts, reversal | 87.78% / -13.07 | 43.65% / -70.68 | 10.17% / -114.93 | 0.55% / -113.31 | 0.00% / -109.47 |

At the 6 h guard (the v1 setting):

| Model | Guard | Unwind | Mean (vp) | p5 (vp) | p50 (vp) | p95 (vp) | p99 (vp) | Mean (USDC) | Tail (USDC) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| GBM, μ = 0 | 6 h | 0.62% | +0.34 | -55.95 | +0.00 | +61.54 | +129.51 | +34 | +12951 |
| GBM, μ = +100% | 6 h | 0.53% | +1.24 | -54.15 | +0.00 | +60.28 | +135.55 | +124 | +13555 |
| AR(1), ρ = +0.3 | 6 h | 0.62% | +16.14 | -33.86 | +3.47 | +105.48 | +213.64 | +1614 | +21364 |
| AR(1), ρ = −0.3 | 6 h | 0.83% | -12.30 | -76.69 | -5.37 | +35.20 | +71.24 | -1230 | +7669 |
| Halts, continuation | 6 h | 10.30% | +211.95 | -47.48 | +104.43 | +831.20 | +1250.12 | +21195 | +125012 |
| Halts, reversal | 6 h | 10.17% | -114.93 | -368.21 | -92.17 | +52.27 | +122.87 | -11493 | +36821 |

### Setup Instructions

Requirements: a stable Rust toolchain, the Solana platform tools (`cargo build-sbf`, the
benchmarks were taken with 4.5.0) and `make`.

```shell
git clone https://github.com/joaquinbejar/varswap.git
cd varswap
make build      # host build of the workspace
make program    # target/deploy/varswap_program.so (the LiteSVM tests load it)
make test       # every test
```

optfix comes from crates.io (`optfix = "0.1"`).

### Usage Examples

#### Admin CLI

`varswap-admin` reads `--url` (`SOLANA_URL`), `--program-id` (`VARSWAP_PROGRAM_ID`) and
`--keypair` (`VARSWAP_KEYPAIR`); `--dry-run` prints the instruction instead of sending it. The
URL is never printed.

```shell
cargo run -p varswap-cli --bin varswap-admin -- --help

# Feed risk parameters for SOL/USD (v1 values)
varswap-admin set-feed-config \
  --feed ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d \
  --min-interval 3600 --tolerance 21600 --backfill-deadline 86400 \
  --max-buckets 1105 --max-conf 1000 --exponent -8 --dry-run

# A 30 day hourly window (721 buckets)
varswap-admin create-window \
  --feed ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d \
  --start 1798761600 --end 1801353600 --interval 3600

varswap-admin show-window <WINDOW_ADDRESS>
```

Other commands: `init-config --collateral-mint <MINT>`, `window-address`, `show-config`,
`show-feed --feed <ID>` and `show-swap <ADDRESS>`.

#### SDK

```rust
use varswap_sdk::{ix, pda::{self, WindowKey}, Address};

fn sample_ixs(program_id: &Address, payer: &Address, price_update: &Address) {
    let key = WindowKey { feed: [0u8; 32], start: 1_798_761_600, end: 1_801_353_600, interval: 3_600 };
    let (window, _bump) = pda::window(program_id, &key);
    let _create = ix::create_window(program_id, payer, &key);
    let _sample = ix::sample(program_id, &window, price_update);
    let _finalize = ix::finalize(program_id, &window);
}
```

`ix::open_swap` takes an `ix::OpenSwap` (window, both parties, payer, both collateral accounts,
mint, `notional`, `strike_var` and `cap_var` in Q64.64 raw, `index`); `ix::settle_swap`,
`ix::claim_long`, `ix::claim_short` and `ix::close_swap` finish the cycle, and `decode::window`,
`decode::swap` and friends read the accounts back.

#### Keeper

The keeper posts every published canonical print the window accepts and finalizes when the
window is complete or past its deadline. It reads the Hermes API key only from the
`VARSWAP_HERMES_API_KEY` environment variable (optionally `VARSWAP_HERMES_URL` and
`VARSWAP_HERMES_AUTH`); the key is never committed and is redacted from `Debug` and errors.

```rust
use varswap_keeper::{Chain, HermesConfig, HermesSource, Keeper, KeeperError, UreqHttp};
use varswap_sdk::Address;

fn run<C: Chain>(chain: &mut C, program_id: Address, window: Address) -> Result<(), KeeperError> {
    let source = HermesSource::new(HermesConfig::from_env()?, UreqHttp::new());
    let report = Keeper::new(program_id).tick(chain, &source, &window)?;
    println!("posted {} skipped {} final {:?}", report.posted, report.skipped, report.finalized);
    Ok(())
}
```

`Chain` is the node interface (`now`, `account`, `post_sample`, `finalize`). In production it
is `ReceiverChain` over `pyth_post::RpcTransport`: each print is fetched from Hermes, checked to
be the exact print the keeper chose, posted through Wormhole and the Pyth receiver as a `Full`
`PriceUpdateV2`, sampled in the posting transaction, and both accounts are closed (fees only).
The `varswap-keeper` binary runs it:

```shell
export SOLANA_URL=https://api.devnet.solana.com VARSWAP_PROGRAM_ID=<program id>
export VARSWAP_KEYPAIR=~/.config/solana/id.json   # pays fees only
export VARSWAP_HERMES_API_KEY=...                  # environment only, never printed
cargo run -p varswap-keeper --release --bin varswap-keeper -- --watch 30 <window> [<window> ...]
```

It prints each transaction signature on stdout and one report line per window and tick on
stderr. With `--watch` it repeats until every window is final.

### Testing and Benchmarks

```shell
make pre-push   # fmt-check, lint, lint-control, sbf, sbf-control, test, doc: zero warnings
make bench      # runs the program in LiteSVM and regenerates docs/benchmarks.md
make study      # regenerates docs/study.md (fixed seeds)
```

Tests include proptests of the accumulator against a brute force model of the Pyth message tape,
GBM proptests of the `ln` kernel against its bound, 603 exact decimal fixtures for `V_cons`,
`pyth-lite` decoding of real mainnet fixtures, and LiteSVM negative tests for every account rule.

Compute units measured in LiteSVM 0.18 (cargo-build-sbf 4.5.0, platform tools v1.57, sBPF v3):

| Instruction | CU | Without events | Scenario |
| --- | ---: | ---: | --- |
| `init_config` | 7632 | 7632 | find_program_address, ProgramData gate, create 40 B |
| `set_feed_config (create)` | 4765 | 4765 | find_program_address, create 96 B |
| `set_feed_config (update)` | 3533 | 3533 | re-derive with stored bump |
| `create_window (B = 25)` | 7851 | 7851 | 1 day hourly, 369 B |
| `create_window (B = 721)` | 6879 | 6879 | 30 days hourly, 6,729 B |
| `create_window (B = 1,105)` | 7167 | 7167 | MAX_BUCKETS, 10,233 B |
| `sample (no neighbour)` | 3714 | 3060 | first bucket, no kernel call |
| `sample (both neighbours)` | 7084 | 6352 | 2 kernel calls |
| `sample (outage run of 6)` | 7734 | 7002 | one print fills 6 buckets, 2 kernel calls |
| `finalize (complete)` | 1407 | 1407 | all filled: annualize |
| `finalize (degraded)` | 1307 | 1307 | empty after the deadline |
| `open_swap` | 16836 | 16836 | create swap and vault, two transfer_checked |
| `settle_swap (finalized)` | 3638 | 3638 | V_cons, payouts |
| `settle_swap (degraded)` | 2484 | 2484 | unwind |
| `claim_long` | 3059 | 3059 | transfer_checked signed by the swap PDA |
| `claim_short` | 3061 | 3061 | transfer_checked signed by the swap PDA |
| `close_swap` | 3093 | 3093 | close vault and swap |

Full table: [`docs/benchmarks.md`](docs/benchmarks.md).

## Contribution and Contact

We welcome contributions to this project! If you would like to contribute, please follow these steps:

1. Fork the repository.
2. Create a new branch for your feature or bug fix.
3. Follow the coding rules in [`rules/global_rules.md`](https://github.com/joaquinbejar/varswap/blob/main/rules/global_rules.md)
   and run `make pre-push`, which must pass with zero warnings.
4. Make your changes and ensure that the project still builds and all tests pass.
5. Commit your changes and push your branch to your forked repository.
6. Submit a pull request to the main repository.

If you have any questions, issues, or would like to provide feedback, please feel free to contact the project maintainer:


### **Contact Information**

- **Author**: Joaquín Béjar García <https://joaquinbejar.capitaldelta.co/>
- **Email**: jb@taunais.com
- **Telegram**: [@joaquin_bejar](https://t.me/joaquin_bejar)
- **Repository**: <https://github.com/joaquinbejar/varswap>
- **Documentation**: <https://github.com/joaquinbejar/varswap/tree/main/docs>

We appreciate your interest and look forward to your contributions!

## ✍️ License

Licensed under **MIT** license

<!-- related-projects:start -->
## Related projects

Repositories by the same author that this project depends on, and repositories that depend on it.

### Depends on

| Repository | Description |
|------------|-------------|
| [optfix](https://github.com/joaquinbejar/optfix) | Fixed-point option pricing for Solana programs: no_std, allocation-free Black-Scholes, greeks and implied volatility, measured in compute units. |

### Used by

| Repository | Description |
|------------|-------------|
| [metalbook](https://github.com/joaquinbejar/metalbook) | Cash-settled European options on gold and silver (XAU, XAG) on Solana, settled in USDC from Pyth; reuses pyth-lite. |

<!-- related-projects:end -->
