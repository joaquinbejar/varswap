# VarSwap

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Realized variance and variance swaps on Solana.

A Pinocchio program accumulates realized variance from canonical Pyth prints over a fixed
hourly grid and settles capped, fully collateralized variance swaps against a variance
strike. The design, its decisions and the numbers behind them are in
[`docs/PLAN.md`](docs/PLAN.md).

**Status:** milestones 1 to 5 done (accumulator, `pyth-lite`, window program, swaps, CU
benchmarks and the outage study); milestone 6 (SDK, keeper, devnet) next. Not deployed.

## Workspace

| Crate | What |
| --- | --- |
| `pyth-lite` | `no_std` reader for Pyth `PriceUpdateV2` accounts: owner, discriminator, layout, Full verification, feed, price, staleness, confidence |
| `varswap-core` | `no_std` window grid, canonical-print sampling, bucket storage, the optfix `ln` kernel and its error bound, swap terms and settlement |
| `program` | the on-chain program: config, feed configs, windows, swaps (`docs/errors.md`, `docs/benchmarks.md`) |
| `study` | off-chain simulation study (`make study` writes `docs/study.md`) |

## Why canonical prints

The sample for each grid boundary is the unique Pyth print with `prev_publish_time < b <=
publish_time`, so the pusher has no choice. Without that rule a pusher accepting any print
within `w` seconds after each boundary could steer the payoff (exact best case by dynamic
programming, GBM σ = 80%, 30 days hourly; `docs/study.md`):

| w | Long gain mean (vp) | Long gain p95 (vp) | Short gain mean (vp) | Short gain p95 (vp) | Long gain mean (USDC) |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 10 s | +482.84 | +529.58 | +424.22 | +466.75 | +48284 |
| 60 s | +1298.53 | +1422.97 | +947.30 | +1047.35 | +129853 |
| 300 s | +3495.54 | +3795.10 | +1764.87 | +1944.01 | +349554 |

(vp: variance points, variance × 10,000; USDC per 1,000,000 USDC of notional per unit of
variance; the latent variance is about 6,400 vp.)

## Outages

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
