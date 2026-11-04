// `make dashboard-check`: the shell's decisions, run with `node --test` and no npm package. The
// functions take their clock and URLs as arguments; rendering is checked by loading the page.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { cellTarget, chartGroups, formatAge, freshness, linkTarget, ticks } from "../../site/dashboard.mjs";

const SECTION = "https://joaquinbejar.github.io/varswap/varswap/section.json";
const site = new URL("../../site/", import.meta.url);
const fixtures = new URL("../../docs/dashboard/fixtures/", import.meta.url);
const fixture = (name) => JSON.parse(readFileSync(new URL(name, fixtures), "utf8"));
const NOW = Date.parse("2026-11-04T19:00:00Z");
const H = 3_600_000;

test("an absolute https link and a plain relative path become links", () => {
  assert.equal(linkTarget("https://github.com/joaquinbejar/varswap", SECTION), "https://github.com/joaquinbejar/varswap");
  assert.equal(linkTarget("index/sol-usd-hourly/", SECTION), "https://joaquinbejar.github.io/varswap/varswap/index/sol-usd-hourly/");
  assert.equal(linkTarget("a.b/c..d/e.json", SECTION), "https://joaquinbejar.github.io/varswap/varswap/a.b/c..d/e.json");
});

test("a relative link cannot leave its section's directory", () => {
  const dir = "https://joaquinbejar.github.io/varswap/varswap/";
  for (const href of ["a/b", "a.json", "x%20y"]) assert.ok(linkTarget(href, SECTION).startsWith(dir), href);
});

test("every dangerous href renders as text", () => {
  for (const href of [
    "javascript:alert(1)", "JaVaScRiPt:alert(1)", "data:text/html,x", "http://example.com/", "HTTPS://example.com/",
    "ftp://example.com/", "mailto:a@b.c", "//example.com/x", "/abs", "\\\\example.com\\x", "\\abs", "a\\b", ".", "..",
    "../up", "a/../up", "a/./b", "a/..", "%2e%2e/up", "%2E%2E/up", ".%2e/up", "a/%2e/b", "%2f%2fexample.com", "a%5cb",
    "with space", "tab\there", "", null, undefined, 7, {},
  ]) {
    assert.equal(linkTarget(href, SECTION), null, String(href));
  }
});

test("the fixture's dangerous links are the ones the check rejects", () => {
  const f = fixture("section-dangerous-links.json");
  const links = f.links.filter((l) => linkTarget(l.href, SECTION) !== null).map((l) => l.label);
  assert.deepEqual(links, ["https stays a link", "relative stays a link"]);
  const t = f.tables[0];
  assert.equal(cellTarget(t.href_columns.day, t.rows[0][0]), null);
  assert.equal(cellTarget(t.href_columns.address, t.rows[0][1]), null);
  assert.equal(cellTarget(t.href_columns.label, t.rows[0][2]), "https://example.com/a%20b%2Fc%3Fd%3D%3Ce%3E");
});

test("a table cell cannot escape its template", () => {
  const template = "https://explorer.solana.com/address/{}?cluster=devnet";
  assert.equal(cellTarget(template, "abc"), "https://explorer.solana.com/address/abc?cluster=devnet");
  assert.equal(
    cellTarget(template, "x/../../evil?a=1#f"),
    "https://explorer.solana.com/address/x%2F..%2F..%2Fevil%3Fa%3D1%23f?cluster=devnet",
  );
  // `$&` and friends would be replacement patterns in a plain string replace.
  assert.equal(cellTarget("https://e.com/{}", "$&$`"), "https://e.com/%24%26%24%60");
  for (const bad of ["http://e.com/{}", "javascript:{}", "//e.com/{}", "https://e.com/", "", null]) {
    assert.equal(cellTarget(bad, "abc"), null, String(bad));
  }
  assert.equal(cellTarget(template, null), null);
  assert.equal(cellTarget(template, ""), null);
});

test("freshness overrides an old section's ok, at the deadline and past it", () => {
  const at = (ms) => new Date(NOW - ms).toISOString().replace(".000Z", "Z");
  assert.deepEqual(freshness(at(48 * H), "ok", 48, NOW), { level: "ok", stale: false, ageMs: 48 * H });
  assert.equal(freshness(at(48 * H + 1000), "ok", 48, NOW).level, "stale");
  assert.equal(freshness(at(47 * H), "warn", 48, NOW).level, "warn");
  assert.equal(freshness(at(1 * H), "stale", 48, NOW).level, "stale");
  assert.equal(freshness(fixture("section-old-ok.json").generated_at, fixture("section-old-ok.json").status.level, 48, NOW).level, "stale");
  assert.equal(freshness(fixture("section-ok.json").generated_at, fixture("section-ok.json").status.level, 48, NOW).level, "ok");
});

test("a generated_at in the future is stale past the tolerance only", () => {
  const ahead = (ms) => new Date(NOW + ms).toISOString().replace(".000Z", "Z");
  assert.equal(freshness(ahead(1 * H), "ok", 48, NOW).level, "ok");
  assert.equal(freshness(ahead(1 * H + 1000), "ok", 48, NOW).level, "stale");
  assert.equal(freshness(fixture("section-future.json").generated_at, "ok", 48, NOW).level, "stale");
});

test("an unparsable generated_at or no deadline is an error, not fresh", () => {
  for (const bad of ["yesterday", "2026-11-04", "2026-11-04T18:05:00+01:00", "2026-13-40T18:05:00Z", "", null, undefined, 5]) {
    assert.ok(freshness(bad, "ok", 48, NOW).error, String(bad));
  }
  for (const bad of [undefined, null, 0, -1, "48"]) {
    assert.ok(freshness("2026-11-04T18:05:00Z", "ok", bad, NOW).error, String(bad));
  }
});

test("ages read as people expect", () => {
  assert.equal(formatAge(5 * 60_000), "5 min ago");
  assert.equal(formatAge(3 * H), "3.0 h ago");
  assert.equal(formatAge(72 * H), "3.0 days ago");
  assert.equal(formatAge(-2 * H), "2.0 h in the future");
});

test("one axis per unit, and never a ninth hue on a chart", () => {
  const s = (unit, i) => ({ unit, label: `${unit}${i}`, points: [] });
  const groups = chartGroups([s("a", 1), s("b", 1), s("a", 2), { label: "none", points: [] }]);
  assert.deepEqual(groups.map((g) => [g.unit, g.series.length]), [["a", 2], ["b", 1], ["", 1]]);
  const many = chartGroups(Array.from({ length: 9 }, (_, i) => s("a", i)));
  assert.deepEqual(many.map((g) => g.series.length), [8, 1]);
});

test("axis ticks cover the data with round steps", () => {
  const t = ticks(0.43, 0.74);
  assert.ok(t[0] <= 0.43 && t[t.length - 1] >= 0.74, String(t));
  assert.ok(t.length >= 3 && t.length <= 7, String(t));
  const flat = ticks(0.5, 0.5);
  assert.ok(flat[0] < 0.5 && flat[flat.length - 1] > 0.5, String(flat));
  const zero = ticks(0, 0);
  assert.ok(zero[0] === 0 && zero[zero.length - 1] > 0, String(zero));
});

test("the page loads only its own files, and nothing goes in as HTML", () => {
  const html = readFileSync(new URL("index.html", site), "utf8");
  const refs = [...html.matchAll(/(?:src|href)="([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(refs.sort(), ["dashboard.css", "dashboard.mjs"]);
  const js = readFileSync(new URL("dashboard.mjs", site), "utf8");
  for (const banned of ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write", "eval(", "new Function"]) {
    assert.ok(!js.includes(banned), banned);
  }
  const css = readFileSync(new URL("dashboard.css", site), "utf8");
  assert.ok(!/url\(|@import/.test(css), "the stylesheet loads something");
});

test("sections.json names a deadline for every section", () => {
  const sections = JSON.parse(readFileSync(new URL("sections.json", site), "utf8"));
  assert.ok(sections.length >= 3);
  for (const s of sections) {
    assert.ok(s.max_age_hours > 0, s.repo);
    assert.ok(s.repo && s.url, JSON.stringify(s));
  }
});
