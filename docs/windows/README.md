# Window records

One record per finalized or degraded devnet window, built and checked with `varswap-admin`
(`docs/PLAN-improvements.md`, I7). This is not a continuous index: each record stands alone,
with the evidence to recompute it.

| Window | State | Coverage | Realized variance | Label | Compressed runs |
| --- | --- | ---: | ---: | --- | ---: |
| [`9P3vhP8pe8WcebFXZ77njPHddnCtLLFGFHLiAucQ2Wz8`](2026-10-30-sol-usd-demo-9P3vhP8p.json) | finalized | 11/11 | 0.196050085777 | complete | 0 |
| [`7s5KJ79XRwBzeL8t42ut6RkQ1yLhrRMg8z5dHQtNXTeA`](2026-10-31-sol-usd-hourly-7s5KJ79X.json) | finalized | 25/25 | 0.068149838107 | complete | 0 |

The table is `varswap-admin window-table docs/windows/*.json`, which verifies each record
offline before printing its row.

## What a record holds

- `account`: the window account's bytes (hex), owner and the slot they were read at, at
  finalized commitment.
- `history`: every transaction that touched the window, back to its `create_window`
  (`reaches_create_window`), from `getSignaturesForAddress` and `getTransaction` at finalized
  commitment: signature, slot, block time, success, and for each successful `sample`
  instruction on this window its event (`varswap:sample first last publish_time price expo`)
  and instruction position.
- `derived`: state, coverage, `sum_sq` and the realized variance (raw and decimal), the runs,
  the number of compressed runs and the longest, and the label.

Labels: `complete` (every run is one bucket), `compressed` (some run spans several buckets: an
outage compressed into one return), `degraded` (no realized variance) and `unknown` (the
history does not reach `create_window`, or its events do not reconcile with the account; the
realized variance is still reported, the quality is not asserted). Reconciliation: the
successful `sample` events partition the filled buckets exactly, and each run's price and
exponent equal the stored ones.

Supported history: `sample` as a top-level instruction. Events are matched to instructions by
counting only this program's depth-1 invocations in the logs (other programs, precompiles
included, may log none), and an event counts only when this program is the only frame, so a
nested program's matching text is never attributed. Truncated logs, or a `sample` instruction
without its event, make the record `unknown` (`events_complete: false`). A window sampled
through a CPI is outside this boundary: its buckets stay uncovered and the record is `unknown`.

## Build and verify

```sh
export SOLANA_URL=https://api.devnet.solana.com
export VARSWAP_PROGRAM_ID=BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9
varswap-admin export-window <window> > docs/windows/<date>-<feed>-<name>.json
varswap-admin verify-window docs/windows/<file>.json          # offline
varswap-admin verify-window --rpc docs/windows/<file>.json    # against the cluster
```

- **Offline** checks the record against itself: the bytes decode to a valid window, `sum_sq`
  and the realized variance recompute from its prices with `varswap-core`, the events
  reconcile, and every derived field, the label included, is what the bytes and the events
  give. That is consistency only. It authenticates nothing against the chain: a consistent
  forgery passes it. CI runs it on every committed record (`cli/tests/record.rs`).
- **`--rpc`** also fetches the account and every listed transaction again at finalized
  commitment and requires them equal, and requires that the cluster lists no transaction of the
  window missing from the record.

## Negative controls

Offline, in CI (`cli/tests/record.rs`), each failing for its stated reason: a price byte edited
in the account (`sum_sq` no longer matches), a deleted event (a filled bucket no event covers),
an event whose run bounds or price were edited, a forged label or realized variance, a
`derived` that is missing or not an object, a history that claims to reach `create_window`
without listing it, and an honest history without `create_window` (its label must be
`unknown`). Against a fake cluster: `--rpc` fails on a listed signature the record lacks
(before fetching anything, even if that transaction cannot be fetched) and on a listed
transaction the cluster cannot return. Parser unit tests: a precompile before `sample`, a
nested program logging the event text, truncated logs.

With `--rpc`, run against devnet on 2026-11-02 on edited copies of the demo window's record:

| Control | Offline | `--rpc` |
| --- | --- | --- |
| `publish_time` of one event edited, run and price kept | passes (consistent forgery) | fails: `3QaW1FW6…: differs from the cluster's` |
| A `sample` transaction of the 2026-10-31 window copied into the record | fails: the event's bucket holds another price, the label becomes `unknown` | fails (same) |
| The `finalize` signature `4HoUsuSY…` removed from the list | passes (no event lost) | fails: `4HoUsuSY…: on the cluster, missing from the bundle` |
