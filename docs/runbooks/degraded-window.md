# Degraded window: unwind its swaps

A window with any empty bucket after `end + backfill_deadline` finalizes as `Degraded`, and
every swap on it settles as an unwind: each side gets exactly its collateral back
([ADR 0001](../adr/0001-degraded-unwind.md)). Nothing is lost but time and fees.

## Detect

`varswap-admin show-window <window>`: `state: degraded`, `realized_var: null`.

## Fix

`finalize`, `settle_swap`, `claim_long`, `claim_short` and `close_swap` are permissionless.

1. `finalize` (the keeper sends it once the deadline passes; anyone can).
2. For each swap on the window: `settle_swap`, then `claim_long` and `claim_short` (either
   order; a frozen token account on one side does not block the other), then `close_swap`.
   `varswap-admin` has no command for these yet: build them with `varswap_sdk::ix`
   (`settle_swap`, `claim_long`, `claim_short`, `close_swap`) as `program/tests/sdk.rs` does.

## Verify

`varswap-admin show-swap <swap>`: settled, `long_payout == long_collateral` and
`short_payout == short_collateral`; each side's token account received its collateral; after
`close_swap` the swap and vault accounts are gone and the rent is back with the payer.
