# Orphaned encoded VAAs

A post is three transactions: create, init and write the encoded VAA; verify it; then
`post_update`, `sample` and the closes. A crash between them leaves the encoded VAA open, its rent
locked and its `write_authority` the payer. A post that fails after the first transaction
reports `<cause>; encoded VAA <address> is still open` and queues it for closing.

## Detect

- The keeper log line ends `open encoded VAAs [<address>, ...]`.
- `scan: N leftover encoded VAA(s) queued for closing` at the keeper's start or every 12 ticks.

## Fix

Nothing by hand in the normal case. The keeper scans the payer's encoded VAAs (Wormhole
`getProgramAccounts`, `write_authority` = payer) at start and every 12 ticks, and closes one only
when consecutive scans saw it unchanged for at least 300 blocks (`LEFTOVER_MIN_AGE`), so a post
still in flight is never closed. Restart the keeper if it is not running.

Precondition (the `pyth-post` contract): one poster per payer. Never run two keepers, or a
keeper and a manual post, on the same payer; `PayerLock` prevents a second process.

## Verify

The scan that adopts it is the first one after the account has been seen unchanged for 300
blocks (about 2 minutes), and scans run at start and then every 12 ticks of the watch
cadence: one hour under the agent (`--watch 300`), 4 minutes with `--watch 20`. The scan
state lives in the process, so a restart starts the count again. Then the log shows
`open encoded VAAs []`, and the account no longer exists:

```sh
solana account <encoded vaa> -u devnet     # "AccountNotFound"
```

Drill: [drills.md](drills.md), orphan.
