# The daily series' records

The records of the devnet series `sol-usd-hourly` (`docs/index/series/sol-usd-hourly.json`), one
per day from day 0 (2026-11-02 17:00Z), as `series-sync` writes them: `<day>-<window>.json`. Each
is an I7 record as described in [`../README.md`](../README.md), so it verifies offline, or a
`missing` stub for a day nobody created.

The dashboard's site serves the same records (`docs/dashboard.md`, "On GitHub Pages"). This copy
lets the repo keep them whatever happens to the site, and the Pages workflow restores it before
each run, so a day the RPC no longer serves the history of is never read again. A day in both
places must be the same record except for `account.slot`, or the run fails.

It is brought up to date monthly by a normal PR, with the records the site publishes:

```sh
node scripts/dashboard-restore.mjs restore <tmp-dir> --site https://joaquinbejar.github.io/varswap
cp <tmp-dir>/published/*.json docs/windows/sol-usd-hourly/
cargo test -p varswap-cli --test index committed_series_records
```

The test checks that the files are the days from 0 with no gap, that each record verifies
offline, and that the series reads every file as its own.
