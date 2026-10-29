# varswap: plan

Status: draft for review. Nothing here is implemented yet.

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
    pub tolerance: Seconds,      // halt bound: publish_time <= boundary + tolerance (4.1)
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
| `Config` | `["config"]` | varswap | 8 + 32 + 32 + 8 | admin |
| `FeedConfig` | `["feed", feed_id]` | varswap | ~64 | admin: risk parameters per feed (receiver program id, min_interval, tolerance, max_conf, backfill_deadline, max_buckets) |
| `Window` | `["window", feed_id, start_le, end_le, interval_le]` | varswap | ~200, computed in M3 | `create_window` (anyone), `sample` (anyone), `finalize` (anyone) |
| `Swap` | `["swap", window, long, short, swap_index_le]` | varswap | 8 + 32×3 + 8×4 + 1 + 8×2 + 2 = ~162 | `open_swap`, `settle_swap`, `claim_*` (payouts and claimed flags, +18 bytes) |
| Collateral vault | `["vault", swap]` PDA token account (system create + `initialize_account3`) | SPL Token | 165 | token CPIs only |

```rust
struct Window {
    hdr: Hdr, spec: WindowSpec, state: WindowState,
    filled: u32,                           // number of buckets filled
    sum_sq: Fixed,                         // sum of squared log returns of adjacent pairs
    realized_var: Fixed,                   // set at finalize
    // followed by: filled bitmap [u64; ceil((N + 1) / 64)] and prices [FixedBytes; N + 1]
}
```

Running sums, not a ring buffer: settlement needs only `sum_sq` and the times. Each accepted
sample is logged as an event (index, publish_time, price, return) so anyone can recompute the
sum off-chain. A ring buffer would cost rent proportional to the window and adds nothing to
settlement; it is listed as an open question for UI use.

### 3.2 Instructions

| # | Instruction | Signers | Accounts |
| --- | --- | --- | --- |
| 0 | `init_config` | admin, payer | config (w), system |
| 0b | `set_feed_config` | admin | config, feed_config (w), system: validates `max_conf`, `0 < tolerance < min_interval`, `backfill_deadline > 0`, `max_buckets` fits 10 KiB |
| 1 | `create_window` | payer | config, window (w), system |
| 2 | `sample` | anyone (payer of fee only) | window (w), price_update (Pyth receiver account), clock sysvar |
| 3 | `finalize` | anyone | window (w), clock |
| 4 | `open_swap` | long, short, payer | window, swap (w), vault (w), long_usdc (w), short_usdc (w), usdc_mint, token program, system |
| 5 | `settle_swap` | anyone | window, swap (w): computes and stores both payouts, state `Settled`; moves no tokens |
| 5b | `claim_long` / `claim_short` | anyone | swap (w), vault (w), that side's USDC account (w), usdc_mint, token program: pays one side only, so a frozen USDC account on one side never blocks the other |
| 6 | `close_swap` | anyone after both sides claimed (a permanently frozen side leaves the rent locked; accepted) | swap (w), vault (w), rent receiver = payer recorded in swap |

### 3.3 Security checks

- `create_window`: PDA from spec fields; `start < end`, `interval > 0`, `(end − start) %
  interval == 0`, `feed` has a `FeedConfig` (so windows only exist on vetted feeds),
  `interval >= feed_config.min_interval`, risk
  parameters copied from it (an anyone-can-create window must not let the first creator pick
  loose risk parameters and own the PDA), `start` in the future.
- `sample`: the `price_update` account is owned by the Pyth receiver program (address pinned
  in the window's `FeedConfig` copy, verified at milestone 2), has the `PriceUpdateV2` discriminator,
  `verification_level == Full`, `feed_id == window.spec.feed`, `price > 0`,
  `conf × 10_000 <= price × max_conf` computed in `u128`, `now <= end + backfill_deadline`,
  window not finalized. The bucket is derived, not taken from data: `k = ceil((publish_time −
  start) / interval)`, `0 <= k <= N`, the canonical crossing `prev_publish_time < b_k <=
  publish_time` holds (4.1), and bucket `k` is empty. Each print converts to `Fixed` with its own
  exponent (log returns are scale invariant), so a Pyth exponent change mid-window does not
  brick it. Clock read with the Pinocchio syscall.
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
  `prev_publish_time < b_k <= publish_time`, i.e. the first aggregate published at or after
  the boundary. `PriceUpdateV2` carries `prev_publish_time`, so the program can check the
  crossing; whether Pyth guarantees that `prev_publish_time` is the immediately preceding
  aggregate (so the canonical print is unique) is verified in milestone 2 against the Pyth
  receiver source and docs. If it cannot be proven, the fallback is the first-write-wins
  rule with the residual selection risk quantified in the bias study and accepted
  explicitly by you.
- A canonical sample can be posted late (Hermes serves historical updates by timestamp), so
  there is no hindsight to exploit and `max_age` no longer limits backfill: any bucket can be
  filled until `end + backfill_deadline` (proposed 24 h).
- Buckets are a bitmap, filled in any order; there is no `k > last_index` rule, so a later
  bucket can never lock out an earlier one. `k` is derived from `publish_time` and must be
  inside `[0, N]` (both grid endpoints included); samples after finalization are rejected.
- Each bucket stores the raw Pyth `price: i64` and `exponent: i8` (9 bytes; converted to
  `Fixed` only to compute `ln`), so 721 buckets for 30 days hourly take about 6.5 KB with the
  bitmap and header, under the 10 KiB limit for an account created through a CPI
  (`MAX_PERMITTED_DATA_INCREASE`, to verify in milestone 3). `FeedConfig.max_buckets` caps
  `N` at `create_window`. When bucket `k` is filled, the returns to its filled neighbours `k − 1`
  and `k + 1` are added to `sum_sq`, so a sample costs at most two `ln` calls and finalize
  costs none.
- Halt bound: the canonical print must also satisfy `publish_time <= b_k + tolerance`;
  otherwise the first print after an outage would be canonical for every boundary in the
  gap and fill them all with one price (zero returns). A gap then leaves buckets empty.
- Confidence: no tight filter. Canonicity and `Full` verification already remove the
  pusher's choice, and a tight `max_conf` would make a missing bucket (hence an unwind) most
  likely exactly when variance is high and the short is losing: a free option for the short
  (varswap session). Only a loose sanity bound (proposed `conf / price > 10%`), documented as
  an exceptional unwind; the bias study measures how often real SOL/USD history would have
  unwound under it.
- To verify in milestone 2: `publish_time` is in whole seconds while Pythnet aggregates about
  every 400 ms, so consecutive prints can share a `publish_time`; uniqueness needs
  `prev_publish_time` to be the previous aggregate's own `publish_time` including repeats
  (fixture with two prints sharing a second); Hermes `/v2/updates/price/{t}` returns the
  first aggregate at or after `t`; and whether the receiver rejects VAAs from an expired
  guardian set (about 24 h after rotation), which could make a bucket unpostable within
  `backfill_deadline`.

### 4.2 Running sums vs ring buffer

As in 3.1: running sums for settlement, events for auditability. **Recommended.**

### 4.3 Missing data

With canonical backfill, a bucket is missing only if nobody posts it before
`end + backfill_deadline` or its canonical print failed the confidence filter. v1 policy is
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
   finalize as pure functions. *Done when* proptest against an f64 reference on simulated GBM
   paths shows realized variance within the documented bound, and that any gap yields
   `Degraded`.
2. **`pyth-lite`.** *Done when* it decodes real `PriceUpdateV2` accounts fetched from mainnet
   (fixtures committed) for Full and Partial levels, rejects wrong owner/discriminator and
   malformed, truncated and unknown-verification-tag fixtures, the receiver program id is
   verified and cited, and the `prev_publish_time` canonical-print property of 4.1 is
   verified (or the fallback is chosen with you).
3. **Window program.** `init_config`, `create_window`, `sample`, `finalize` with LiteSVM and a
   negative test per rule in 3.3. *Done when* all pass and a 24-sample SOL/USD replay from
   fixtures finalizes to the off-chain value.
4. **Swaps.** `open_swap`, `settle_swap`, `close_swap`. *Done when* conservation
   (`payouts == collateral`) holds in proptest across realized in `[0, 2 × cap]`, degraded and
   unwind paths are tested, `claim_long` and `claim_short` are tested in both orders and with
   either side's USDC account frozen (the other side still claims), and every forged account
   is rejected.
5. **Benchmarks and manipulation study.** *Done when* `docs/benchmarks.md` has CU for every
   instruction and the README has the pusher-bias table from 4.1.
6. **SDK and devnet.** *Done when* a Rust SDK and a keeper example push samples on devnet for
   a live SOL/USD window and it finalizes.

## Verified facts (checked 2026-10-28)

- `PriceUpdateV2` (pyth-solana-receiver-sdk 2.0.0, `src/price_update.rs`): 8-byte Anchor
  discriminator, `write_authority: Pubkey`, `verification_level` (`Partial { num_signatures:
  u8 }` = 2 bytes or `Full` = 1 byte), `price_message` (`feed_id [u8;32]`, `price i64`, `conf
  u64`, `exponent i32`, `publish_time i64`, `prev_publish_time i64`, `ema_price i64`,
  `ema_conf u64`), `posted_slot u64`; `LEN = 134`. `get_price_no_older_than` requires `Full`.
- `pyth-solana-receiver-sdk` 2.0.0, `pythnet-sdk` 3.0.0 on crates.io.
- Crypto.SOL/USD feed id `ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d`
  (Hermes `/v2/price_feeds?query=SOL/USD&asset_type=crypto`).
- Not yet verified, to do in milestone 2: the Pyth receiver program id (candidate from the
  varswap session's memory, unverified: `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`; push
  oracle `pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT`), and whether sponsored push feed
  accounts exist for the feeds we use.
- To verify in milestone 1 with optfix: `i128` alignment. Since Rust 1.77, `i128` is
  16-aligned on x86_64 hosts while account data is only guaranteed 8-aligned, so a
  `bytemuck::from_bytes` over a struct holding `Fixed` may fail. Fallback: store `Fixed` as
  `[u8; 16]` in accounts and convert at load.
- optfix is consumed as a git dependency pinned by `rev` until it is on crates.io (a path
  dependency would break CI).

## Dependencies (latest stable on crates.io, checked 2026-10-29)

Rule from `rules/global_rules.md`: every dependency at the latest stable version, written
`MAJOR.MINOR`, re-checked via `https://crates.io/api/v1/crates/<name>` (`max_stable_version`)
whenever it is added or touched.

| Crate | Latest stable | Requirement |
| --- | --- | --- |
| `pinocchio` | 0.12.0 | `0.12` |
| `bytemuck` | 1.25.2 | `1.25` |
| `proptest` (dev) | 1.12.0 | `1.12` |
| `litesvm` (dev) | 0.18.1 | `0.18` |
| `mollusk-svm` (dev, optional) | 0.16.4 | `0.16` |
| `solana-address` | 2.9.0 | `2.9` |
| `pinocchio-system` | 0.7.0 | `0.7` |
| `pinocchio-token` | 0.7.0 | `0.7`, see conflict below |
| `rust_decimal` (SDK conversions) | 1.43.0 | `1.43` |
| `optfix` | unpublished | git, pinned `rev` |
| `litesvm-token` (dev) | 0.18.1 | `0.18` |
| `pyth-solana-receiver-sdk` (dev only, fixtures and cross-check) | 2.0.0 | `2.0` |

**Version conflict, needs a decision before this repo's first program milestone.**
`pinocchio-token` 0.7.0 and `pinocchio-associated-token-account` 0.4.0 depend on
`solana-account-view ^2.0` (the Pinocchio 0.11 generation), while `pinocchio` 0.12.0 depends on
`solana-account-view` 3.0; `pinocchio-system` 0.7.0 already targets `pinocchio ^0.12`. So the
latest stable versions of all four cannot be combined today. Options: (a) Pinocchio 0.12 with
the three token CPIs varswap needs written in-crate (`transfer_checked`, `close_account`,
`initialize_account3`; the vault is a PDA token account, so no ATA crate at all),
replaced by `pinocchio-token` once a 0.12-compatible release exists; (b) the whole set one
generation back (Pinocchio 0.11, as matchbook), against the latest-stable rule; (c) wait for the
companion releases. Proposal: (a), re-checked on crates.io at the start of the milestone.

## 6. Open questions

1. First underlying: SOL/USD (24/7, simplest) proposed. Or straight to XAU?
2. Degraded windows: always unwind in v1 (proposed after the codex review; the varswap
   session agrees). OK?
3. Annualization for markets with closures: calendar time or trading time?
4. Rounding dust to the short (proposed, simplest conservation) or to a treasury?
5. Is a ring buffer of recent samples wanted for UIs, at extra rent?
6. Should `open_swap` route through quotebook (a quote for a variance strike) in v1 or v2?
