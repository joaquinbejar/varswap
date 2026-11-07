// Brings the published records back before a run, and checks after it that none of them changed
// (docs/PLAN-phase2.md, P4). A GitHub hosted runner starts empty, so the scheduled workflow
// restores what the site already serves, and what the repo keeps in docs/windows/, into
// series-sync's directory: series-sync never rewrites a day that has a file, so those records
// pass through unchanged, and only the days after them are read from the chain.
//
//   node scripts/dashboard-restore.mjs restore <out-dir> [--site <url>] [--seed <dir>] [--allow-unpublished]
//   node scripts/dashboard-restore.mjs check <out-dir>
//
// `restore` writes each record into <out-dir>/records/, the state file series-sync left on the
// site into <out-dir>/records/sync.json, and a copy of each record into <out-dir>/published/,
// the baseline `check` compares against. It fails if <out-dir>/records/ already holds a file.
//
// - `--site` is the site's base URL. Its index.json lists the records; a 404 on it fails the run
//   unless `--allow-unpublished` is given, which is only for the first deployment. Any other
//   error fails the run: a run that restored nothing would read every day from the chain again,
//   and a day whose history the RPC no longer serves would come back `unknown`.
// - `--seed` is a directory of records kept in the repo (docs/windows/sol-usd-hourly/). A day in
//   both places must be the same record, except for `account.slot`; the site's copy is used.
//
// `check` fails unless every record under <out-dir>/published/ is in the generation that
// <out-dir>/current points at, and listed by its index.json, equal except for `account.slot`:
// the slot of the read is the one field that changes when a window is exported twice. The
// workflow deploys nothing when it fails.
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { isDeepStrictEqual, parseArgs } from "node:util";
import { pathToFileURL } from "node:url";

/** Where the series lives, in the site and in a generation. */
export const SERIES_DIR = "varswap/index/sol-usd-hourly";
/** A record's file name, as series-sync writes it: the day, then the window's base58 address. */
const RECORD_NAME = /^[0-9]{4}-[1-9A-HJ-NP-Za-km-z]{32,44}\.json$/;
const SYNC_FILE = "sync.json";
const ATTEMPTS = 3;
const TIMEOUT_MS = 30_000;

/** The record with the one field a second export changes taken out. */
function comparable(record) {
  if (record?.account === undefined || typeof record.account !== "object") return record;
  const { slot: _slot, ...account } = record.account;
  return { ...record, account };
}

/** Whether two records are the same but for `account.slot`. */
export function sameRecord(a, b) {
  return isDeepStrictEqual(comparable(a), comparable(b));
}

function parseRecord(text, where) {
  try {
    return JSON.parse(text);
  } catch (e) {
    throw new Error(`${where}: ${e.message}`);
  }
}

/** The file name of an index.json entry, `records/<name>`; anything else is refused. */
export function recordName(entry) {
  const m = typeof entry === "string" ? /^records\/([^/]+)$/.exec(entry) : null;
  if (m === null || !RECORD_NAME.test(m[1])) throw new Error(`index.json lists a record with a bad name: ${JSON.stringify(entry)}`);
  return m[1];
}

/** GET with a timeout and a few retries on a network error or a 5xx. 404 gives `null`. */
async function get(url, fetchImpl) {
  let last;
  for (let i = 0; i < ATTEMPTS; i += 1) {
    if (i > 0) await new Promise((r) => setTimeout(r, 1000 * 2 ** i));
    let res;
    try {
      res = await fetchImpl(url, { signal: AbortSignal.timeout(TIMEOUT_MS), redirect: "error" });
    } catch (e) {
      last = new Error(`${url}: ${e.message}`);
      continue;
    }
    if (res.status === 404) return null;
    if (res.status >= 500) {
      last = new Error(`${url}: HTTP ${res.status}`);
      continue;
    }
    if (res.status !== 200) throw new Error(`${url}: HTTP ${res.status}`);
    return res.text();
  }
  throw last;
}

/** Records under the site, as a map from file name to text; `null` when the site has none. */
async function fromSite(site, { fetchImpl }) {
  const base = `${site.replace(/\/+$/, "")}/${SERIES_DIR}`;
  const indexText = await get(`${base}/index.json`, fetchImpl);
  if (indexText === null) return null;
  const index = parseRecord(indexText, `${base}/index.json`);
  if (!Array.isArray(index.records)) throw new Error(`${base}/index.json has no records list`);
  const records = new Map();
  for (const entry of index.records) {
    const name = recordName(entry);
    const text = await get(`${base}/records/${name}`, fetchImpl);
    if (text === null) throw new Error(`${base}/records/${name}: listed by index.json, and not found`);
    parseRecord(text, `${base}/records/${name}`);
    records.set(name, text);
  }
  const sync = await get(`${base}/records/${SYNC_FILE}`, fetchImpl);
  if (sync === null) throw new Error(`${base}/records/${SYNC_FILE}: not found`);
  parseRecord(sync, `${base}/records/${SYNC_FILE}`);
  return { records, sync };
}

function fromSeed(dir) {
  const records = new Map();
  for (const name of readdirSync(dir).sort()) {
    if (name === "README.md") continue;
    if (!RECORD_NAME.test(name)) throw new Error(`${dir}/${name}: not a record's name`);
    const text = readFileSync(join(dir, name), "utf8");
    parseRecord(text, `${dir}/${name}`);
    records.set(name, text);
  }
  return records;
}

/** See the file's note. Returns how many records came from each place. */
export async function restore(out, { site, seed, allowUnpublished = false, fetchImpl = fetch }) {
  if (site === undefined && seed === undefined) throw new Error("restore needs --site, --seed or both");
  const recordsDir = join(out, "records");
  const publishedDir = join(out, "published");
  mkdirSync(recordsDir, { recursive: true });
  mkdirSync(publishedDir, { recursive: true });
  for (const dir of [recordsDir, publishedDir]) {
    if (readdirSync(dir).length > 0) throw new Error(`${dir} is not empty`);
  }
  const seeded = seed === undefined ? new Map() : fromSeed(seed);
  let published = null;
  if (site !== undefined) {
    published = await fromSite(site, { fetchImpl });
    if (published === null && !allowUnpublished) {
      throw new Error(`${site} serves no ${SERIES_DIR}/index.json; pass --allow-unpublished on the first deployment only`);
    }
  }
  const all = new Map(seeded);
  for (const [name, text] of published?.records ?? []) {
    const kept = seeded.get(name);
    if (kept !== undefined && !sameRecord(JSON.parse(kept), JSON.parse(text))) {
      throw new Error(`${name}: the site's record and the repo's differ`);
    }
    all.set(name, text);
  }
  for (const [name, text] of all) {
    writeFileSync(join(recordsDir, name), text);
    writeFileSync(join(publishedDir, name), text);
  }
  if (published !== null) writeFileSync(join(recordsDir, SYNC_FILE), published.sync);
  return { seed: seeded.size, site: published?.records.size ?? 0, records: all.size };
}

/** See the file's note. Returns how many records were checked. */
export function check(out) {
  const generation = join(out, "current", SERIES_DIR);
  const index = parseRecord(readFileSync(join(generation, "index.json"), "utf8"), `${generation}/index.json`);
  if (!Array.isArray(index.records)) throw new Error(`${generation}/index.json has no records list`);
  const listed = new Set(index.records.map(recordName));
  const publishedDir = join(out, "published");
  let checked = 0;
  for (const name of readdirSync(publishedDir).sort()) {
    const before = parseRecord(readFileSync(join(publishedDir, name), "utf8"), `${publishedDir}/${name}`);
    if (!listed.has(name)) throw new Error(`${name}: published before, and not in the new index.json`);
    let text;
    try {
      text = readFileSync(join(generation, "records", name), "utf8");
    } catch (e) {
      throw new Error(`${name}: published before, and not in the new generation (${e.code ?? e.message})`);
    }
    if (!sameRecord(before, parseRecord(text, `${generation}/records/${name}`))) {
      throw new Error(`${name}: a published record would change`);
    }
    checked += 1;
  }
  return checked;
}

async function main() {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: {
      site: { type: "string" },
      seed: { type: "string" },
      "allow-unpublished": { type: "boolean", default: false },
    },
  });
  const [cmd, out, ...rest] = positionals;
  if (out === undefined || rest.length > 0) throw new Error("usage: restore <out-dir> [--site <url>] [--seed <dir>] [--allow-unpublished] | check <out-dir>");
  if (cmd === "restore") {
    const n = await restore(out, { site: values.site, seed: values.seed, allowUnpublished: values["allow-unpublished"] });
    console.log(`dashboard-restore: ${n.records} records (${n.site} from the site, ${n.seed} from the repo)`);
  } else if (cmd === "check") {
    if (values.site !== undefined || values.seed !== undefined || values["allow-unpublished"]) throw new Error("check takes only <out-dir>");
    console.log(`dashboard-restore: ${check(out)} published records unchanged`);
  } else {
    throw new Error(`unknown command: ${cmd}`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((e) => {
    console.error(`dashboard-restore: ${e.message}`);
    process.exit(1);
  });
}
