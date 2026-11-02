# Keeper payer is low

A post costs fees only: every account it opens (encoded VAA, price update) is closed in the
same flow and its rent returns to the payer. On devnet one post (three transactions) costs
about 0.00002 SOL in fees; the 2026-10-31 window's 76 transactions cost 0.00063 SOL. Rent is
borrowed while a post is in flight: keep at least 0.05 SOL above the fees.

## Detect

```sh
solana balance <payer> -u devnet
```

A post failing with `insufficient funds` in the keeper log, or a balance under 0.05 SOL.

## Fix

Transfer from a funded key (never from the upgrade authority):

```sh
solana transfer <payer> 0.5 --from ~/.config/solana/id.json --allow-unfunded-recipient -u devnet
```

On devnet `solana airdrop` also works when the faucet allows it.

## Verify

`solana balance <payer>`; the next tick posts again. If buckets were missed meanwhile, the
keeper backfills them ([backfill.md](backfill.md)).
