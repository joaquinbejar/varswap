# Window with missing buckets before its deadline

A bucket can be filled until `end + backfill_deadline` (24 h on the devnet SOL/USD feed):
Hermes serves historical updates by timestamp, and the canonical print is unique, so a late
fill is the same as an on-time one.

## Detect

`varswap-admin show-window <window>`: `missing` is not empty and the boundary is past.

## Fix

Run a keeper on the window (the agent, or by hand for a single pass):

```sh
varswap-keeper <window>                     # one tick: posts every missing bucket it can
```

The keeper posts every missing bucket whose canonical print it can fetch and the program would
accept. If a bucket stays missing after a tick (the log counts it as `skipped`, or prints the
error for the window), the usual reasons are:

- no canonical print within the guard (an outage longer than `tolerance`) or a print outside
  the confidence bound: nothing can fill it; the window will degrade at the deadline
  ([degraded-window.md](degraded-window.md));
- Hermes 429: wait ([hermes.md](hermes.md)); with many buckets missing the backfill takes a
  few minutes at varswap's 3 requests per minute.

## Verify

`show-window`: `missing: []`, then `state: finalized` (the keeper finalizes a full window).
Drill: [drills.md](drills.md), restart (the restart drill misses and backfills a bucket).
