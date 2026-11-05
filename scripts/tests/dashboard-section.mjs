// `make dashboard-check`: varswap's section, generated from a real `vol-index --history --json`
// of the first two devnet days (docs/dashboard/fixtures/vol-index/), and scripts/dashboard.sh
// end to end with a mock varswap-admin, so no RPC is touched. cli/tests/index.rs keeps that
// fixture equal to what the CLI prints for those records.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { buildIndex, buildSection } from "../dashboard-section.mjs";
import { validate } from "../dashboard-validate.mjs";

const file = (path) => fileURLToPath(new URL(path, import.meta.url));
const schema = JSON.parse(readFileSync(file("../../docs/dashboard/section-1.schema.json"), "utf8"));
const historyPath = file("../../docs/dashboard/fixtures/vol-index/history.json");
const history = () => JSON.parse(readFileSync(historyPath, "utf8"));
const WINDOW_1 = "CjtkCZY1BEzN8MFgrbViGtW9KEUqP6vMKu7CCPRqiATG";
const meta = { name: "SOL/USD", commit: "abc1234", generatedAt: "2026-11-04T18:30:00Z" };
const build = (h = history(), m = meta) => buildSection(h, m);

test("the section of the first two devnet days is valid and carries their values", () => {
  const s = build();
  assert.deepEqual(validate(s, schema), []);
  assert.equal(s.cluster, "devnet");
  assert.deepEqual(s.source, { commit: "abc1234", slot: 518646301, program_ids: ["BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9"] });
  assert.deepEqual(s.status, { level: "ok", message: "" });
  assert.deepEqual(s.tiles[0], {
    id: "sigma-1d", label: "SOL/USD σ 1d", value: "0.736999341883", unit: "annualized",
    quality: "complete", note: "1/1 days to 2026-11-04",
  });
  assert.deepEqual(s.tiles.slice(1).map((t) => [t.id, t.value, t.quality, t.note]), [
    ["sigma-7d", null, "insufficient", "2/7 days to 2026-11-04"],
    ["sigma-30d", null, "insufficient", "2/30 days to 2026-11-04"],
  ]);
  assert.deepEqual(s.series.map((x) => x.id), ["sigma-1d", "sigma-7d", "sigma-30d"]);
  assert.deepEqual(s.series[0].points, [
    ["2026-11-03T17:00:00Z", "0.435327760666"],
    ["2026-11-04T17:00:00Z", "0.736999341883"],
  ]);
  assert.deepEqual(s.series[0].quality, ["complete", "complete"]);
  assert.deepEqual(s.series[1].points.map((p) => p[1]), [null, null]);
  assert.deepEqual(s.tables[0].rows[0], ["1", "2026-11-03", WINDOW_1, "complete", null, "0.736999341883", "0", "0", "0"]);
  assert.equal(s.tables[0].rows.length, 2);
  assert.equal(s.tables[0].href_columns.address, "https://explorer.solana.com/address/{}?cluster=devnet");
  assert.deepEqual(s.links.map((l) => l.href), [
    "index/sol-usd-hourly/index.json",
    "index/sol-usd-hourly/sync.json",
    "https://github.com/joaquinbejar/varswap",
  ]);
});

// The last day of the fixture with its 1d and 7d values replaced by an exclusion.
function withExcluded(reason) {
  const h = history();
  const last = h.history.at(-1);
  const excluded = [{ day: 1, window: WINDOW_1, reason }];
  Object.assign(last.values[0], { usable: 0, label: "insufficient", sigma: null, sigma_raw: null, rv: null, rv_raw: null, excluded });
  Object.assign(last.values[1], { usable: 1, excluded });
  return h;
}

test("status is warn when a 7d day is excluded, and says which", () => {
  for (const reason of ["degraded", "unknown", "missing", "open"]) {
    const s = build(withExcluded(reason));
    assert.equal(s.status.level, "warn", reason);
    assert.equal(s.status.message, `the 7d value excludes day 1: ${reason}`);
    assert.deepEqual(validate(s, schema), []);
  }
});

test("a day before the series is not an exclusion: partial values alone stay ok", () => {
  assert.equal(history().history.at(-1).values[1].before_series > 0, true);
  assert.equal(build().status.level, "ok");
});

test("status is stale when the newest final day is older than 48 h, just past and not at it", () => {
  const at = (generatedAt) => build(history(), { ...meta, generatedAt }).status;
  assert.equal(at("2026-11-06T17:00:00Z").level, "ok");
  assert.deepEqual(at("2026-11-06T17:00:01Z"), {
    level: "stale",
    message: "the newest final day ended 48 h ago (2026-11-04T17:00:00Z)",
  });
});

test("stale wins over warn", () => {
  const h = withExcluded("degraded");
  assert.equal(build(h, { ...meta, generatedAt: "2026-11-09T00:00:00Z" }).status.level, "stale");
});

test("open and missing days are not final, so they age the section; degraded and unknown are final", () => {
  // Day 1 has no final window: the age runs from day 0's end, 2026-11-03T17:00Z.
  const at49h = { ...meta, generatedAt: "2026-11-05T18:00:00Z" };
  for (const reason of ["open", "missing"]) assert.equal(build(withExcluded(reason), at49h).status.level, "stale", reason);
  for (const reason of ["degraded", "unknown"]) assert.equal(build(withExcluded(reason), at49h).status.level, "warn", reason);
});

test("a series with no finished day is stale, with no values and still valid", () => {
  const h = { ...history(), history: [], last_day: null };
  const s = build(h);
  assert.deepEqual(validate(s, schema), []);
  assert.deepEqual(s.status, { level: "stale", message: "no final day yet" });
  assert.deepEqual(s.tiles.map((t) => t.value), [null, null, null]);
  assert.deepEqual(s.series[0].points, []);
});

test("missed samples show on the tile", () => {
  const h = history();
  h.history.at(-1).values[0].missed_samples = 3;
  assert.equal(build(h).tiles[0].note, "1/1 days to 2026-11-04; 3 missed samples");
});

test("samples with no block time have their own column, apart from the late ones", () => {
  const h = history();
  Object.assign(h.history.at(-1).values[0], { late_samples: 2, late_unknown: 5 });
  const { columns, rows } = build(h).tables[0];
  assert.deepEqual(columns.slice(-2), ["late", "no time"]);
  assert.deepEqual(rows[0].slice(-2), ["2", "5"]);
});

test("the table keeps the newest 100 days, newest first", () => {
  const h = history();
  const day = h.history[0];
  h.history = Array.from({ length: 130 }, (_, i) => ({ ...day, day: i }));
  const rows = build(h).tables[0].rows;
  assert.equal(rows.length, 100);
  assert.equal(rows[0][0], "129");
  assert.equal(rows[99][0], "30");
  assert.equal(build(h).series[0].points.length, 130);
});

test("a field the generator needs, gone from the CLI's output, is an error and not a hole", () => {
  const drop = (edit) => { const h = history(); edit(h); return h; };
  const cases = [
    (h) => delete h.slot,
    (h) => delete h.series.cluster,
    (h) => delete h.history[1].window,
    (h) => h.history[1].values.splice(1, 1),
    (h) => delete h.history[1].values[0].sigma,
    (h) => delete h.history[1].values[0].late_unknown,
    (h) => delete h.history[1].values[1].excluded,
  ];
  for (const edit of cases) assert.throws(() => build(drop(edit)), /has no/, edit.toString());
});

test("the published index is the CLI's history with the commit, the time and the records beside it", () => {
  const h = history();
  const index = buildIndex(h, { ...meta, records: ["records/a.json"] });
  assert.equal(index.generator_commit, "abc1234");
  assert.equal(index.generated_at, meta.generatedAt);
  assert.deepEqual(index.records, ["records/a.json"]);
  for (const [k, v] of Object.entries(h)) assert.deepEqual(index[k], v, k);
});

// scripts/dashboard.sh with a mock varswap-admin: series-sync copies the fixture's records and
// vol-index prints the fixture's history; MOCK_FAIL names a subcommand that fails.
function run(out, env = {}) {
  const t = mkdtempSync(join(tmpdir(), "dashboard-test-"));
  const admin = join(t, "varswap-admin");
  writeFileSync(admin, `#!/bin/sh
[ "\${MOCK_FAIL:-}" = "$1" ] && { echo "mock failure" >&2; exit 1; }
printf '%s\\n' "$*:$SOLANA_URL" >>"$MOCK_CALLS"
case $1 in
series-sync) cp "$MOCK_RECORDS"/* "$4"/; echo '{}' ;;
vol-index) cat "$MOCK_HISTORY" ;;
*) exit 9 ;;
esac
`);
  chmodSync(admin, 0o755);
  const calls = join(t, "calls");
  const result = spawnSync("sh", [file("../dashboard.sh"), out], {
    encoding: "utf8",
    env: {
      PATH: process.env.PATH,
      VARSWAP_ADMIN_BIN: admin,
      MOCK_CALLS: calls,
      MOCK_RECORDS: file("../../docs/dashboard/fixtures/vol-index/records"),
      MOCK_HISTORY: env.MOCK_HISTORY ?? historyPath,
      ...env,
    },
  });
  const called = existsSync(calls) ? readFileSync(calls, "utf8") : "";
  rmSync(t, { recursive: true });
  return { ...result, called };
}

function scratch(body) {
  const out = mkdtempSync(join(tmpdir(), "dashboard-out-"));
  try {
    body(out);
  } finally {
    rmSync(out, { recursive: true });
  }
}

test("dashboard.sh writes the section, the index, the records and the shell, and the section is valid", () => {
  scratch((out) => {
    const r = run(out);
    assert.equal(r.status, 0, r.stderr);
    const section = JSON.parse(readFileSync(join(out, "varswap/section.json"), "utf8"));
    assert.deepEqual(validate(section, schema), []);
    assert.equal(section.tiles[0].value, "0.736999341883");
    assert.match(section.source.commit, /^[0-9a-f]{40}$/);
    assert.match(section.generated_at, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/);
    const dir = join(out, "varswap/index/sol-usd-hourly");
    const index = JSON.parse(readFileSync(join(dir, "index.json"), "utf8"));
    assert.equal(index.generator_commit, section.source.commit);
    assert.equal(index.records.length, 2);
    for (const name of index.records) assert.ok(existsSync(join(dir, name)), name);
    assert.ok(!index.records.includes("records/sync.json"));
    assert.equal(readFileSync(join(dir, "sync.json"), "utf8"), readFileSync(join(dir, "records/sync.json"), "utf8"));
    for (const f of ["index.html", "dashboard.mjs", "dashboard.css", "sections.json"]) {
      assert.equal(readFileSync(join(out, f), "utf8"), readFileSync(file(`../../site/${f}`), "utf8"), f);
    }
    assert.deepEqual(section.links.slice(0, 2).map((l) => existsSync(join(out, "varswap", l.href))), [true, true]);
  });
});

test("dashboard.sh defaults to the public RPC, and passes no URL on the command line", () => {
  scratch((out) => {
    const r = run(out);
    assert.equal(r.status, 0, r.stderr);
    const lines = r.called.trim().split("\n");
    assert.equal(lines.length, 2);
    for (const line of lines) {
      const [args, ...url] = line.split(":");
      assert.equal(url.join(":"), "https://api.devnet.solana.com");
      assert.ok(!args.includes("http"), args);
    }
  });
});

test("a failure at any step leaves the published section and index as they were", () => {
  scratch((out) => {
    assert.equal(run(out).status, 0);
    const section = join(out, "varswap/section.json");
    const index = join(out, "varswap/index/sol-usd-hourly/index.json");
    const before = [readFileSync(section, "utf8"), readFileSync(index, "utf8")];
    const bad = join(out, "bad-history.json");
    writeFileSync(bad, '{"series": {}}');
    for (const env of [{ MOCK_FAIL: "series-sync" }, { MOCK_FAIL: "vol-index" }, { MOCK_HISTORY: bad }]) {
      const r = run(out, env);
      assert.notEqual(r.status, 0, JSON.stringify(env));
      assert.deepEqual([readFileSync(section, "utf8"), readFileSync(index, "utf8")], before, JSON.stringify(env));
    }
  });
});

test("a section the schema rejects is an error, and nothing is written", () => {
  scratch((out) => {
    const bad = join(out, "bad-history.json");
    writeFileSync(bad, JSON.stringify({ ...history(), slot: null }));
    const r = run(out, { MOCK_HISTORY: bad });
    assert.equal(r.status, 1);
    assert.match(r.stderr, /the section fails its schema: \$\.source\.slot/);
    assert.ok(!existsSync(join(out, "varswap/section.json")));
    assert.ok(!existsSync(join(out, "varswap/index/sol-usd-hourly/index.json")));
  });
});

test("dashboard.sh needs an out dir and a built varswap-admin", () => {
  const usage = spawnSync("sh", [file("../dashboard.sh")], { encoding: "utf8" });
  assert.equal(usage.status, 2);
  scratch((out) => {
    const r = spawnSync("sh", [file("../dashboard.sh"), out], {
      encoding: "utf8",
      env: { PATH: process.env.PATH, VARSWAP_ADMIN_BIN: join(out, "nope") },
    });
    assert.equal(r.status, 1);
    assert.match(r.stderr, /missing: cargo build/);
  });
});
