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
    pub tolerance: Seconds,      // sample must have publish_time in [boundary, boundary + tolerance]
    pub max_age: Seconds,        // update older than this vs Clock is stale
    pub max_conf: Bps,           // conf / price above this is rejected
    pub min_coverage: Bps,       // fraction of grid points that must be sampled
}

#[repr(u8)]
pub enum WindowState { Pending, Sampling, Finalized, Degraded }

#[repr(u8)]
pub enum SwapState { Open, Settled }
```

Accumulator invariants:

- Samples are strictly ordered by grid index and by `publish_time`.
- `sum_sq_log_returns >= 0`; each return is `ln(p_i / p_{i−1})` computed with optfix `ln`.
- `realized_var = sum_sq × YEAR_SECONDS / (t_last − t_first)` once finalized, so a missed grid
  point makes one return span two intervals instead of biasing the estimate (quadratic
  variation over elapsed time; see 4.3).
- `Finalized` requires `samples >= min_coverage × grid_points` and a sample in the last
  bucket; otherwise the window becomes `Degraded` (see 4.3).

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
| `Window` | `["window", feed_id, start_le, end_le, interval_le]` | varswap | 8 + spec 96 + state 64 = ~168 | `create_window` (anyone), `sample` (anyone), `finalize` (anyone) |
| `Swap` | `["swap", window, long, short, swap_index_le]` | varswap | 8 + 32×3 + 8×4 + 1 = ~144 | `open_swap`, `settle_swap` |
| Collateral vault | ATA of the `Swap` PDA for USDC | SPL Token | 165 | token CPIs only |

```rust
struct Window {
    hdr: Hdr, spec: WindowSpec, state: WindowState,
    samples: u32, last_index: u32,         // grid index of the last sample
    first_time: i64, last_time: i64,
    last_price: Fixed,                     // normalized to the spec's exponent
    sum_sq: Fixed,                         // running sum of squared log returns
    realized_var: Fixed,                   // set at finalize
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
| 1 | `create_window` | payer | config, window (w), system |
| 2 | `sample` | anyone (payer of fee only) | window (w), price_update (Pyth receiver account), clock sysvar |
| 3 | `finalize` | anyone | window (w), clock |
| 4 | `open_swap` | long, short, payer | window, swap (w), vault (w), long_usdc (w), short_usdc (w), usdc_mint, token program, ATA program, system |
| 5 | `settle_swap` | anyone | window, swap (w), vault (w), long_usdc (w), short_usdc (w), usdc_mint, token program |
| 6 | `close_swap` | anyone after settle | swap (w), vault (w), rent receiver = payer recorded in swap |

### 3.3 Security checks

- `create_window`: PDA from spec fields; `start < end`, `interval > 0`, `(end − start) %
  interval == 0`, `tolerance < interval`, `feed` in the admin allowlist in `Config` (so windows
  only exist on feeds we have vetted), `start` in the future.
- `sample`: the `price_update` account is owned by the Pyth receiver program (address pinned
  in `Config`, verified at milestone 2), has the `PriceUpdateV2` discriminator,
  `verification_level == Full`, `feed_id == window.spec.feed`, `exponent` equals the first
  sample's exponent, `price > 0`, `conf × 10_000 <= price × max_conf`,
  `now − publish_time <= max_age`, `publish_time` inside `[boundary_k, boundary_k + tolerance]`
  for the grid index `k` it claims, and `k > last_index`. Clock from the sysvar, not args.
- `open_swap`: both parties sign; window not finalized and `now < window.start` (no opening
  after sampling begins, so nobody trades on known returns); swap PDA; vault is the ATA of the
  swap PDA with the config's USDC mint; collateral amounts recomputed on-chain, not trusted
  from data.
- `settle_swap`: window `Finalized` or `Degraded`; swap `Open`; destination token accounts
  owned by `swap.long` / `swap.short` with the USDC mint; vault matches.
- Negative tests for each: forged price update owned by another program, Partial verification,
  other feed, stale, wide confidence, wrong grid, replayed bucket, vault of another swap,
  destination of another owner.

## 4. Key design decisions

### 4.1 Who pushes samples, and manipulation resistance

Options: admin keeper only; anyone. **Recommended: anyone** (liveness does not depend on us),
with the following limiting the pusher's choice:

- **Grid sampling**: one sample per bucket, and only a price published within `tolerance`
  after the boundary. With `tolerance = 10 s` and Pyth's sub-second updates, the pusher picks
  among about 25 prices in a 10-second interval, not freely over the hour. Smaller tolerance
  is tighter but more likely to miss.
- **Minimum interval** is the grid itself; extra updates for the same bucket are rejected.
- **Confidence filter** rejects prints during dislocations (`max_conf`).
- Residual risk: the pusher's choice inside the tolerance window and the choice to skip a
  bucket (one return over two intervals). Both are quantified in the plan's test suite with
  simulated GBM paths: worst-case bias of a pusher picking extremes, reported in the README.

### 4.2 Running sums vs ring buffer

As in 3.1: running sums for settlement, events for auditability. **Recommended.**

### 4.3 Gaps

- A missed bucket costs coverage, not correctness, because variance is normalized by elapsed
  time.
- If coverage at finalize is below `min_coverage`, or the last bucket is missing within
  `end + grace`, the window is `Degraded`. Swaps on a degraded window settle at the variance
  estimated from the samples present (still elapsed-time normalized) only if coverage is above
  a hard floor (for example 50%); below the floor, swaps unwind: each side gets its collateral
  back. Open question 2.
- Market closures (weekends for metals, see metalbook) must be excluded from the grid, or
  they appear as one long return across the weekend. v1: grid spec includes an optional
  "trading-hours calendar" for 24/7 feeds vs metals; first windows target SOL/USD (24/7).

### 4.4 Collateral and payoff cap

Payoff for the short is unbounded without a cap. Options: cap realized variance (`cap_var`,
market convention is 2.5² × strike variance, i.e. 2.5× strike vol); margin with liquidation.
**Recommended: cap, fully collateralized.** Max loss of the long is `notional × strike_var`
(realized variance zero); of the short, `notional × (cap_var − strike_var)`.

Settlement: `pnl_long = notional × (min(realized, cap) − strike)`. `long_payout =
long_collateral + pnl_long`, computed rounded down; `short_payout = total − long_payout`. Thus
rounding dust favors the short by at most one atom and the vault empties exactly. Documented
as "rounding toward the short", a deliberate, symmetric-enough choice that keeps the
conservation invariant trivial; open question 4.

### 4.5 Annualization

`YEAR_SECONDS = 31_536_000` matching optfix `Years`. For metals, if the grid skips closed
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
   paths shows realized variance within the documented bound, including random gaps.
2. **`pyth-lite`.** *Done when* it decodes real `PriceUpdateV2` accounts fetched from mainnet
   (fixtures committed) for Full and Partial levels, rejects wrong owner/discriminator, and the
   receiver program id is verified and cited.
3. **Window program.** `init_config`, `create_window`, `sample`, `finalize` with LiteSVM and a
   negative test per rule in 3.3. *Done when* all pass and a 24-sample SOL/USD replay from
   fixtures finalizes to the off-chain value.
4. **Swaps.** `open_swap`, `settle_swap`, `close_swap`. *Done when* conservation
   (`payouts == collateral`) holds in proptest across realized in `[0, 2 × cap]`, degraded and
   unwind paths are tested, and every forged account is rejected.
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
- Not yet verified, to do in milestone 2: the Pyth receiver program id, and whether sponsored
  push feed accounts exist for the feeds we use.

## 6. Open questions

1. First underlying: SOL/USD (24/7, simplest) proposed. Or straight to XAU?
2. Degraded windows: settle on partial data above a 50% floor and unwind below it, or always
   unwind?
3. Annualization for markets with closures: calendar time or trading time?
4. Rounding dust to the short (proposed, simplest conservation) or to a treasury?
5. Is a ring buffer of recent samples wanted for UIs, at extra rent?
6. Should `open_swap` route through quotebook (a quote for a variance strike) in v1 or v2?
