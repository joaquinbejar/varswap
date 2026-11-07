// `make dashboard-check`: scripts/dashboard-restore.mjs, which the Pages workflow runs around
// scripts/dashboard.sh. The site is a fake `fetch` over the records of the first two devnet days
// (docs/dashboard/fixtures/vol-index/records/), so no network is touched.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { check, recordName, restore, sameRecord, SERIES_DIR } from "../dashboard-restore.mjs";

const file = (path) => fileURLToPath(new URL(path, import.meta.url));
const script = file("../dashboard-restore.mjs");
const fixtures = file("../../docs/dashboard/fixtures/vol-index/records");
const DAY_0 = "0000-7ywXkV5Sa3fGHKhTAcdXqR5jkNg5J7pqvGQg1Hh6XjG1.json";
const DAY_1 = "0001-CjtkCZY1BEzN8MFgrbViGtW9KEUqP6vMKu7CCPRqiATG.json";
const SITE = "https://example.test/varswap";
const text = (name) => readFileSync(join(fixtures, name), "utf8");

function tmp(t) {
  const dir = mkdtempSync(join(tmpdir(), "varswap-pages-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

/** A site serving `files` (path under the series directory to text), with every request logged. */
function site(files, { status = {} } = {}) {
  const requests = [];
  const fetchImpl = async (url, options) => {
    requests.push(url);
    assert.equal(options.redirect, "error");
    const path = url.slice(`${SITE}/${SERIES_DIR}/`.length);
    const code = status[path] ?? (files[path] === undefined ? 404 : 200);
    return { status: code, text: async () => files[path] };
  };
  return { fetchImpl, requests };
}

function published(names = [DAY_0, DAY_1]) {
  const files = { "index.json": JSON.stringify({ records: names.map((n) => `records/${n}`) }), "records/sync.json": text("sync.json") };
  for (const n of names) files[`records/${n}`] = text(n);
  return files;
}

/** A generation as scripts/dashboard.sh leaves it, holding `records` (name to text). */
function generation(out, records) {
  const dir = join(out, "generations", "gen.test", SERIES_DIR);
  mkdirSync(join(dir, "records"), { recursive: true });
  for (const [name, body] of Object.entries(records)) writeFileSync(join(dir, "records", name), body);
  writeFileSync(join(dir, "index.json"), JSON.stringify({ records: Object.keys(records).map((n) => `records/${n}`) }));
  symlinkSync("generations/gen.test", join(out, "current"));
  return dir;
}

const withSlot = (body, slot) => {
  const v = JSON.parse(body);
  v.account.slot = slot;
  return JSON.stringify(v, null, 2);
};

test("restore puts the site's records and its sync.json where series-sync resumes, and keeps a baseline", async (t) => {
  const out = tmp(t);
  const { fetchImpl, requests } = site(published());
  assert.deepEqual(await restore(out, { site: `${SITE}/`, fetchImpl }), { seed: 0, site: 2, records: 2 });
  assert.deepEqual(readdirSync(join(out, "records")).sort(), [DAY_0, DAY_1, "sync.json"]);
  assert.deepEqual(readdirSync(join(out, "published")).sort(), [DAY_0, DAY_1]);
  assert.equal(readFileSync(join(out, "records", DAY_1), "utf8"), text(DAY_1));
  assert.equal(readFileSync(join(out, "records", "sync.json"), "utf8"), text("sync.json"));
  assert.equal(requests[0], `${SITE}/${SERIES_DIR}/index.json`);
});

test("a site with no index fails the run, unless it is the first deployment", async (t) => {
  await assert.rejects(restore(tmp(t), { site: SITE, fetchImpl: site({}).fetchImpl }), /--allow-unpublished/);
  const out = tmp(t);
  assert.deepEqual(await restore(out, { site: SITE, allowUnpublished: true, fetchImpl: site({}).fetchImpl }), { seed: 0, site: 0, records: 0 });
  assert.deepEqual(readdirSync(join(out, "records")), []);
});

test("any other failure of the site fails the run: a listed record missing, a 403, a 5xx that stays, no sync.json", async (t) => {
  const files = published();
  delete files[`records/${DAY_1}`];
  await assert.rejects(restore(tmp(t), { site: SITE, fetchImpl: site(files).fetchImpl }), /listed by index.json, and not found/);
  await assert.rejects(restore(tmp(t), { site: SITE, fetchImpl: site(published(), { status: { "index.json": 403 } }).fetchImpl }), /HTTP 403/);
  const noSync = published();
  delete noSync["records/sync.json"];
  await assert.rejects(restore(tmp(t), { site: SITE, fetchImpl: site(noSync).fetchImpl }), /sync.json: not found/);
  const s = site(published(), { status: { [`records/${DAY_0}`]: 503 } });
  t.mock.method(globalThis, "setTimeout", (fn) => fn());
  await assert.rejects(restore(tmp(t), { site: SITE, fetchImpl: s.fetchImpl }), /HTTP 503/);
  assert.equal(s.requests.filter((u) => u.endsWith(DAY_0)).length, 3);
});

test("an index entry that is not a record's name is refused before anything is fetched for it", async (t) => {
  for (const bad of ["../../x.json", "records/../x.json", "records/a/b.json", "records/x.json", "/etc/passwd", 7]) {
    assert.throws(() => recordName(bad), /bad name/);
  }
  const files = published();
  files["index.json"] = JSON.stringify({ records: [`records/${DAY_0}`, "records/../../sync.json"] });
  const s = site(files);
  await assert.rejects(restore(tmp(t), { site: SITE, fetchImpl: s.fetchImpl }), /bad name/);
  assert.ok(s.requests.every((u) => !u.includes("..")));
});

test("the repo's records seed the run, and a day in both places must agree except for its slot", async (t) => {
  const seed = tmp(t);
  writeFileSync(join(seed, DAY_0), withSlot(text(DAY_0), 1));
  writeFileSync(join(seed, "README.md"), "kept in the repo\n");
  const out = tmp(t);
  const n = await restore(out, { site: SITE, seed, fetchImpl: site(published([DAY_0, DAY_1])).fetchImpl });
  assert.deepEqual(n, { seed: 1, site: 2, records: 2 });
  assert.equal(readFileSync(join(out, "records", DAY_0), "utf8"), text(DAY_0), "the site's copy is used");

  const seedOnly = tmp(t);
  assert.deepEqual(await restore(seedOnly, { seed, site: SITE, allowUnpublished: true, fetchImpl: site({}).fetchImpl }), { seed: 1, site: 0, records: 1 });
  assert.deepEqual(readdirSync(join(seedOnly, "records")), [DAY_0]);

  const changed = JSON.parse(text(DAY_0));
  changed.derived.label = "compressed";
  writeFileSync(join(seed, DAY_0), JSON.stringify(changed));
  await assert.rejects(restore(tmp(t), { site: SITE, seed, fetchImpl: site(published()).fetchImpl }), /the site's record and the repo's differ/);

  writeFileSync(join(seed, "notes.txt"), "");
  await assert.rejects(restore(tmp(t), { seed }), /not a record's name/);
});

test("restore refuses a directory that already holds records", async (t) => {
  const out = tmp(t);
  mkdirSync(join(out, "records"));
  writeFileSync(join(out, "records", DAY_0), text(DAY_0));
  await assert.rejects(restore(out, { site: SITE, fetchImpl: site(published()).fetchImpl }), /is not empty/);
});

test("check passes when every published record is in the new generation, a new slot aside", async (t) => {
  const out = tmp(t);
  await restore(out, { site: SITE, fetchImpl: site(published()).fetchImpl });
  generation(out, { [DAY_0]: text(DAY_0), [DAY_1]: withSlot(text(DAY_1), 999) });
  assert.equal(check(out), 2);
});

test("check fails when a published record changed, is gone, or is no longer listed", async (t) => {
  const changedField = tmp(t);
  await restore(changedField, { site: SITE, fetchImpl: site(published()).fetchImpl });
  const v = JSON.parse(text(DAY_1));
  v.derived.realized_var = "0.1";
  generation(changedField, { [DAY_0]: text(DAY_0), [DAY_1]: JSON.stringify(v) });
  assert.throws(() => check(changedField), /a published record would change/);

  const gone = tmp(t);
  await restore(gone, { site: SITE, fetchImpl: site(published()).fetchImpl });
  generation(gone, { [DAY_0]: text(DAY_0) });
  assert.throws(() => check(gone), new RegExp(`${DAY_1}: published before, and not in the new index.json`));

  const unlisted = tmp(t);
  await restore(unlisted, { site: SITE, fetchImpl: site(published()).fetchImpl });
  const dir = generation(unlisted, { [DAY_0]: text(DAY_0), [DAY_1]: text(DAY_1) });
  writeFileSync(join(dir, "index.json"), JSON.stringify({ records: [`records/${DAY_0}`] }));
  assert.throws(() => check(unlisted), /not in the new index.json/);
});

test("sameRecord ignores account.slot and nothing else", () => {
  const a = JSON.parse(text(DAY_0));
  assert.ok(sameRecord(a, JSON.parse(withSlot(text(DAY_0), 1))));
  const b = structuredClone(a);
  b.account.owner = "x";
  assert.ok(!sameRecord(a, b));
  const c = structuredClone(a);
  c.slot = 1;
  assert.ok(!sameRecord(a, c));
});

test("the CLI exits 1 with its error, and check takes no site", (t) => {
  const out = tmp(t);
  cpSync(fixtures, join(out, "published"), { recursive: true });
  rmSync(join(out, "published", "sync.json"));
  generation(out, { [DAY_0]: text(DAY_0) });
  const r = spawnSync(process.execPath, [script, "check", out], { encoding: "utf8" });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /^dashboard-restore: .*published before, and not in the new index.json/);
  const s = spawnSync(process.execPath, [script, "check", out, "--site", SITE], { encoding: "utf8" });
  assert.equal(s.status, 1);
  assert.match(s.stderr, /check takes only <out-dir>/);
  const u = spawnSync(process.execPath, [script, "restore", out], { encoding: "utf8" });
  assert.equal(u.status, 1);
  assert.match(u.stderr, /restore needs --site, --seed or both/);
});
