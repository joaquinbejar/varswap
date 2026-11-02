# Audit scope

What an external audit of varswap covers, what it may assume, and which test or fuzz target
checks each invariant (`docs/PLAN-improvements.md`, I4). Pinned at `main` `6731d34`; the
deployed devnet build is the verifiable build of that source (build hash
`b9336e631b92f0213652a67440376fb9b275f6a5101c2c6ef809d555273386bb`, `docs/verifiable.md`).

## In scope: on chain

| Crate | Path | Files | Lines (non blank, non comment) | What |
| --- | --- | ---: | ---: | --- |
| `varswap-program` | `program/src` | 8 | 978 | the Pinocchio program: instructions, account validation, CPIs to the token and system programs, error mapping |
| `varswap-core` | `varswap-core/src` | 10 | 1,435 | window grid, canonical print acceptance, bucket storage, the `ln` kernel and its error bound, swap terms and payouts, account layouts |
| `pyth-lite` | `pyth-lite/src` | 4 | 327 | `PriceUpdateV2` decoding and the oracle checks |

Dependencies reached on chain: `pinocchio` 0.11, `pinocchio-system` 0.6, `pinocchio-token`
0.7, `pinocchio-log` 0.5 (the `events` feature), `bytemuck` 1.25, `solana-address` 2.7,
`optfix` 0.2 (`ln`, `wide::mul_shr`; audited
separately in its own repository). Versions as in `Cargo.lock`.

## Separately scoped: off chain

| Crate | Path | Files | Lines | What |
| --- | --- | ---: | ---: | --- |
| `pyth-post` | `pyth-post/src` | 8 | 852 | posting Hermes updates through Wormhole and the receiver, recovery of orphaned encoded VAAs, `PayerLock` |
| `varswap-keeper` | `keeper/src` | 8 | 936 | the keeper loop, the Hermes source, the receiver chain |

Off chain code holds no authority over funds: a faulty keeper can waste its payer's fees or
leave a bucket empty (which unwinds swaps, ADR 0001), never move collateral.

## Out of scope

`cli` (`varswap-admin`, a thin client of the SDK), `sdk` (instruction builders, checked byte
for byte against the test harness: `program/tests/sdk.rs`), `study`, `lint-control`, `fuzz`,
the scripts.

## Trust assumptions

- **Pyth publishers and the Pythnet aggregate**: prices are honest within their confidence;
  varswap bounds confidence (`max_conf`) but cannot detect a wrong aggregate.
- **Wormhole guardians**: a VAA with a quorum of the current guardian set is authentic. The
  receiver verifies it; varswap reads only `Full` verified updates.
- **The Pyth receiver program** (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`) and its config:
  it writes `PriceUpdateV2` accounts only from verified messages. varswap checks the owner
  against the receiver id its window copied, the discriminator and the `Full` level.
- **The token program**: SPL Token (or the program id the swap recorded) behaves as specified.
- **The upgrade authority** (`docs/governance.md`): it can replace the program. Until the
  program is frozen, users trust it.

## Invariants and where they are checked

| Invariant | Checked by |
| --- | --- |
| Only the upgrade authority can create `Config`, and only once; nobody can front run it | `program/tests/config.rs` (`test_init_config_*`, 12 tests including forged `ProgramData`, wrong owner, malformed) |
| Only the admin can vet a feed; windows copy the feed's parameters and never read the mutable `FeedConfig` again | `config.rs` `test_set_feed_config_*`; `window.rs` `test_create_window_copies_feed_parameters`, `test_window_bound_to_its_feed_config_copy` |
| A price update is accepted only if owned by the window's receiver, with the right discriminator, `Full`, the window's feed, a positive price and confidence within `max_conf` | `pyth-lite/tests/fixtures.rs` (17 tests), `anchor_crosscheck.rs` (5); `window.rs` `test_sample_oracle_rejections`, `test_sample_price_update_forged_owner_rejected`, `test_sample_price_update_malformed_rejected`; fuzz `pyth_update`, `sample_ix` |
| Each bucket gets its unique canonical print (`prev_publish_time < b_k <= publish_time`), whatever the submission order | `varswap-core/tests/model.rs` (`prop_any_order_matches_reference`, `prop_canonical_run_matches_brute_force`) |
| A bucket is filled at most once; a rejected sample writes nothing | `varswap-core/tests/window.rs` (`test_sample_replay_rejected_and_bytes_untouched`, `test_sample_rejections_leave_bytes_untouched`); fuzz `window_sample`, `sample_ix` |
| `sum_sq` is the sum of the squared log returns of consecutive filled buckets | fuzz `window_sample`; `model.rs` |
| The on chain `sample` equals the off chain path (`pyth-lite`, `accept_print`, `Window::sample_accepted`) byte for byte, and fails with the mapped error code otherwise | fuzz `sample_ix`; `program/tests/keeper.rs` `test_keeper_eligibility_equals_the_program` |
| `ln` kernel error bound: the conservative realized variance never exceeds the true one, so the long is never overpaid | `ln_kernel.rs` (`prop_gbm_realized_variance_within_bound`, `test_realized_var_error_bound_*`); `vcons_exact.rs` (603 exact decimal fixtures) |
| A window is `Finalized` exactly when full, `Degraded` only after `end + backfill_deadline` | `varswap-core/tests/window.rs` `test_finalize_*`, `test_outage_longer_than_guard_degrades`; fuzz `window_sample` |
| Collateral is `ceil(notional × strike)` and `ceil(notional × (cap − strike))`; `long + short == total` for every settlement; `Degraded` unwinds each side exactly | `varswap-core/tests/swap.rs` (`prop_conservation_and_monotone`, `test_collateral_rounds_up_exactly`, `test_unwind_returns_each_collateral`); fuzz `swap_conservation`; `program/tests/swap.rs` `test_settle_degraded_unwinds`, `test_full_cycle_payouts_match_core_across_paths` |
| The vault pays out exactly what it holds; claims in either order conserve it; a frozen side never blocks the other | `program/tests/swap.rs` (`test_claims_both_orders_conserve_the_vault`, `test_frozen_side_does_not_block_the_other`, `test_close_*`) |
| Every account rule (signer, owner, PDA seeds, mint, vault) rejects a forged account | the `*_forged_*`, `*_wrong_*` and `*_not_signer_*` tests in `program/tests/config.rs`, `window.rs`, `swap.rs` |
| No panic, unchecked arithmetic, indexing, float or integer division in on chain code | workspace lints (`Cargo.toml` `[workspace.lints]`), proven to fire by `make lint-control` |
| No stack frame over 4 KiB | `scripts/sbf-build.sh` in `make sbf`, proven by `make sbf-control` |
| Bounded compute: every instruction under 17,000 CU | `docs/benchmarks.md` (`make bench`), reproduced byte for byte by CI's `bench` job |

## Known limitations (by design)

- **Degraded unwinds every swap** ([ADR 0001](adr/0001-degraded-unwind.md)): one empty bucket
  at the deadline unwinds the window's swaps. Under the study's assumed outages, 0.62% of
  30 day hourly windows at a 6 h guard (GBM μ = 0).
- **The outage guard** (`tolerance`) compresses an outage up to the guard into one return;
  longer guards trade unwinds for error (`docs/study.md`).
- **Publish time collisions and residual canonicality cases** (`docs/PLAN.md` 4.1): later
  aggregates within the same second, and failed-slot copies, carry `prev_publish_time ==
  publish_time`, an empty interval that never matches, so each boundary has at most one
  canonical print. An aggregate never sent leaves its boundaries empty (the window degrades);
  the first aggregate after a Pyth `init_price` (`prev_publish_time = 0`) is rejected, and the
  boundaries only it covers stay empty.
- **No admin rotation**: the config admin is fixed at `init_config` (`docs/governance.md`).
- **The keeper is not trustless liveness**: anyone can sample, but if nobody does, buckets stay
  empty until the deadline.

## Prior reviews

Every PR was reviewed by codex (the SOLANA-PROJECTS reviewer agent) before merge; the reviews
are on the PRs (for example #22 `pyth-post` and the receiver chain, #24 the leftover scanner,
#30 the improvement plan, #31 CI, #35 fuzzing, #36 the verifiable build). No external audit
has been done.
