# Shared dashboard: the section format

A read-only dashboard of the Solana projects' devnet state: a static page and JSON files, with no
backend. Each repo publishes its own `section.json`; varswap owns the shell that renders them
(`docs/PLAN-phase2.md`, P3). The format was agreed on the bus with metalbook (#52648) and quotebook
(#52690).

The JSON Schema is [`docs/dashboard/section-1.schema.json`](dashboard/section-1.schema.json), and
[`docs/dashboard/fixtures/section-ok.json`](dashboard/fixtures/section-ok.json) uses every field.
Each repo validates its own output against the schema in its CI.

## `section.json`, version 1

```json
{
  "schema": "solana-projects-dashboard/section/1",
  "repo": "varswap",
  "title": "varswap: realized volatility",
  "cluster": "devnet",
  "generated_at": "2026-11-04T18:05:00Z",
  "source": { "commit": "<git sha>", "slot": 0, "program_ids": ["BWqs…"] },
  "status": { "level": "ok", "message": "" },
  "tiles": [ { "id": "rv-7d", "label": "SOL/USD σ 7d", "value": "0.512345678901",
               "unit": "annualized", "quality": "partial", "note": "6/7 days" } ],
  "series": [ { "id": "sigma-1d", "label": "σ 1d", "unit": "annualized",
                "points": [["2026-11-03T17:00:00Z", "0.498765432109"]],
                "quality": ["complete"] } ],
  "tables": [ { "id": "windows", "columns": ["day", "address", "label"],
                "rows": [["2026-11-02", "7ywX…", "complete"]],
                "href_columns": { "address": "https://explorer.solana.com/address/{}?cluster=devnet" } } ],
  "links": [ { "label": "records", "href": "index/sol-usd-hourly/" } ]
}
```

- **Numbers are decimal strings**, so no float rounding happens between repos. A series point's
  value is a decimal string or `null`, and a point with no value leaves a gap. A tile's `value` is
  shown as text, and `null` shows as no value.
- **Times** are RFC 3339 in UTC, ending in `Z`.
- `status.level` is `ok`, `warn` or `stale`, and `quality` is free text per repo, shown as a badge.
- **Unknown fields are ignored**, so a repo can add fields without breaking the shell. A breaking
  change bumps the version, and the shell renders each version it knows.
- **The shell has no row limit** and scrolls long tables. A section should keep a table to about
  100 rows and link the rest.

## Everything in a section is untrusted

A section is another repo's data. The shell renders all of it as text, never as HTML, and these
rules hold whatever a section says. The schema carries the same link rules, but the shell does not
validate at run time, so it enforces them itself.

### Links

- `links[].href` is a link only if it is an absolute `https://` URL, or a relative path with no
  scheme, no leading `/` or `\`, no `\` anywhere, no `.` or `..` segment, also right before a `?`
  or `#`, and no encoded slash (percent-encoded forms included). No form may hold a control
  character: a URL parser strips some of them, so `\u0000javascript:` would become `javascript:`.
  So `javascript:`, `data:`, `http:` and protocol-relative `//host` values render as plain text.
- A table's `href_columns` maps a column name to a URL template with `{}`. The shell URL-encodes
  the cell into `{}` and makes a link only when the template starts with `https://`; anything else
  renders as plain text.

### Freshness

`status.level` is written by a generator, so if a workflow stops running, its file keeps saying
`ok` forever. The shell computes freshness itself, with the browser's clock:

- a section whose `generated_at` is older than its `max_age_hours`, or more than 1 h in the
  future, is shown as `stale` with its age, whatever `status.level` says;
- otherwise the shell shows the section's own level;
- a `generated_at` that does not parse, or is not a date in the calendar (`2026-02-31`), gives an
  error card.

## `sections.json`

The shell's list, owned by varswap:

```json
[ { "repo": "varswap", "url": "varswap/section.json", "max_age_hours": 48 },
  { "repo": "quotebook", "url": "https://…/section.json", "max_age_hours": 48, "pending": true } ]
```

The deadline lives here and not in the section, so a section cannot extend its own. All three
sections publish daily, so each starts at 48: one missed run is tolerated, two are not. An entry
with `pending` is shown as such and not fetched, until its repo publishes and the flag is removed.

| Repo | Published at | Runs |
| --- | --- | --- |
| varswap | same site as the shell: `https://joaquinbejar.github.io/varswap/varswap/section.json` | 18:30Z |
| metalbook | pending its first run (from about November 16); `https://joaquinbejar.github.io/metalbook/metalbook/section.json` | 22:30Z |
| quotebook | pending its confirmation; expected `https://joaquinbejar.github.io/quotebook/quotebook/section.json` | expected 23:00Z |

## The varswap section

`scripts/dashboard.sh <out-dir>` publishes the shell and this repo's section (`SOLANA_URL` defaults
to devnet's public RPC; `cargo build --release -p varswap-cli` first). It holds no key, signs nothing
and makes no Hermes request.

```text
<out-dir>/current                   symlink to the published generation: serve or deploy this
<out-dir>/generations/gen.XXXXXX/   the live generation, the one before it, and any being built
<out-dir>/records/                  series-sync's state; a rerun resumes from what is here

and in a generation:
index.html, dashboard.mjs, dashboard.css, sections.json
varswap/section.json
varswap/index/sol-usd-hourly/index.json     every day, the generator's commit and the time
varswap/index/sol-usd-hourly/sync.json
varswap/index/sol-usd-hourly/records/       the records, and sync.json as series-sync's state
```

**Publication** is one step. A generation is built and checked apart, and goes live when
`scripts/dashboard-publish.mjs` renames the `current` symlink, which is atomic: a reader of `current`
gets a whole generation, the new one or the old one, and never an index from one run with a section
from another. It checks first that the shell, the section, each of the section's local links and each
record the index lists are all there. A page fetched across that rename can still mix two generations
over its separate requests; the next load corrects it.

**One run at a time.** A run holds `<out-dir>/.lock` from before the sync to the end, and a second one
exits non zero without touching anything. A lock is never taken over, even when its process is gone:
two runs taking over the same dead lock at once would both publish and prune, and one could remove the
generation the other had just made live. So a run killed with `kill -9`, or cut by a reboot, leaves the
lock, and every later run exits 1 with an error that names the pid and says whether it is running.
Remove the lock by hand once no run is active; the next publication then removes what the dead run left
in `generations/`. SIGINT and SIGTERM remove the lock and the build. As a second guard, the pruning
after a publication reads `current` again before each removal, so it never removes the generation that
is live at that moment.

`scripts/dashboard-section.mjs` builds the section from `vol-index --history --json`:

- **Tiles and series**: σ 1d, 7d and 30d, with the labels of `docs/devnet.md`. A value that is
  `insufficient` is `null`, shown as no value or as a gap, never as a number.
- **Windows table**: one row per day, newest first. `late` counts only samples known to be late, and
  `no time` the samples with no block time, so `late` 0 proves nothing while the other is above 0.
- **`status.level`**, computed from the data at generation; the shell applies its own deadline on
  top. `stale` when the newest final day ended more than 48 h ago. A day is final if its window
  finished, including a `degraded` or `unknown` one; an `open` or `missing` day is not. Otherwise
  `warn` when the latest 7d value excludes a day, and the message names it. A day before the series
  started is not an exclusion: it makes the value `partial` or `insufficient` and leaves `ok`.
- **Failure**: any failure before the rename exits non zero and leaves `current` as it was, so the
  shell shows an old section as stale and not a partial one as fresh. So does a field missing from
  the CLI's output, and a section the schema rejects. Failing to remove an old generation after the
  rename is only a warning: the new one is live.

Exporting a window twice gives the same record except for `account.slot`, the slot of the read: on
2026-11-04 a second export of days 0 and 1 differed in that line alone. Comparing a published record
with a new export has to ignore that field.

## On GitHub Pages

`.github/workflows/dashboard.yml` publishes the shell and the varswap section at
`https://joaquinbejar.github.io/varswap/`, daily at 18:30Z (after the 17:00Z end of each window,
with margin for its last bucket and finality) and by hand. It runs on a GitHub hosted runner from
`main`, reads devnet's public RPC, holds no secret, signs nothing and makes no Hermes request. Its
token can read the repo in the build job and write Pages in the deploy job, nothing else. It adds no
required check: a failed run leaves the site as it was, and the shell shows the section as stale
once it is older than its 48 h deadline.

A runner starts empty, so each run:

1. restores the records: `scripts/dashboard-restore.mjs restore` fetches the site's `index.json`,
   every record it lists and `records/sync.json` into `<out>/records/`, together with the copy the
   repo keeps in `docs/windows/sol-usd-hourly/`. A day in both places must be the same record except
   for `account.slot`. `series-sync` never rewrites a day that has a file, so those records pass
   through unchanged and only later days are read from the chain. A site with no `index.json` fails
   the run, unless the run was started by hand with `first_deploy`: otherwise a broken site would make
   the run read every day again, and a day whose history the RPC no longer serves would come back
   `unknown`. Any other error of the site fails the run too;
2. runs `scripts/dashboard.sh <out>`;
3. checks that records are append only: `scripts/dashboard-restore.mjs check` fails unless every
   restored record is in the new generation and its `index.json`, unchanged except for `account.slot`;
4. deploys `<out>/current` with the official Pages actions, pinned by SHA.

The repo's copy, `docs/windows/sol-usd-hourly/`, is brought up to date monthly by a normal PR (its
README has the commands), so the records stay reviewable and survive whatever happens to the site.

### Without Pages

`scripts/dashboard-agent.sh install <name>` runs the same `scripts/dashboard.sh` under a LaunchAgent,
every hour, into `~/.local/share/varswap-keeper/<name>/out/`; serve its `current` with
`python3 -m http.server -d <dir>/out/current`. It uses the checkout it was installed from and that
checkout's `varswap-admin`, and keeps its records between runs, so it needs no restore. On the Studio
it shares the public RPC allowance with the keeper and the watcher: before a first run that has to
catch up on many days, check the watcher's `rpc` alerts.

## Checking

`make dashboard-check` runs four files with `node --test` and no npm package:

- `scripts/tests/dashboard-schema.mjs`: the fixtures are valid, each required field is required,
  wrong values fail, and every accepted and rejected form of `href` is tested.
- `scripts/tests/dashboard-shell.mjs`: the shell's link and freshness decisions, against fixtures
  with an old section that still says `ok`, a `generated_at` in the future, and dangerous links; and
  that the page loads only its own files and puts nothing in as HTML.
- `scripts/tests/dashboard-section.mjs`: the section generated from a real `vol-index` output of the
  first two devnet days (`docs/dashboard/fixtures/vol-index/`), each status, and
  `scripts/dashboard.sh` end to end with a mock `varswap-admin`: a failure at each step, two writers
  at once, a run killed mid way. `cli/tests/index.rs` keeps that
  output equal to what the CLI prints for those records, so the two cannot drift apart unseen.
- `scripts/tests/dashboard-pages.mjs`: the restore and the append only check, against a fake site
  serving the same two days: a site with no index, a listed record missing, an error that stays, a
  name that is not a record's, the repo's copy disagreeing with the site's, and a published record
  that changed, went or is no longer listed.

`cli/tests/index.rs` also checks the repo's copy of the series: the days from 0 with no gap, each
record verified offline, and every file read by the series as its own.

A sibling repo can vendor the schema at a commit and validate with whatever its CI already has.

## The shell

`site/`: `index.html`, `dashboard.mjs`, `dashboard.css` and `sections.json`, hand written, with no
framework, build step, CDN, external font or analytics. The browser reads those files and the
sections, and nothing else: no RPC. `dashboard.mjs` is a module so the same file loads in the page
and in `node --test`. It needs a static server (`python3 -m http.server` in the site's directory, or
GitHub Pages): a `file://` page cannot fetch. One section that fails to load or parse shows an error
card, and the others are unaffected.

A chart's values are checked before anything is drawn: a time that does not parse, a value that is
not a finite decimal, or values too far apart or too small for an axis give the section's error card.
A relative link must resolve to the section's own directory, on the protocol the section was served
with: `https`, or `http` for a local server.

Series with the same `unit` share one chart and one axis, up to 8 per chart; a section with more
gets another chart, never a generated color. Each chart has a table view, so every value can be read
without hovering. `status.level` is shown with its icon and label, never by color alone.
