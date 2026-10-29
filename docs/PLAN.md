# varswap: plan

Status: approved by Joaquin on 2026-10-29. Changes after approval are recorded as decisions.

## 1. Goal and non-goals

### Goal

Two things on Solana, in one Pinocchio program:

1. **A realized-variance accumulator**: an account per (feed, window) that turns Pyth pull-
   oracle price updates into a running sum of squared log returns over a fixed window, with
   explicit rules for sampling, staleness, confidence and gaps.
2. **Variance swaps** settled against a finished accumulator: notional, variance strike, payoff
   `notional × (realized_var − strike_var)`, fully collateralized on both sides, settled at
   window end.

Plus `pyth-lite`, a `no_std` zero-copy reader for Pyth `PriceUpdateV2` accounts without Anchor,
which metalbook reuses.

### Non-goals (v1)

- Margin calls, liquidations, mark-to-market of open swaps (fully collateralized instead).
- Order book or AMM for swaps: two parties agree off-chain (or via quotebook later) and sign
  one `open_swap` transaction together.
- Corridor, conditional or gamma swaps; volatility swaps (convexity adjustment).
- Pushing prices ourselves: anyone posts Pyth updates through the Pyth receiver; varswap only
  reads them.

## 2. Domain model

`varswap-core` (`no_std`), using optfix's `Fixed` for log returns and variance.

```rust
pub struct FeedId(pub [u8; 32]);
pub struct UnixTimestamp(pub i64);
pub struct Seconds(pub u32);
pub struct UsdcAtoms(pub u64);
pub struct Bps(pub u16);

/// A Pyth price as read from PriceUpdateV2: price × 10^exponent, conf in the same units.
pub struct OraclePrice { pub price: i64, pub conf: u64, pub exponent: i32, pub publish_time: UnixTimestamp }

/// Annualized variance (sigma squared), e.g. 0.04 for 20% vol. Never negative.
pub struct Variance(Fixed);
/// Variance notional: USDC paid per 1.0 of annualized variance difference.
pub struct VarianceNotional(UsdcAtoms);

pub struct WindowSpec {
    pub feed: FeedId,
    pub start: UnixTimestamp,
    pub end: UnixTimestamp,
    pub interval: Seconds,       // sampling grid, e.g. 3600
    // Copied at creation from the admin-written FeedConfig, never from the creator (3.3):
    pub tolerance: Seconds,      // long outage guard, hours: publish_time <= boundary + tolerance (4.1)
    pub max_conf: Bps,           // loose sanity bound only (4.1)
    pub backfill_deadline: Seconds, // buckets can be filled until end + this (4.1, 4.3)
    pub receiver_program: Address,  // copied from FeedConfig; samples are checked against it
}
// Stored with explicit padding and state as u8: `#[repr(u8)]` enums and implicit padding
// cannot derive Pod. Sizes are computed from the final field order in milestone 3.

#[repr(u8)]
pub enum WindowState { Pending, Sampling, Finalized, Degraded }

#[repr(u8)]
pub enum SwapState { Open, Settled }
```

Implemented in milestone 1a (`varswap-core`): `FeedId` and `UnixTimestamp` are pyth-lite's;
`Seconds(u32)` and `Bps(u16)` as above; the risk parameters copied from `FeedConfig` are a
`RiskParams` value; a sampled print is a `Print` built from a verified
`pyth_lite::OraclePrice`; `sum_sq` is a `SumSq(u128)` in the raw units of the `ReturnKernel`
(optfix `Fixed` raw from milestone 1b); the library error is `CoreError` (`#[repr(u8)]`,
mapped into `VarswapError` by the program). `SampleOutOfOrder` is gone: buckets fill in any
order. A loaded window is checked for `filled` equal to the bits set and a state consistent
with it (`Pending`: none; `Finalized`: all; `Degraded`: not all).

Accumulator invariants:

- Each bucket `k` in `[0, N]` holds at most one sample, the canonical print for `b_k` (4.1).
- `sum_sq_log_returns >= 0`; it is the sum of `ln(p_k / p_{k−1})²` over the pairs of
  consecutive buckets that are both filled, computed with optfix `ln`.
- `Finalized` requires all `N + 1` buckets; then `realized_var = sum_sq × YEAR_SECONDS /
  (b_N − b_0)`. Otherwise, after `end + backfill_deadline`, the window is `Degraded` and its
  swaps unwind (4.3).

Swap invariants:

- `long_collateral == notional × strike_var` (rounded up), `short_collateral == notional ×
  (cap_var − strike_var)` (rounded up). `cap_var > strike_var > 0`.
- Settlement pays from the pooled collateral only; `long_payout + short_payout == total
  collateral` exactly, so the vault never goes negative or keeps dust.

```rust
#[repr(u32)]
pub enum VarswapError {
    InvalidInstruction = 100, InvalidInstructionData, MissingSignature, InvalidAccountOwner,
    InvalidPda, InvalidMint, InvalidVault, InvalidSysvar,
    // oracle
    OracleOwnerMismatch, OracleDiscriminatorMismatch, OracleNotFullyVerified,
    OracleFeedMismatch, OracleExponentMismatch, OracleStale, OracleConfidenceTooWide,
    OracleNonPositivePrice,
    // sampling
    WindowNotStarted, WindowEnded, NotOnGrid, BucketAlreadySampled, SampleOutOfOrder,
    // swap
    WindowNotFinal, SwapAlreadySettled, InvalidStrike, InvalidCap, CollateralMismatch,
    Overflow, Pricing(optfix::PricingError) /* mapped to 200 + code */,
}
```

## 3. Accounts and instructions

### 3.1 Accounts

| Account | Seeds | Owner | Size | Written by |
| --- | --- | --- | --- | --- |
| `Config` | `["config"]` | varswap | 40: `Hdr` + admin (M3a; swap fields join in milestone 4) | admin |
| `FeedConfig` | `["feed", feed_id]` | varswap | 96 (`program::layout::FeedConfigData`) | admin: risk parameters per feed (receiver program id, min_interval, tolerance, max_conf, backfill_deadline, max_buckets) |
| `Window` | `["window", feed_id, start_le, end_le, interval_le]` | varswap | `144 + 8 × ceil((N + 1) / 64) + 9 × (N + 1)`: 6,729 bytes for 721 buckets (sketch, see below) | `create_window` (anyone), `sample` (anyone), `finalize` (anyone) |
| `Swap` | `["swap", window, long, short, swap_index_le]` | varswap | 8 + 32×3 + 8×4 + 1 + 8×2 + 2 = ~162 | `open_swap`, `settle_swap`, `claim_*` (payouts and claimed flags, +18 bytes) |
| Collateral vault | `["vault", swap]` PDA token account (system create + `initialize_account3`) | SPL Token | 165 | token CPIs only |

```rust
#[repr(C)]
struct WindowHeader {                      // 144 bytes, no implicit padding
    hdr: Hdr,                              // 8
    spec: WindowSpec,                      // 96: 94 bytes of fields + `_pad: [u8; 2]`
    state: u8, _pad: [u8; 3],              // WindowState stored as u8, validated on load
    filled: u32,                           // number of buckets filled
    sum_sq: FixedBytes,                    // [u8; 16]: sum of squared log returns of adjacent pairs
    realized_var: FixedBytes,              // [u8; 16]: set at finalize
}
// Followed by three arrays, structure of arrays so every i64 stays 8-aligned:
//   filled bitmap  [u64; ceil((N + 1) / 64)]
//   prices         [i64; N + 1]   raw Pyth price of the canonical print per bucket
//   exponents      [i8;  N + 1]   its exponent; converted to Fixed only to compute ln
```

For `B = N + 1` buckets the serialized size is `bytes(B) = 144 + 8 × ceil(B / 64) + 9 × B`:
6,729 bytes for 721 buckets (30 days hourly). `WindowSpec` fields are ordered by alignment
(`feed`, `start`, `end`, `receiver_program`, then the `u32`s, then `max_conf: u16`) so its
only padding is the explicit trailing `_pad`. An account created through a CPI is capped at
10,240 bytes (`MAX_PERMITTED_DATA_INCREASE`, to verify in milestone 3), so the largest window
has `B = 1,105` buckets (`bytes = 10,233`; 1,106 would be 10,242). `MAX_BUCKETS = 1_105` is a
constant derived from this formula; milestone 3 replaces the sketch's 144 with
`size_of::<WindowHeader>()` and a unit test asserts `bytes(MAX_BUCKETS) <= 10_240 <
bytes(MAX_BUCKETS + 1)`. `set_feed_config` rejects `max_buckets > MAX_BUCKETS`;
`create_window` rejects `N + 1 > max_buckets`.

Every bucket's price is kept on-chain for the life of the window: buckets fill in any order,
and filling bucket `k` adds the returns to whichever neighbours `k − 1` and `k + 1` are already
filled, so their prices must be stored. Settlement reads only `sum_sq`; each accepted sample
is also logged as an event (bucket range, publish_time, price mantissa, exponent) so anyone can
recompute `sum_sq` off-chain. The `sum_sq` increment is not logged: clients recompute it from
the bucket prices, and formatting the `u128` cost about 3,400 CU per sample (decided
2026-10-29). The stored price array also serves UIs, so no separate ring
buffer is needed.

### 3.2 Instructions

| # | Instruction | Signers | Accounts |
| --- | --- | --- | --- |
| 0 | `init_config` | admin (the program's upgrade authority), payer | config (w), program_data, system |
| 0b | `set_feed_config` | admin, payer (on create) | config, feed_config (w), system: creates or overwrites; validates `max_conf`, `tolerance > 0`, `backfill_deadline > 0`, `max_buckets <= MAX_BUCKETS` (3.1) |
| 1 | `create_window` | payer | feed_config, window (w), system (the feed and its risk parameters come from `feed_config`) |
| 2 | `sample` | anyone (payer of fee only) | window (w), price_update (Pyth receiver account); clock from the syscall; return data and an event per accepted sample |
| 3 | `finalize` | anyone | window (w); clock from the syscall |
| 4 | `open_swap` | long, short, payer | window, swap (w), vault (w), long_usdc (w), short_usdc (w), usdc_mint, token program, system |
| 5 | `settle_swap` | anyone | window, swap (w): computes and stores both payouts, state `Settled`; moves no tokens |
| 5b | `claim_long` / `claim_short` | anyone | swap (w), vault (w), that side's USDC account (w), usdc_mint, token program: pays one side only, so a frozen USDC account on one side never blocks the other |
| 6 | `close_swap` | anyone after both sides claimed (a permanently frozen side leaves the rent locked; accepted) | swap (w), vault (w), rent receiver = payer recorded in swap |

### 3.3 Security checks

- `init_config` must not be front-runnable (cross-repo rule from quotebook milestone 2): it
  takes the program's `ProgramData` account, which must be owned by `BPFLoaderUpgradeab1e`,
  sit at the PDA of `[program_id]` under that loader, and have an `upgrade_authority` equal to
  the signing admin. Negative tests with a forged `ProgramData`: wrong owner, wrong address,
  a different authority, and no authority (immutable program). LiteSVM loads the program
  through the upgradeable loader so the real `ProgramData` exists.
- `create_window`: PDA from spec fields; `start < end`, `interval > 0`, `(end − start) %
  interval == 0`, `feed` has a `FeedConfig` (so windows only exist on vetted feeds),
  `interval >= feed_config.min_interval`, risk
  parameters copied from it (an anyone-can-create window must not let the first creator pick
  loose risk parameters and own the PDA), `start` in the future.
- `sample`: the `price_update` account is owned by the Pyth receiver program (address pinned
  in the window's `FeedConfig` copy; mainnet id verified in milestone 2 as
  `pyth_lite::RECEIVER_PROGRAM_ID`), has the `PriceUpdateV2` discriminator,
  `verification_level == Full`, `feed_id == window.spec.feed`, `price > 0`,
  `conf × 10_000 <= price × max_conf` computed in `u128`, `prev_publish_time > 0` (4.1),
  `now <= end + backfill_deadline`,
  window not finalized. Buckets are derived, not taken from data: the print fills every `k` in `[0, N]` with
  `prev_publish_time < b_k <= publish_time <= b_k + tolerance` (4.1), i.e. `k_lo =
  max(0, floor((prev_publish_time − start) / interval) + 1, ceil((publish_time − tolerance −
  start) / interval))` to `k_hi = min(N, floor((publish_time − start) / interval))`, all
  empty, a run of at most `tolerance / interval + 1` buckets so CU stays bounded. Returns
  inside the run are zero; only the two edge returns go to `sum_sq` (still at most two `ln`
  calls). M3 test: an outage spanning 3 boundaries, one `sample` call, `sum_sq` equals one
  return over 4 intervals.
  Floors and ceilings use `checked_div_euclid` (`prev_publish_time − start` is negative for
  the print that fills `b_0`; truncating division would be off by one; test `b_0`), and
  `k_lo > k_hi` is the typed error `NotOnGrid`, never a no-op. The print's exponent must equal the
  exponent the window copied from its `FeedConfig` (`ExponentMismatch` otherwise; 4.4): an
  exponent change, which only a Pyth admin re-init causes, degrades the window instead of
  widening the error bound. Clock read with the Pinocchio syscall.
- `open_swap`: both parties sign; window not finalized and `now < window.start` (no opening
  after sampling begins, so nobody trades on known returns); swap PDA; vault is the `["vault", swap]` PDA token account
  for the swap's USDC mint; collateral amounts recomputed on-chain, not trusted
  from data.
- `finalize`: immediately when all `N + 1` buckets are filled, otherwise only after
  `end + backfill_deadline`, so nobody can degrade a window by finalizing early. `N >= 1`
  is checked at `create_window`; the window is `Degraded` if any bucket is empty.
- `settle_swap`: window `Finalized` or `Degraded`; swap `Open`. `claim_*`: swap `Settled`,
  destination owned by that side with the USDC mint, vault matches, claimed once.
- Negative tests for each: forged price update owned by another program, Partial verification,
  other feed, stale, wide confidence, wrong grid, replayed bucket, vault of another swap,
  destination of another owner.

## 4. Key design decisions

Binding: a `Window` copies its `FeedConfig` (receiver program id, feed, risk parameters) at
creation and a `Swap` stores its collateral mint and token program; `sample`, `settle_swap`
and `claim_*` read only those copies, never mutable global config. Tested by mutating
`Config` and `FeedConfig` mid-life.

### 4.1 Who pushes samples, and manipulation resistance

Anyone may push (liveness does not depend on us), but the pusher must have no choice. A
first-write-wins rule with a tolerance window would still let the pusher pick among several valid
prints, and a skipped bucket changes the payoff pathwise (codex review: prices
`[100, 110, 100]` give `sum_sq ≈ 0.01817`; dropping the middle print gives 0 over the same
elapsed time; a short gains by omitting reversals, a long by merging same-direction moves).
So samples are **canonical**:

- The sample for boundary `b_k = start + k × interval` is the unique Pyth print with
  `prev_publish_time < b_k <= publish_time`, i.e. the first successful aggregate published
  at or after the boundary. `PriceUpdateV2` carries `prev_publish_time`, so the program can
  check the crossing. **Verified in milestone 2** (sources under Verified facts, "Canonical
  print"): `prev_publish_time` is the `publish_time` of the previous *successful* aggregate
  of the feed, and successful aggregates have non-decreasing publish times, so their
  intervals `(prev_publish_time, publish_time]` tile the time line and every boundary has
  at most one canonical print. Messages from slots where aggregation failed, and later
  aggregates within the same second, carry `prev_publish_time == publish_time`: an empty
  interval that never matches. No fallback to first-write-wins is needed. Residual cases:
  - an aggregate that Pythnet computed but never sent (the legacy V1 message path) leaves
    its boundaries with no canonical print: the bucket stays empty and the window degrades
    (liveness, not choice);
  - the first aggregate after an oracle `init_price` (feed creation or an admin re-init) has
    `prev_publish_time = 0`, so its interval overlaps earlier prints and would give the
    pusher a choice inside `tolerance`. `sample` rejects `prev_publish_time == 0` (3.3,
    negative test in milestone 3); the boundaries only that print covers stay empty.
- A canonical sample can be posted late (Hermes serves historical updates by timestamp), so
  there is no hindsight to exploit and `max_age` no longer limits backfill: any bucket can be
  filled until `end + backfill_deadline` (proposed 24 h).
- Buckets are a bitmap, filled in any order; there is no `k > last_index` rule, so a later
  bucket can never lock out an earlier one. `k` is derived from `publish_time` and must be
  inside `[0, N]` (both grid endpoints included); samples after finalization are rejected.
- Each bucket stores the raw Pyth `price: i64` and `exponent: i8` (9 bytes, in separate
  arrays; converted to `Fixed` only to compute `ln`): 6,729 bytes for 721 buckets, under the
  10,240-byte CPI creation cap, with `MAX_BUCKETS = 1_105` derived from the serialized size
  in 3.1. `FeedConfig.max_buckets` caps `N + 1` at `create_window`. When bucket `k` is filled,
  the returns to its filled neighbours `k − 1`
  and `k + 1` are added to `sum_sq`, so a sample costs at most two `ln` calls and finalize
  costs none.
- Outages: the first print after `b_k` is canonical for every boundary it crosses, so after
  an outage it fills all the gap's buckets with one price, and the gap's whole move appears
  as one return `R = r_1 + … + r_m` spanning `m` intervals.
  - **Contractual payoff.** The contract pays on squared log returns between consecutive
    canonical samples; across an outage that is `R²`, by definition. Nobody can choose the
    samples (canonicity), so this is not a manipulation lever.
  - **Latent grid variation.** The contract does not measure the variation `Σ r_i²` the
    full grid would have seen; the two differ by `R² − Σ r_i² = 2 Σ_{i<j} r_i r_j`. In
    expectation the gap is zero only for zero-mean, uncorrelated increments. Drift adds
    `m (m − 1) μ²`; autocorrelation adds `2 Σ Cov(r_i, r_j)`; and halts are endogenous
    (they cluster around large moves), so increments conditional on a halt need not be
    zero-mean or uncorrelated. Examples: two equal moves `[a, a]` give `4a²` against `2a²`
    (the long gains); a reversal `[a, −a]` gives `0` against `2a²` (the short gains). The
    compression is not unbiased in general and the plan does not claim it is.
  - **Guard, a policy choice.** A short guard turns every Pyth halt into an unwind, and
    halts cluster in stress, when the short is losing: a free option for the short, as with
    a tight confidence filter. No guard lets an arbitrarily long gap collapse into one
    return. v1 takes `publish_time <= b_k + tolerance` with `tolerance = 6 h` (decided
    2026-10-29): gaps up to 6 h settle on the compressed return, longer ones unwind. This
    trades the two risks; it removes neither.
  - **Quantified in milestone 5**, by simulation and not by counting alone: GBM with drift;
    AR(1) returns with positive and negative `ρ`; and an endogenous-halt model (a halt
    triggered by `|r|` above a threshold, lasting `L` intervals, with continuation and with
    reversal). For each, and for `tolerance` in {1, 3, 6, 12, 24} h, report (a) the
    distribution of `R² − Σ r_i²` summed over the window, in variance points and in USDC per
    unit of variance notional, and (b) the unwind frequency. Real SOL/USD history supplies
    only outage frequency and length, because the latent path inside a real outage is not
    observed; counting how often the guard fires does not measure the error.
  - (metalbook keeps a short tolerance: its payoff is a mean of prices, where one reopening
    print filling many slots would bias the mean.)
- Confidence: no tight filter. Canonicity and `Full` verification already remove the
  pusher's choice, and a tight `max_conf` would make a missing bucket (hence an unwind) most
  likely exactly when variance is high and the short is losing: a free option for the short
  (varswap session). Only a loose sanity bound (proposed `conf / price > 10%`), documented as
  an exceptional unwind; the bias study measures how often real SOL/USD history would have
  unwound under it.
- Verified in milestone 2 (sources under Verified facts):
  - `publish_time` is whole Unix seconds from the Pythnet clock, which never goes
    backwards. Several aggregates can share a second; only the first carries
    `prev_publish_time < publish_time`, the rest have `prev_publish_time == publish_time`,
    so uniqueness holds with repeats. This is proven from the aggregation source rather than
    from a fixture: the receiver keeps only the latest update per account, and Hermes serves
    one update per requested time.
  - Hermes `GET /v2/updates/price/{t}` returns the first update with `publish_time >= t`,
    ties broken by the earliest slot (the real aggregate before any failed-slot copy).
    Keepers request `t = b_k`. The program still checks `prev_publish_time < b_k` itself:
    after the real update leaves Hermes's cache (about 640 s) Hermes may serve a failed-slot
    copy, or fall back to the Benchmarks service, whose source is not public.
  - Guardian sets: Wormhole refuses to verify a VAA signed by a guardian set that expired
    (24 h after a rotation), in `verify_encoded_vaa_v1` and in `post_update_atomic`;
    `post_update` accepts an encoded VAA verified earlier at any time. A canonical print
    whose VAA was not verified within 24 h of a rotation becomes unpostable and its bucket
    degrades the window. That is a liveness risk for keepers, not a lever: nobody can post a
    different print in its place.
  - Push oracle feed accounts are not a source of canonical prints: the push oracle stores
    an update only if its `publish_time` is strictly newer than the stored one, on Pyth's
    schedule and not on our grid. Keepers post canonical prints with `post_update` from
    Hermes (milestone 6).

### 4.2 Running sums vs ring buffer

As in 3.1: `sum_sq` for settlement, plus every bucket's price on-chain (required by
out-of-order neighbour updates), plus events for auditability. A separate ring buffer is not
needed: the price array already holds the whole window.

### 4.3 Missing data

With canonical backfill, a bucket is missing only if nobody posts it before
`end + backfill_deadline`, or it has no acceptable canonical print. The unwind triggers are:
- no canonical print within the outage guard (a halt longer than `tolerance`, 4.1);
- the canonical print fails the confidence sanity bound (4.1);
- the canonical print is the first after a Pyth `init_price` (`prev_publish_time = 0`, 3.3);
- the canonical print's exponent differs from the one the window pinned, which only a Pyth
  admin re-init causes (3.3, 4.4);
- a VAA that nobody verified within 24 h of a guardian set rotation (4.1);
- nobody posts the print before the deadline. v1 policy is
strict and honest about the payoff: the contract is "sum of squared log returns between
consecutive grid points from `b_0` to `b_N`". If any bucket (including either endpoint) is
missing at the deadline, the window is `Degraded` and every swap on it unwinds: each side
gets its collateral back. No partial-coverage settlement: coverage thresholds do not bound
the pathwise error. `min_coverage` is removed.

Market closures (weekends for metals) would appear as long returns across the closure; v1
windows target 24/7 SOL/USD only, and a trading-hours calendar is a v2 question.

### 4.4 Collateral and payoff cap

Payoff for the short is unbounded without a cap. Options: cap realized variance (`cap_var`,
market convention is 2.5² × strike variance, i.e. 2.5× strike vol); margin with liquidation.
**Recommended: cap, fully collateralized.** Max loss of the long is `notional × strike_var`
(realized variance zero); of the short, `notional × (cap_var − strike_var)`.

Settlement: `pnl_long = notional × (min(realized, cap) − strike)`. `long_payout =
long_collateral + pnl_long`, computed rounded down and clamped to `[0, total]` as defense;
`short_payout = total − long_payout`. The vault empties exactly; in total at most 3 atoms of
rounding end with the short (2 from rounding collateral up, 1 from rounding the long payout
down). Documented
as "rounding toward the short", a deliberate, symmetric-enough choice that keeps the
conservation invariant trivial; open question 4.

Realized variance error (milestone 1b, decided 2026-10-29). optfix `ln` is within 2 ulp
(`2^-64`) and rounds to nearest, so its error is undirected. Per squared log return the error
is at most `2 |r| δr + 3` raw units, `δr = 4 + |Δe|` (two `ln` calls plus the half-ulp
`ln 10` constant scaled by the exponent change; `LnKernel::term_error_bound`, checked per term
by the GBM proptest). Over a complete window, by Cauchy–Schwarz, the realized variance error
is at most

`ε(interval, cap) = 2 δr √(cap × Y) + 3 Y + 2` raw units, `Y = ceil(YEAR_SECONDS / interval)`,

independent of the number of buckets (`LnKernel::realized_var_error_bound`; derivation in its
docs). The payoff reads `min(realized, cap_var)`, so the bound is evaluated at `cap_var`.

The exponent is pinned (coordinator, 2026-10-29): `FeedConfig` records the feed's Pyth
exponent, every window copies it, and `sample` rejects a print with another one
(`ExponentMismatch`), so `Δe = 0` and `δr = 4`. Pyth changes a feed's exponent only through an
admin `init_price` (feed creation or re-init; the same event that yields the
`prev_publish_time = 0` print rejected in 3.3). After such a change no print can fill the
window's remaining buckets, so it degrades and its swaps unwind; the admin updates
`FeedConfig` for new windows. A liveness cost on a rare admin event, not a safety one. With
`cap_var = 25` (500% vol):

| Interval | ε (raw) | ε (variance) | Max notional for ε < 1 atom (atoms per unit variance) |
| --- | ---: | ---: | ---: |
| 1 s | 94,832,630 | `5.1 × 10^-12` | `1.95 × 10^11` (195k USDC) |
| 60 s | 1,605,802 | `8.7 × 10^-14` | `1.15 × 10^13` (11.5M USDC) |
| 3,600 s | 30,026 | `1.6 × 10^-15` | `6.14 × 10^14` (614M USDC) |

Unpinned (`Δe` up to 255) the 60 s row would be 3,454,508 raw (5.3M USDC). At short intervals
the `3 Y` term dominates: one raw unit of `mul_down` truncation per term plus the bound's own
round-up.

**Milestone 4 plan (B), approved 2026-10-29:** accumulate `sum_sq` at `2^-96` instead of
`2^-64`: each term is the 256-bit product `r × r` shifted to `2^-96` (rounded down), so the
per-term truncation falls from `2^-64` to `2^-96` and the `3 Y` term to about `2^-32 × 3 Y`;
the `u128` sum still holds `2^32` units of `Σ r²`. `annualize` converts back to Q64.64
(rounded down). The widening multiply-and-shift comes from optfix as a public, reviewed
function (requested by the coordinator), not a copy; milestone 4 pins that optfix rev and
regenerates this table with exact limits from `realized_var_error_bound`.

**Which case applies: the bound is applied in the short's favour (codex review of #11).**
A payoff error under one atom is not enough by itself: the payout is floored in atoms, and a
tiny positive `ln` error can still push the long's payout across an atom boundary, one atom
against the short (codex's counterexample, now a regression test). So payouts settle on the
conservative variance `V_cons = max(0, V − ε(window.interval, cap_var))`
(`LnKernel::conservative_realized_var`), taken before the cap and before the floor.

- Guarantee, on the capped values: `min(V_cons, cap) <= min(V_exact, cap)`: **the long is
  never overpaid**. When `V_exact < cap`, the kernel's uncapped bound `|V − V_exact| <= ε(cap)`
  gives `V_cons <= V_exact`; when `V_exact >= cap` the right side is `cap` and the inequality
  is immediate (a capped absolute-error bound alone would not give the direction). Above the cap the
  uncapped `V_cons <= V_exact` need not hold; the payout never reads it.
- Budget: the `ln` error is within `±ε` and `ε` is then subtracted, so the short gains at most
  `2ε × notional` from the bound, plus at most one atom from the payout's floor. The notional
  limit stays at `notional × ε < 2^64` (coordinator, 2026-10-29: capacity matters more than
  atoms of `10^-6` USDC), so **the short's advantage is under three atoms per swap**: under
  two from the bound, at most one from the floor.
- Checked exactly against a 100-digit decimal reference (`varswap-core/tools/vcons_fixtures.py`,
  603 cases committed in `tests/fixtures/vcons.txt`, codex's three counterexamples included:
  the atom crossing, an above-cap case where the uncapped `V_cons > V_exact`, and a 2-atom
  short advantage), at the largest notional the limit allows: `|min(V, cap) − min(V_exact,
  cap)| <= ε`, `min(V_cons, cap) <= min(V_exact, cap)`, the long never overpaid, the short's
  advantage at most 2 whole atoms below the exact payout's floor (under 3 against the exact
  payout), and `ε` equal to an independent Python replica. The GBM proptest checks the same
  quantities' magnitudes in f64 (a mutation subtracting `4ε` fails it at 4.5 atoms).

The directed parts of the kernel (`r²` and the annualization rounded down) also lower the
realized variance.

### 4.5 Annualization

`YEAR_SECONDS = 31_536_000` matching optfix `Years`. Industry variance swaps annualize with
252 / N trading days and quote the strike in variance points (vol² × 10,000) with a vega
notional; elapsed-time normalization fits 24/7 SOL/USD, and the SDK exposes vega notional
and variance points as conversions. For metals, if the grid skips closed
hours, annualization uses trading time; that is a convention to fix with you (open question 3).

### 4.6 Pyth access without Anchor

`pyth-lite` (in this workspace, `no_std`): checks owner, 8-byte discriminator, decodes
`PriceUpdateV2` by offset (the `VerificationLevel` field is 1 or 2 bytes, so offsets after it
are computed, not constant), returns `OraclePrice`. No dependency on
`pyth-solana-receiver-sdk` on-chain (it pulls in Anchor); that crate is a dev-dependency used
to build test accounts and to cross-check the decoder.

## 5. Milestones

1. **Domain and accumulator math.** `varswap-core` with types, errors, accumulator update and
   finalize as pure functions. Split in two (coordinator, 2026-10-29), because the log-return
   kernel needs optfix `ln`, which optfix decides in its milestone 3:
   - **1a, done:** newtypes and `CoreError`; `WindowSpec` with its validation, grid and
     stored layout (`layout::WindowSpecRaw`, 96 bytes, fields ordered by alignment;
     `WindowHeader`, 144 bytes; offsets tested); `window_size` and `MAX_BUCKETS = 1_105`
     (`window_size(1_105) = 10_233 <= 10_240 < 10_242 = window_size(1_106)`); the filled
     bitmap and the price and exponent arrays; canonical-run assignment with Euclidean
     floors and `NotOnGrid`; `Print` rejecting exponents outside `i8` and `prev_publish_time
     == 0`; sampling (edge returns only, nothing written on error), the backfill deadline,
     the 6 h outage guard and finalize (`Finalized` at once when complete, `Degraded` only
     after `end + backfill_deadline`). The return math is behind the `ReturnKernel` trait
     (`squared_log_return`, `annualize`, raw `u128` units of the kernel). Tested with an
     exact integer kernel: a proptest model of the Pyth message tape (same-second
     aggregates, failed-slot copies, long halts) submitted in random order matches a
     brute-force reference exactly in filled buckets, prices, `sum_sq`, final state and
     realized variance; canonical runs match brute force; a coverage test checks the
     strategy produces complete and degraded windows, multi-bucket runs and guarded prints.
   - **1b, done:** `LnKernel` on optfix `ln` (git dependency pinned at `fd1e81c`, Q64.64):
     `r = ln(p_to) − ln(p_from) + (e_to − e_from) × ln 10`, each `ln` on a price mantissa,
     so no ratio can overflow whatever the exponents; `r²` rounded down, annualization
     rounded down. Error bound per term in 4.4. *Done:* a proptest on GBM paths (σ 5% to
     300%, drift ±100%, intervals 1 min to 1 day, up to 720 intervals, four price scales)
     checks every squared return against an f64 `ln_1p` reference within the documented
     bound plus the reference's own `4 × 2^-53` relative error, that `sum_sq` and the
     realized variance are exactly the sum of those terms and its annualization, and that
     dropping any one bucket yields `Degraded`. A mutation of 50 raw units per term fails it.

   The workspace carries the shared lint control (`rules/global_rules.md`,
   synced from optfix `afb9d4d`; set up with milestone 2, which ran first; the
   entrypoint-module allow landed with the program in milestone 3a):
   - `lint-control/` (`Cargo.toml`, `src/lib.rs`, `check.sh`) copied from optfix and added
     to the workspace members;
   - `cfg(lint_control)` in `unexpected_cfgs` `check-cfg`;
   - `disallowed_macros = "deny"` in `[workspace.lints.clippy]`, with `clippy.toml` at the
     root;
   - `clippy::disallowed_macros` allowed in tests and benches and in the module that invokes
     the Pinocchio entrypoint macros (`no_allocator!` expands to `assert!`), with a comment;
   - `make lint-control` part of `make pre-push`.
2. **`pyth-lite`.** *Done when* it decodes real `PriceUpdateV2` accounts fetched from mainnet
   (fixtures committed) for Full and Partial levels, rejects wrong owner/discriminator and
   malformed, truncated and unknown-verification-tag fixtures, the receiver program id is
   verified and cited, and the `prev_publish_time` canonical-print property of 4.1 is
   verified (or the fallback is chosen with you).
3. **Window program.** `init_config`, `create_window`, `sample`, `finalize` with LiteSVM and a
   negative test per rule in 3.3, including the four forged-`ProgramData` cases of
   `init_config`. *Done when* all pass and a 24-sample SOL/USD replay finalizes to the
   off-chain value. Split (coordinator, 2026-10-29):
   - **3a, done:** the program (`program/`) with `init_config`, `set_feed_config`,
     `create_window`, `sample`, `finalize`, running `varswap_core::IntegerKernel` (an exact
     integer stand-in). Fail closed (codex review): the stand-in exists only behind the
     `insecure-test-kernel` feature, and without it the program is a `compile_error!`, so no
     deployable ELF can be built; `make sbf-control` requires the default `cargo build-sbf`
     to fail with that error. Tests and the bench build with the feature; milestone 1b
     removed the gate by making the optfix kernel the default (the integer kernel stays
     behind the feature, for `varswap-core`'s own tests). LiteSVM loads it
     through the upgradeable loader. Negative tests with forged accounts for every rule of
     3.3; the `FeedConfig` binding test; the 10,240-byte CPI cap exercised by creating a
     `MAX_BUCKETS` window (10,233 bytes) through the system program CPI. The replay is a
     **synthetic** 24-interval SOL-like tape built from the canonical-print definition
     (same-second aggregates, failed-slot copies, intermediate aggregates, a 2 h halt),
     submitted out of order, and byte-identical to the off-chain `varswap-core` window.
     Error codes: 100 to 119 program validation, `120 +` `PythError`, `140 +`
     `CoreError::code`, `200 +` kernel payload (`program/src/error.rs`). `sample` and
     `finalize` check the window's owner and its header (`WINDOW_TAG`, which only
     `create_window` writes) instead of re-deriving its PDA. Each accepted sample logs
     `varswap:sample first= last= publish_time= price= expo=` (pinocchio-log); the event's
     CU is measured separately in `docs/benchmarks.md`. The `added` field was dropped after
     measuring it (4,131 CU for the event with it, most of it formatting the `u128`).
   - **3b, done with 1b:** the program runs `LnKernel`; the synthetic replay is byte-identical
     between SBF and the host; `sample` with two kernel calls is 6,627 CU
     (`docs/benchmarks.md`).
   - The replay from real SOL/USD prints moves to milestone 6, where keepers need Hermes
     access anyway (verified facts, 2026-10-29).
4. **Swaps.** `open_swap`, `settle_swap`, `close_swap`. *Done when* conservation
   (`payouts == collateral`) holds in proptest across realized in `[0, 2 × cap]`, degraded and
   unwind paths are tested, `claim_long` and `claim_short` are tested in both orders and with
   either side's USDC account frozen (the other side still claims), every forged account
   is rejected, and `open_swap` rejects `notional × LnKernel::realized_var_error_bound(
   window.interval, cap_var, 0) >= 2^64` (the realized variance error bound of 4.4, exponent
   pinned), with a test at the limit and one just above it; `settle_swap` pays on
   `LnKernel::conservative_realized_var` (`V − ε`, before the cap and the floor), with tests
   that the long's payout never exceeds the payout on the exact capped variance and that the
   short's advantage stays under three atoms; `sum_sq` is accumulated at
   `2^-96` per plan (B) of 4.4 and the 4.4 table is regenerated from it.
5. **Benchmarks and manipulation study.** *Done when* `docs/benchmarks.md` has CU for every
   instruction and the README has the pusher-bias table and the outage tables from 4.1
   (compression error and unwind frequency per model and per `tolerance`).
6. **SDK and devnet.** *Done when* a Rust SDK and a keeper example push samples on devnet for
   a live SOL/USD window and it finalizes, and a replay of 24 real canonical SOL/USD prints
   (fixtures fetched from Hermes, committed) finalizes to the off-chain value. Keepers read
   the Hermes API key from an environment variable; it is never committed.

## Verified facts (checked 2026-10-28)

- `PriceUpdateV2` (pyth-solana-receiver-sdk 2.0.0, `src/price_update.rs`): 8-byte Anchor
  discriminator, `write_authority: Pubkey`, `verification_level` (`Partial { num_signatures:
  u8 }` = 2 bytes or `Full` = 1 byte), `price_message` (`feed_id [u8;32]`, `price i64`, `conf
  u64`, `exponent i32`, `publish_time i64`, `prev_publish_time i64`, `ema_price i64`,
  `ema_conf u64`), `posted_slot u64`; `LEN = 134`. `get_price_no_older_than` requires `Full`.
- `pyth-solana-receiver-sdk` 2.0.0, `pythnet-sdk` 3.0.0 on crates.io.
- Crypto.SOL/USD feed id `ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d`
  (Hermes `/v2/price_feeds?query=SOL/USD&asset_type=crypto`).
- Checked in milestone 2 (2026-10-29):
  - Program ids: receiver `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`, push oracle
    `pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT` (pyth-solana-receiver-sdk 2.0.0
    `src/lib.rs`, `declare_id!` and `PYTH_PUSH_ORACLE_ID` with default features; the
    `pro-compatible` feature uses `rec2HHDDnjLfj4kE7VyEtFA1HPGQLK33259532cRyHp` and
    `pyt2F414BA6dPttK6RddPZUdHfapoBN24GL5wbrPCou`). The receiver owns all four mainnet
    fixtures. `pyth_lite::RECEIVER_PROGRAM_ID` and `PUSH_ORACLE_PROGRAM_ID` are tested
    against the SDK constants and the base58 strings.
  - Push oracle feed accounts are PDAs of the push oracle with seeds `[shard_id (u16 LE),
    feed_id]` (`pyth-push-oracle/src/sdk.rs`), owned by the receiver, with the PDA itself as
    `write_authority`, always `Full` (`post_update` hard-codes it). Sponsored accounts
    exist on mainnet, shard 0: Metal.XAU/USD `2uPQGpm8X4ZkxMHxrAW1QuhXcse1AHEgPih6Xp9NuEWW`
    (feed `765d2ba906dbc32ca17cc11f5310a89e9ee1f6420508c63861f2f8ba4ee34bb2`), Metal.XAG/USD
    `H9JxsWwtDZxjSL6m7cdCVsWibj3JBMD9sxqLjadoZnot` (feed
    `f2fb02c32b055c805e7238d628e5e9dadef274376114eb1f012337cabe93871e`), Crypto.SOL/USD
    `7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE`. The XAG/USD account's last
    `publish_time` is 2026-07-03, about 118 days before the fetch: that sponsored feed is
    not maintained, so nothing may rely on it being fresh. Exponents seen: SOL −8,
    XAU −3, XAG −5, all within `i8` (3.1).
  - Real `Partial` accounts exist (found with `getProgramAccounts` on the receiver,
    `dataSize 134`, tag byte 0 at offset 40); the committed one is a BTC/USD update with
    `num_signatures = 5`.
  - Canonical print (pyth-client `acba47f`, pyth-crosschain `69d9242`, pythnet validator
    `pyth-v1.14.17` `99aa1e1`):
    - pyth-client `program/c/src/oracle/upd_aggregate.h:135-215` rolls
      `prev_timestamp_ = timestamp_` only when the stored status is `Trading`.
    - `program/rust/src/accounts/price.rs:116-133` builds the message: on a failed slot
      it uses `prev_timestamp_` for both times.
    - `pythnet_sdk/src/messages.rs:99-110` documents the rule "the unique update is the one
      such that prev_publish_time < t <= publish_time".
    - Pythnet `runtime/src/bank/pyth/accumulator.rs:393-484` emits one message per feed per
      slot, and `runtime/src/bank.rs:2226-2233` keeps the clock monotone.
    - Hermes `apps/hermes/server/src/api/rest/v2/timestamp_price_updates.rs:25-29` and
      `state/cache.rs:285-305` give "first update with publish_time >= t".
    - Guardian expiry: `core-bridge/src/legacy/state/guardian_set.rs:230-238`,
      `processor/parse_and_verify_vaa/verify_encoded_vaa_v1.rs:38-43` and
      `pyth-solana-receiver/src/lib.rs:412-440`. Push oracle: `pyth-push-oracle/src/lib.rs:94-108`.
- To verify in milestone 1 with optfix: `i128` alignment. Since Rust 1.77, `i128` is
  16-aligned on x86_64 hosts while account data is only guaranteed 8-aligned, so a
  `bytemuck::from_bytes` over a struct holding `Fixed` may fail. Fallback: store `Fixed` as
  `[u8; 16]` in accounts and convert at load.
- optfix is consumed as a git dependency pinned by `rev` until it is on crates.io (a path
  dependency would break CI).
- Checked 2026-10-29 (milestone 3a): Hermes `GET /v2/updates/price/{t}` and
  `/v2/updates/price/latest`, and Benchmarks `GET /v1/updates/price/{t}`, return HTTP 401
  `unauthorized` without an API key; `GET /v2/price_feeds` still answers without one. Keepers
  (milestone 6) need a Hermes API key, read from an environment variable and never
  committed.
- Checked 2026-10-29 (milestone 3a): `cargo build-sbf` 4.5.0 (platform-tools v1.57) prints
  `Error: Function … overflows the maximum allowed frame space …` for a frame over 4 KiB and
  still exits 0, and replays the message on cached builds; `scripts/sbf-build.sh` turns it
  into a failure (`make sbf-control` proves it, and that a failing cargo fails the script).
- Checked 2026-10-29 (milestone 3a): LiteSVM 0.18.1 does not compile against the agave 4.4
  crates that a fresh resolution picks; the workspace `Cargo.lock` keeps the resolution
  optfix uses (agave 4.3 generation), as quotebook does.

## Dependencies (latest stable on crates.io, checked 2026-10-29)

Rule from `rules/global_rules.md`: every dependency at the latest stable version, written
`MAJOR.MINOR`, re-checked via `https://crates.io/api/v1/crates/<name>` (`max_stable_version`)
whenever it is added or touched.

| Crate | Latest stable | Requirement |
| --- | --- | --- |
| `pinocchio` | 0.12.0 (0.11.2 used) | `0.11`, decided exception below |
| `bytemuck` | 1.25.2 | `1.25` |
| `proptest` (dev) | 1.12.0 | `1.12` |
| `litesvm` (dev) | 0.18.1 | `0.18` |
| `mollusk-svm` (dev, optional) | 0.16.4 | `0.16` |
| `solana-address` | 2.9.0 | `2.9` |
| `pinocchio-system` | 0.7.0 (0.6.1 used) | `0.6`, decided exception below |
| `pinocchio-token` | 0.7.0 | `0.7`, see conflict below |
| `rust_decimal` (SDK conversions) | 1.43.0 | `1.43` |
| `optfix` | unpublished | git, pinned `rev = fd1e81c5726c0b0a0e6e252f497f821cbcf92b97` (optfix M3) |
| `litesvm-token` (dev) | 0.18.1 | `0.18` |
| `pyth-solana-receiver-sdk` (dev only, fixtures and cross-check) | 2.0.0 | `2.0` |
| `anchor-lang` (dev only, pyth-lite cross-check; approved 2026-10-29) | 1.2.2 | `1.2` |
| `pinocchio-log` (sample events, `default-features = false`; approved 2026-10-29) | 0.5.1 | `0.5` |
| `solana-clock` (dev, LiteSVM clock) | 4.0.0 (3.2.1 used) | `3.2`, LiteSVM 0.18.1 pins `~3.2` |
| `solana-account`, `solana-instruction`, `solana-keypair`, `solana-sdk-ids`, `solana-signer`, `solana-transaction` (dev) | 5.4, 4.0, 4.0, 3.1, 4.0, 6.0 | `4.7`, `3.5`, `3.1`, `3.1`, `3.0`, `4.3`: the generations LiteSVM 0.18.1 pins, recorded exception as in optfix and quotebook |

**Version decision (Joaquin, 2026-10-29): option (b), the Pinocchio 0.11 generation.**
`pinocchio-token` 0.7.0 and `pinocchio-associated-token-account` 0.4.0 are built on
`solana-account-view` 2 (Pinocchio 0.11), while `pinocchio` 0.12.0 uses 3, so the latest
stable versions cannot be combined. All four repos and matchbook therefore use `pinocchio`
0.11 (0.11.2) and `pinocchio-system` 0.6 (0.6.1), the newest releases compatible with the
token crates. This is the documented exception to the latest-stable rule; move all four to
the 0.12 generation together once 0.12-compatible token crates are released.

## 6. Open questions

1. First underlying: SOL/USD (24/7, simplest) proposed. Or straight to XAU?
2. Decided (2026-10-29): degraded windows always unwind in v1. Canonical-print bounds:
   `conf / price <= 10%` and outage guard `tolerance = 6 h`, both exceptional unwinds.
3. Annualization for markets with closures: calendar time or trading time?
4. Rounding dust to the short (proposed, simplest conservation) or to a treasury?
5. Resolved: no separate ring buffer; every bucket's price is stored on-chain (3.1).
6. Should `open_swap` route through quotebook (a quote for a variance strike) in v1 or v2?
