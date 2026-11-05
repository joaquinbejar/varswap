// Turns `varswap-admin vol-index --history --json` into varswap's dashboard section and the
// published index (docs/PLAN-phase2.md, P3c). scripts/dashboard.sh runs it; the CLI output stays
// deterministic, so the generator's commit and the time are added here.
//
//   node scripts/dashboard-section.mjs --history <file> --name <text> --commit <sha>
//     --generated-at <time> --section <section.json> --index <index.json> --records <dir>
//
// It fails, and writes nothing, on an input with a field it needs missing or on a section the
// schema rejects, so a change in the CLI's output cannot publish a section with holes in it.
import { readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { basename } from "node:path";
import { pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

import { validate } from "./dashboard-validate.mjs";

const PERIODS = [1, 7, 30];
// `status` is stale past this; the same as the shell's max_age_hours for the section.
const MAX_FINAL_AGE_HOURS = 48;
// docs/dashboard.md asks a section to keep a table to about 100 rows; index.json has every day.
const TABLE_ROWS = 100;
const HOUR_MS = 3_600_000;
const SOURCE = "https://github.com/joaquinbejar/varswap";
const UNIT = "annualized";
// The reasons for a day with no final window. A `degraded` or `unknown` window did finish.
const NOT_FINAL = new Set(["open", "missing"]);

function need(object, key, where) {
  const v = object?.[key];
  if (v === undefined) throw new Error(`${where} has no ${key}`);
  return v;
}

function period(entry, days) {
  const v = need(entry, "values", `day ${entry.day}`).find((x) => x.days === days);
  if (v === undefined) throw new Error(`day ${entry.day} has no ${days}d value`);
  for (const key of ["usable", "label", "sigma", "excluded", "missed_samples", "late_samples", "late_unknown"]) {
    need(v, key, `day ${entry.day} ${days}d`);
  }
  return v;
}

function parseTime(text, where) {
  const ms = Date.parse(text);
  if (Number.isNaN(ms)) throw new Error(`${where} is not a time: ${text}`);
  return ms;
}

function status(entries, generatedAt) {
  const finals = entries.filter((e) => {
    const reason = period(e, 1).excluded[0]?.reason;
    return reason === undefined || !NOT_FINAL.has(reason);
  });
  const newest = finals.at(-1);
  if (newest === undefined) return { level: "stale", message: "no final day yet" };
  const ageMs = parseTime(generatedAt, "generated_at") - parseTime(newest.end, "end");
  if (ageMs > MAX_FINAL_AGE_HOURS * HOUR_MS) {
    const hours = Math.floor(ageMs / HOUR_MS);
    return { level: "stale", message: `the newest final day ended ${hours} h ago (${newest.end})` };
  }
  const excluded = period(entries.at(-1), 7).excluded;
  if (excluded.length > 0) {
    const days = excluded.map((x) => `day ${need(x, "day", "excluded")}: ${need(x, "reason", "excluded")}`);
    return { level: "warn", message: `the 7d value excludes ${days.join(", ")}` };
  }
  return { level: "ok", message: "" };
}

function tile(last, days, name) {
  const id = `sigma-${days}d`;
  const label = `${name} σ ${days}d`;
  if (last === undefined) return { id, label, value: null, unit: UNIT, quality: "insufficient", note: "no finished day yet" };
  const v = period(last, days);
  let note = `${v.usable}/${days} days to ${last.end.slice(0, 10)}`;
  if (v.missed_samples > 0) note += `; ${v.missed_samples} missed samples`;
  return { id, label, value: v.sigma, unit: UNIT, quality: v.label, note };
}

function series(entries, days) {
  return {
    id: `sigma-${days}d`,
    label: `σ ${days}d`,
    unit: UNIT,
    points: entries.map((e) => [need(e, "end", `day ${e.day}`), period(e, days).sigma]),
    quality: entries.map((e) => period(e, days).label),
  };
}

function windowsTable(entries, cluster) {
  const rows = entries.slice(-TABLE_ROWS).reverse().map((e) => {
    const v = period(e, 1);
    return [
      String(e.day),
      // The date alone keeps the table in the page's width; index.json has the exact time.
      need(e, "start", `day ${e.day}`).slice(0, 10),
      need(e, "window", `day ${e.day}`),
      v.label,
      v.excluded[0]?.reason ?? null,
      v.sigma,
      String(v.missed_samples),
      String(v.late_samples),
      String(v.late_unknown),
    ];
  });
  return {
    id: "windows",
    columns: ["day", "start", "address", "label", "reason", "σ 1d", "missed", "late", "no time"],
    rows,
    href_columns: { address: `https://explorer.solana.com/address/{}?cluster=${encodeURIComponent(cluster)}` },
  };
}

/** The section of one index series, from its `vol-index --history --json`. */
export function buildSection(history, { name, commit, generatedAt }) {
  const spec = need(history, "series", "history");
  const entries = need(history, "history", "history");
  const id = need(spec, "id", "series");
  const cluster = need(spec, "cluster", "series");
  const base = `index/${id}`;
  return {
    schema: "solana-projects-dashboard/section/1",
    repo: "varswap",
    title: "varswap: realized volatility",
    cluster,
    generated_at: generatedAt,
    source: { commit, slot: need(history, "slot", "history"), program_ids: [need(spec, "program_id", "series")] },
    status: status(entries, generatedAt),
    tiles: PERIODS.map((days) => tile(entries.at(-1), days, name)),
    series: PERIODS.map((days) => series(entries, days)),
    tables: [windowsTable(entries, cluster)],
    links: [
      { label: "index.json", href: `${base}/index.json` },
      { label: "sync.json", href: `${base}/sync.json` },
      { label: "source", href: SOURCE },
    ],
  };
}

/** The published index: the CLI's history, with what makes it non deterministic beside it. */
export function buildIndex(history, { commit, generatedAt, records }) {
  return { generator_commit: commit, generated_at: generatedAt, records, ...history };
}

function writeJson(path, value) {
  const tmp = `${path}.tmp`;
  writeFileSync(tmp, `${JSON.stringify(value, null, 2)}\n`);
  renameSync(tmp, path);
}

function main() {
  const { values: a } = parseArgs({
    options: Object.fromEntries(
      ["history", "name", "commit", "generated-at", "section", "index", "records"].map((k) => [k, { type: "string" }]),
    ),
  });
  for (const [k, v] of Object.entries(a)) if (v === "") throw new Error(`--${k} is empty`);
  const history = JSON.parse(readFileSync(a.history, "utf8"));
  const meta = { name: a.name, commit: a.commit, generatedAt: a["generated-at"] };
  const section = buildSection(history, meta);
  const schemaUrl = new URL("../docs/dashboard/section-1.schema.json", import.meta.url);
  const errors = validate(section, JSON.parse(readFileSync(schemaUrl, "utf8")));
  if (errors.length > 0) throw new Error(`the section fails its schema: ${errors.join("; ")}`);
  const records = readdirSync(a.records)
    .filter((f) => f.endsWith(".json") && f !== "sync.json")
    .sort()
    .map((f) => `${basename(a.records)}/${f}`);
  writeJson(a.index, buildIndex(history, { ...meta, records }));
  writeJson(a.section, section);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    main();
  } catch (e) {
    console.error(`dashboard-section: ${e.message}`);
    process.exit(1);
  }
}
