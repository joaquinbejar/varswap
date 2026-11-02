# Degraded window: unwind its swaps

A window with any empty bucket after `end + backfill_deadline` finalizes as `Degraded`, and
every swap on it settles as an unwind: each side gets exactly its collateral back
([ADR 0001](../adr/0001-degraded-unwind.md)). Nothing is lost but time and fees.

## Detect

`varswap-admin show-window <window>`: `state: degraded`, `realized_var: null`.

## Fix

`finalize`, `settle_swap`, `claim_long`, `claim_short` and `close_swap` are permissionless.

1. `finalize` (the keeper sends it once the deadline passes; anyone can).
2. For each swap on the window (any funded keypair pays the fees):
   ```sh
   varswap-admin settle-swap <swap>
   varswap-admin claim-long <swap>     # into the long's associated token account (--dest to override)
   varswap-admin claim-short <swap>    # either order; a frozen account on one side does not block the other
   varswap-admin close-swap <swap>     # after both claims; rents go to the payer recorded in the swap
   ```
   Each command reads the swap first and checks the destination token account (the side's, or
   the payer's for `close-swap`, of the swap's mint) before signing; `--dry-run` prints the
   instruction instead.

## Verify

`varswap-admin show-swap <swap>`: settled, `long_payout == long_collateral` and
`short_payout == short_collateral`; each side's token account received its collateral; after
`close_swap` the swap and vault accounts are gone and the rent is back with the payer.
