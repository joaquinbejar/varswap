# ADR 0001: a degraded window unwinds every swap

- Status: accepted for v1 (2026-11-02).
- Context documents: [`docs/PLAN.md`](../PLAN.md) 4.1 (outage guard) and 4.3 (missing data),
  [`docs/study.md`](../study.md).

## Context

A window is a grid `b_0 < b_1 < … < b_N` and a swap pays on the sum of squared log returns
between consecutive grid points. Each bucket is filled by its canonical Pyth print; one print
may fill a run of buckets when the feed did not update across them (an outage), up to the
window's guard (`tolerance`, 6 h on the devnet SOL/USD feed): the run's returns are compressed
into one return at its edges.

The code:

- `varswap-core/src/window.rs:436-443` (`Window::finalize`): when every bucket is filled the
  window is `Finalized` and its realized variance is computed; otherwise, once
  `now > end + backfill_deadline`, it is `Degraded`; before that, `FinalizeTooEarly`.
- `program/src/swap.rs:242-249` (`settle_swap`): `Finalized` settles as
  `Settlement::Realized(conservative realized variance)`; `Degraded` settles as
  `Settlement::Unwind`, and each side gets its own collateral back (`SwapTerms::payouts`).

A bucket stays empty when no acceptable canonical print exists for it (an outage longer than
the guard, a print outside the confidence bound, a print right after a Pyth re-init, an
exponent change, a VAA nobody verified in time) or when nobody posts it before the deadline.

## Evidence

`docs/study.md` (`make study`, fixed seed, 4,000 windows per model) simulates 30 day hourly
windows (`N = 720`) at σ = 80% under an **assumed** outage regime (exogenous outages at 1 per
168 h, exponential length of mean 1 h; the halt models add endogenous halts after > 3σ moves).
It is not a historical measurement: Hermes and Benchmarks history needs an API key. Share of
windows that unwind, by guard:

| Model | 1 h | 3 h | 6 h | 12 h | 24 h |
| --- | ---: | ---: | ---: | ---: | ---: |
| GBM, μ = 0 | 64.40% | 12.40% | 0.62% | 0.00% | 0.00% |
| GBM, μ = +100% | 62.00% | 12.38% | 0.53% | 0.00% | 0.00% |
| AR(1), ρ = +0.3 | 63.70% | 12.20% | 0.62% | 0.00% | 0.00% |
| AR(1), ρ = −0.3 | 63.58% | 12.65% | 0.83% | 0.00% | 0.00% |
| Halts, continuation | 89.28% | 43.43% | 10.30% | 0.53% | 0.00% |
| Halts, reversal | 87.78% | 43.65% | 10.17% | 0.55% | 0.00% |

A longer guard trades unwinds for error: for GBM μ = 0 the p99 of contract minus latent
variance grows from +55.08 variance points (1 h) to +129.51 (6 h), and under halts with
continuation the mean error at 6 h is +211.95 points (p99 +1,250.12). The v1 parameters use
6 h.

## Alternatives considered

| Alternative | Why not in v1 |
| --- | --- |
| Settle on partial coverage above a threshold (for example 95% of buckets) | Coverage does not bound the pathwise error: the missing buckets are exactly the outage hours, where the moves are largest (the halt models), so a 95% covered window can miss most of the variance. `min_coverage` was removed for this reason (`docs/PLAN.md` 4.3). |
| Interpolate missing buckets | Invents prices nobody published. A linear bridge in log price splits a move of `r` over `k` returns of `r / k`, whose squares sum to `r² / k`: it understates the variance and systematically favors the short. |
| Scale `sum_sq` by `N / filled` | Assumes the missing returns look like the observed ones, the opposite of what outages do; manipulable by whoever can keep a bucket empty. |
| Longer backfill deadline | Already covers keeper downtime (24 h on devnet). It cannot fill a bucket that has no canonical print. |
| Longer guard (12 h, 24 h) | Removes almost all unwinds in the study but compresses half a day of moves into one return; the error tails above grow, and a feed can be gamed around a long halt. A per feed choice, not a protocol default. |
| Per feed guard | Already the design: `tolerance` is a `FeedConfig` parameter copied into each window, so a feed with long scheduled closures can choose differently. Not a fix for unscheduled outages. |
| Partial settlement on the covered segments | Changes the product (the payoff no longer matches the contract's definition) and needs a pricing story for both sides. A v2 question. |

## Decision

v1 keeps the strict policy: a window with any empty bucket at the deadline is `Degraded`,
and every swap on it unwinds, each side getting exactly its collateral back. The payoff is
always the contract's definition or nothing, never an estimate.

## Consequences

- Unwinds are a normal outcome, not a failure: both sides lose only the time value of their
  collateral and the fees. Front ends and docs say so before a swap is opened.
- Liveness matters: a missed bucket that has a canonical print is a keeper failure, which
  the standby keeper and alerts (`docs/PLAN-improvements.md`, I6) and the backfill runbook
  address.
- Window records (I7) label degraded windows as such, with their coverage, and report no
  realized variance.

## What would reopen it

- Historical outage data for the target feeds (with a Hermes key) showing an unwind rate
  far from the assumed regime at the chosen guard.
- A partial settlement rule with a proven error bound in the presence of outages.
- Markets that need trading-hours calendars (metals), where closures are scheduled and a
  calendar, not a guard, is the right tool.
