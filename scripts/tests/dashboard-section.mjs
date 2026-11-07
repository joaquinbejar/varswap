// `make dashboard-check`: varswap's section, generated from a real `vol-index --history --json`
// of the first two devnet days (docs/dashboard/fixtures/vol-index/), and scripts/dashboard.sh
// end to end with a mock varswap-admin, so no RPC is touched. cli/tests/index.rs keeps that
// fixture equal to what the CLI prints for those records.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, readlinkSync, rmSync,
  renameSync, statSync, symlinkSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { checkGeneration, publish } from "../dashboard-publish.mjs";
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
// vol-index prints the fixture's history; MOCK_FAIL names a subcommand that fails and MOCK_SLEEP
// holds series-sync. `cp` and `node` in PATH wrap the real ones: MOCK_CP_FAIL fails a cp whose
// arguments contain it, and MOCK_AFTER_PUBLISH fails the publisher after it has done its work.
function mock(env = {}) {
  const t = mkdtempSync(join(tmpdir(), "dashboard-test-"));
  const tool = (name, body) => {
    writeFileSync(join(t, name), `#!/bin/sh\n${body}`);
    chmodSync(join(t, name), 0o755);
  };
  tool("varswap-admin", `[ "\${MOCK_FAIL:-}" = "$1" ] && { echo "mock failure" >&2; exit 1; }
printf '%s\\n' "$*:$SOLANA_URL" >>"$MOCK_CALLS"
case $1 in
series-sync) [ -z "\${MOCK_SLEEP:-}" ] || sleep "$MOCK_SLEEP"; /bin/cp "$MOCK_RECORDS"/* "$4"/; echo '{}' ;;
vol-index) cat "$MOCK_HISTORY" ;;
*) exit 9 ;;
esac
`);
  tool("cp", `case "$*" in *"\${MOCK_CP_FAIL:-no cp fails}"*) echo "mock cp failure" >&2; exit 1 ;; esac
exec /bin/cp "$@"
`);
  tool("node", `${process.execPath} "$@" || exit $?
case "$1:\${MOCK_AFTER_PUBLISH:-}" in *dashboard-publish.mjs:1) exit 1 ;; esac
`);
  const calls = join(t, "calls");
  return {
    env: {
      PATH: `${t}:${process.env.PATH}`,
      VARSWAP_ADMIN_BIN: join(t, "varswap-admin"),
      MOCK_CALLS: calls,
      MOCK_RECORDS: file("../../docs/dashboard/fixtures/vol-index/records"),
      MOCK_HISTORY: historyPath,
      ...env,
    },
    called: () => (existsSync(calls) ? readFileSync(calls, "utf8") : ""),
    done: () => rmSync(t, { recursive: true }),
  };
}

function run(out, env = {}) {
  const m = mock(env);
  const result = spawnSync("sh", [file("../dashboard.sh"), out], { encoding: "utf8", env: m.env });
  const called = m.called();
  m.done();
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

const generationsIn = (out) => readdirSync(join(out, "generations")).sort();

// Every file `current` serves, with its contents, and where the link points.
function published(out) {
  const files = {};
  const walk = (dir, prefix) => {
    for (const name of readdirSync(dir).sort()) {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) walk(path, `${prefix}${name}/`);
      else files[`${prefix}${name}`] = readFileSync(path, "utf8");
    }
  };
  walk(join(out, "current"), "");
  return { target: readlinkSync(join(out, "current")), files };
}

test("dashboard.sh publishes the section, the index, the records and the shell, and the section is valid", () => {
  scratch((out) => {
    const r = run(out);
    assert.equal(r.status, 0, r.stderr);
    const live = join(out, "current");
    assert.match(readlinkSync(live), /^generations\/gen\.\w+$/);
    assert.equal(statSync(live).mode & 0o777, 0o755);
    const section = JSON.parse(readFileSync(join(live, "varswap/section.json"), "utf8"));
    assert.deepEqual(validate(section, schema), []);
    assert.equal(section.tiles[0].value, "0.736999341883");
    assert.match(section.source.commit, /^[0-9a-f]{40}$/);
    assert.match(section.generated_at, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/);
    const dir = join(live, "varswap/index/sol-usd-hourly");
    const index = JSON.parse(readFileSync(join(dir, "index.json"), "utf8"));
    assert.equal(index.generator_commit, section.source.commit);
    assert.equal(index.generated_at, section.generated_at);
    assert.equal(index.records.length, 2);
    for (const name of index.records) assert.ok(existsSync(join(dir, name)), name);
    assert.ok(!index.records.includes("records/sync.json"));
    assert.equal(readFileSync(join(dir, "sync.json"), "utf8"), readFileSync(join(dir, "records/sync.json"), "utf8"));
    for (const f of ["index.html", "dashboard.mjs", "dashboard.css", "sections.json"]) {
      assert.equal(readFileSync(join(live, f), "utf8"), readFileSync(file(`../../site/${f}`), "utf8"), f);
    }
    assert.deepEqual(section.links.slice(0, 2).map((l) => existsSync(join(live, "varswap", l.href))), [true, true]);
    assert.ok(!existsSync(join(out, ".lock")));
    assert.ok(!existsSync(join(out, ".current.tmp")));
    assert.ok(existsSync(join(out, "records/sync.json")));
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

test("a record left half written by a killed run is not published", () => {
  scratch((out) => {
    mkdirSync(join(out, "records"));
    writeFileSync(join(out, "records/0002-x.json.tmp"), "{");
    assert.equal(run(out).status, 0);
    assert.deepEqual(Object.keys(published(out).files).filter((f) => f.endsWith(".tmp")), []);
  });
});

test("a failure at any step before the rename leaves what is published as it was, and no build behind", () => {
  scratch((out) => {
    assert.equal(run(out).status, 0);
    const before = published(out);
    const bad = join(out, "bad-history.json");
    writeFileSync(bad, '{"series": {}}');
    const steps = [
      { MOCK_FAIL: "series-sync" },
      { MOCK_FAIL: "vol-index" },
      { MOCK_HISTORY: bad },
      { MOCK_CP_FAIL: "records/sync.json" },
      { MOCK_CP_FAIL: "/records/0000-" },
      ...["index.html", "dashboard.mjs", "dashboard.css", "sections.json"].map((f) => ({ MOCK_CP_FAIL: `site/${f}` })),
    ];
    for (const env of steps) {
      const r = run(out, env);
      assert.notEqual(r.status, 0, JSON.stringify(env));
      assert.deepEqual(published(out), before, JSON.stringify(env));
      assert.deepEqual(generationsIn(out), [before.target.split("/")[1]], JSON.stringify(env));
      assert.ok(!existsSync(join(out, ".lock")), JSON.stringify(env));
    }
  });
});

test("a section the schema rejects is an error, and nothing is published", () => {
  scratch((out) => {
    const bad = join(out, "bad-history.json");
    writeFileSync(bad, JSON.stringify({ ...history(), slot: null }));
    const r = run(out, { MOCK_HISTORY: bad });
    assert.equal(r.status, 1);
    assert.match(r.stderr, /the section fails its schema: \$\.source\.slot/);
    assert.ok(!existsSync(join(out, "current")));
    assert.deepEqual(generationsIn(out), []);
  });
});

test("a failure after the rename keeps the live generation", () => {
  scratch((out) => {
    const r = run(out, { MOCK_AFTER_PUBLISH: "1" });
    assert.equal(r.status, 1);
    assert.ok(published(out).files["varswap/section.json"]);
  });
});

test("a real directory at current fails the run and stays", () => {
  scratch((out) => {
    mkdirSync(join(out, "current"));
    writeFileSync(join(out, "current/keep"), "x");
    const r = run(out);
    assert.equal(r.status, 1);
    assert.match(r.stderr, /is not a symlink/);
    assert.equal(readFileSync(join(out, "current/keep"), "utf8"), "x");
    assert.deepEqual(generationsIn(out), []);
  });
});

// A generation with what publish needs, by hand, and a first publication of it.
function generation(out, name) {
  const g = join(out, "generations", name);
  const dir = join(g, "varswap/index/s");
  mkdirSync(join(dir, "records"), { recursive: true });
  for (const f of ["index.html", "dashboard.mjs", "dashboard.css", "sections.json"]) writeFileSync(join(g, f), name);
  writeFileSync(join(g, "varswap/section.json"), JSON.stringify({ links: [
    { label: "index", href: "index/s/index.json" },
    { label: "sync", href: "index/s/sync.json" },
    { label: "source", href: "https://github.com/joaquinbejar/varswap" },
  ] }));
  writeFileSync(join(dir, "index.json"), JSON.stringify({ records: ["records/0.json"] }));
  writeFileSync(join(dir, "sync.json"), "{}");
  writeFileSync(join(dir, "records/0.json"), "{}");
  return g;
}

test("each publish step that fails leaves current on the old generation, and the next run recovers", () => {
  const real = { readlink: readlinkSync, remove: rmSync, symlink: symlinkSync, rename: renameSync, list: readdirSync };
  for (const step of ["readlink", "remove", "symlink", "rename"]) {
    scratch((out) => {
      publish(out, generation(out, "gen.old"));
      const before = published(out);
      const broken = { ...real, [step]: () => { throw new Error(`mock ${step}`); } };
      assert.throws(() => publish(out, generation(out, "gen.new"), broken), new RegExp(`mock ${step}`));
      assert.deepEqual(published(out), before, step);
      publish(out, join(out, "generations/gen.new"));
      assert.equal(readlinkSync(join(out, "current")), "generations/gen.new", step);
      assert.ok(!existsSync(join(out, ".current.tmp")), step);
    });
  }
});

test("a generation with anything missing is not published", () => {
  const missing = [
    "index.html", "dashboard.mjs", "dashboard.css", "sections.json", "varswap/section.json",
    "varswap/index/s/index.json", "varswap/index/s/sync.json", "varswap/index/s/records/0.json",
  ];
  for (const path of missing) {
    scratch((out) => {
      publish(out, generation(out, "gen.old"));
      const g = generation(out, "gen.new");
      rmSync(join(g, path));
      assert.throws(() => publish(out, g), /the generation has no/, path);
      assert.equal(readlinkSync(join(out, "current")), "generations/gen.old", path);
    });
  }
  scratch((out) => {
    const g = generation(out, "gen.new");
    writeFileSync(join(g, "varswap/section.json"), '{"links": []}');
    assert.throws(() => checkGeneration(g), /links no index.json/);
  });
});

test("publishing keeps the live generation and the one before it, and removes the rest", () => {
  scratch((out) => {
    publish(out, generation(out, "gen.a"));
    publish(out, generation(out, "gen.b"));
    assert.deepEqual(generationsIn(out), ["gen.a", "gen.b"]);
    generation(out, "gen.crashed");
    publish(out, generation(out, "gen.c"));
    assert.deepEqual(generationsIn(out), ["gen.b", "gen.c"]);
  });
});

test("an old generation that cannot be removed is a warning: the new one is live", () => {
  scratch((out) => {
    publish(out, generation(out, "gen.a"));
    const g = generation(out, "gen.b");
    const broken = {
      readlink: readlinkSync, symlink: symlinkSync, rename: renameSync, list: readdirSync,
      remove: (path, options) => {
        if (options?.recursive) throw new Error("mock prune");
        rmSync(path, options);
      },
    };
    generation(out, "gen.stuck");
    const errors = [];
    const original = console.error;
    console.error = (text) => errors.push(text);
    try {
      publish(out, g, broken);
    } finally {
      console.error = original;
    }
    assert.equal(readlinkSync(join(out, "current")), "generations/gen.b");
    assert.match(errors.join(), /published, but could not remove an old generation: mock prune/);
  });
});

function start(out, env = {}) {
  const m = mock(env);
  const child = spawn("sh", [file("../dashboard.sh"), out], { env: m.env, stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  child.stderr.on("data", (d) => { stderr += d; });
  const ended = new Promise((resolve) => {
    child.on("close", (status, signal) => {
      m.done();
      resolve({ status, signal, stderr });
    });
  });
  return { child, ended };
}

async function until(condition) {
  for (let i = 0; i < 500; i += 1) {
    if (condition()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error("timed out");
}

test("two writers at once: one publishes, the other fails, and what is live is whole", async () => {
  const out = mkdtempSync(join(tmpdir(), "dashboard-out-"));
  try {
    const first = start(out, { MOCK_SLEEP: "1" });
    await until(() => existsSync(join(out, ".lock/pid")));
    const second = start(out, { MOCK_SLEEP: "1" });
    const [a, b] = await Promise.all([first.ended, second.ended]);
    assert.equal(a.status, 0, a.stderr);
    assert.equal(b.status, 1);
    assert.match(b.stderr, /another dashboard\.sh holds/);
    const live = published(out);
    const section = JSON.parse(live.files["varswap/section.json"]);
    const index = JSON.parse(live.files["varswap/index/sol-usd-hourly/index.json"]);
    assert.equal(index.generated_at, section.generated_at);
    assert.deepEqual(generationsIn(out), [live.target.split("/")[1]]);
    assert.ok(!existsSync(join(out, ".lock")));
  } finally {
    rmSync(out, { recursive: true });
  }
});

test("a lock is never taken over: a live, unknown or dead holder fails the run until it is removed by hand", async () => {
  const out = mkdtempSync(join(tmpdir(), "dashboard-out-"));
  try {
    assert.equal(run(out).status, 0);
    const before = published(out);
    mkdirSync(join(out, ".lock"));
    let r = run(out);
    assert.equal(r.status, 1, "a lock with no pid yet");
    assert.match(r.stderr, /held by pid unknown, which is not running: .* remove .* by hand/);
    writeFileSync(join(out, ".lock/pid"), `${process.pid}\n`);
    r = run(out);
    assert.equal(r.status, 1, "a live pid");
    assert.match(r.stderr, new RegExp(`another dashboard\\.sh holds .*\\(pid ${process.pid}, running\\)`));
    assert.deepEqual(published(out), before);
    rmSync(join(out, ".lock"), { recursive: true });

    const killed = start(out, { MOCK_SLEEP: "1" });
    await until(() => existsSync(join(out, ".lock/pid")) && generationsIn(out).length === 2);
    const pid = readFileSync(join(out, ".lock/pid"), "utf8").trim();
    killed.child.kill("SIGKILL");
    assert.equal((await killed.ended).signal, "SIGKILL");
    assert.ok(existsSync(join(out, ".lock")), "kill -9 leaves the lock");
    assert.equal(generationsIn(out).length, 2, "and the build");

    r = run(out);
    assert.equal(r.status, 1, "a dead pid is not taken over");
    assert.match(r.stderr, new RegExp(`held by pid ${pid}, which is not running`));
    assert.ok(existsSync(join(out, ".lock/pid")), "the dead run's lock stays");
    assert.deepEqual(published(out), before);
    assert.equal(generationsIn(out).length, 2, "nothing pruned");

    rmSync(join(out, ".lock"), { recursive: true });
    r = run(out);
    assert.equal(r.status, 0, r.stderr);
    assert.notEqual(published(out).target, before.target);
    assert.deepEqual(generationsIn(out).sort(), [before.target, published(out).target].map((t) => t.split("/")[1]).sort());
  } finally {
    rmSync(out, { recursive: true });
  }
});

test("two publishers that got past the lock: the pruning of one never removes what the other made live", () => {
  // The interleaving of the review: B reads `current` (gen.old), A makes gen.a live, B makes gen.b
  // live and prunes gen.a, then A prunes with gen.a and gen.old to keep. Before each removal is
  // checked against `current`, A removed gen.b and `current` pointed at nothing.
  const real = { readlink: readlinkSync, remove: rmSync, symlink: symlinkSync, rename: renameSync, list: readdirSync };
  scratch((out) => {
    publish(out, generation(out, "gen.old"));
    const a = generation(out, "gen.a");
    const b = generation(out, "gen.b");
    let stale = true;
    const fsB = { ...real, readlink: (path) => (stale ? ((stale = false), "generations/gen.old") : readlinkSync(path)) };
    let once = true;
    const fsA = {
      ...real,
      list: (path) => {
        if (once) {
          once = false;
          assert.equal(readlinkSync(join(out, "current")), "generations/gen.a");
          publish(out, b, fsB);
          assert.ok(!existsSync(a), "B pruned A's generation");
        }
        return readdirSync(path);
      },
    };
    publish(out, a, fsA);
    assert.equal(readlinkSync(join(out, "current")), "generations/gen.b");
    assert.ok(existsSync(join(out, "current/varswap/section.json")), "current resolves");
    assert.doesNotThrow(() => checkGeneration(join(out, "current")));
    assert.deepEqual(generationsIn(out), ["gen.b", "gen.old"]);
  });
});

test("SIGTERM removes the build and the lock and publishes nothing", async () => {
  const out = mkdtempSync(join(tmpdir(), "dashboard-out-"));
  try {
    const stopped = start(out, { MOCK_SLEEP: "1" });
    await until(() => existsSync(join(out, ".lock/pid")) && generationsIn(out).length === 1);
    stopped.child.kill("SIGTERM");
    const r = await stopped.ended;
    assert.equal(r.status, 143, r.stderr);
    assert.ok(!existsSync(join(out, "current")));
    assert.deepEqual(generationsIn(out), []);
    assert.ok(!existsSync(join(out, ".lock")));
  } finally {
    rmSync(out, { recursive: true });
  }
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
