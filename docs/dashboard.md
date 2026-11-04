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
- a `generated_at` that does not parse gives an error card.

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
| varswap | same site as the shell | 18:30Z |
| metalbook | pending its first run (from about November 16); `https://joaquinbejar.github.io/metalbook/metalbook/section.json` | 22:30Z |
| quotebook | pending its confirmation; expected `https://joaquinbejar.github.io/quotebook/quotebook/section.json` | expected 23:00Z |

## Checking

`make dashboard-check` runs two files with `node --test` and no npm package:

- `scripts/tests/dashboard-schema.mjs`: the fixtures are valid, each required field is required,
  wrong values fail, and every accepted and rejected form of `href` is tested.
- `scripts/tests/dashboard-shell.mjs`: the shell's link and freshness decisions, against fixtures
  with an old section that still says `ok`, a `generated_at` in the future, and dangerous links; and
  that the page loads only its own files and puts nothing in as HTML.

A sibling repo can vendor the schema at a commit and validate with whatever its CI already has.

## The shell

`site/`: `index.html`, `dashboard.mjs`, `dashboard.css` and `sections.json`, hand written, with no
framework, build step, CDN, external font or analytics. The browser reads those files and the
sections, and nothing else: no RPC. `dashboard.mjs` is a module so the same file loads in the page
and in `node --test`. It needs a static server (`python3 -m http.server` in the site's directory, or
GitHub Pages): a `file://` page cannot fetch. One section that fails to load or parse shows an error
card, and the others are unaffected.

Series with the same `unit` share one chart and one axis, up to 8 per chart; a section with more
gets another chart, never a generated color. Each chart has a table view, so every value can be read
without hovering. `status.level` is shown with its icon and label, never by color alone.
