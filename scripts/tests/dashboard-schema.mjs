// `make dashboard-check`: docs/dashboard/section-1.schema.json against its fixtures, with
// `node --test` and no npm package.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { validate } from "../dashboard-validate.mjs";

const root = new URL("../../docs/dashboard/", import.meta.url);
const read = (name) => JSON.parse(readFileSync(new URL(name, root), "utf8"));
const schema = read("section-1.schema.json");

const section = () => read("fixtures/section-ok.json");

test("the fixture with every field is valid", () => {
  assert.deepEqual(validate(section(), schema), []);
});

test("every required field is required", () => {
  for (const field of schema.required) {
    const s = section();
    delete s[field];
    assert.ok(validate(s, schema).some((e) => e.includes(`no ${field}`)), field);
  }
});

test("wrong values fail", () => {
  const cases = [
    (s) => { s.schema = "solana-projects-dashboard/section/2"; },
    (s) => { s.status.level = "fine"; },
    (s) => { s.generated_at = "yesterday"; },
    (s) => { s.generated_at = "2026-11-04T18:05:00+01:00"; },
    (s) => { s.series[0].points[0][1] = 0.5; },
    (s) => { s.series[0].points[0][1] = "1e5"; },
    (s) => { s.series[0].points[0] = ["2026-11-03T17:00:00Z"]; },
    (s) => { s.tables[0].rows[0][0] = 7; },
    (s) => { s.tables[0].href_columns.address = "http://explorer.solana.com/{}"; },
    (s) => { s.tables[0].href_columns.address = "https://explorer.solana.com/address"; },
  ];
  for (const change of cases) {
    const s = section();
    change(s);
    assert.notDeepEqual(validate(s, schema), [], change.toString());
  }
});

const accepted = [
  "https://github.com/joaquinbejar/varswap",
  "index/sol-usd-hourly/",
  "records.json",
  "a.b/c..d/e.json",
  "a..?x",
  "a?x=1#y",
];
const rejected = [
  "javascript:alert(1)",
  "JaVaScRiPt:alert(1)",
  "data:text/html,x",
  "http://example.com/",
  "//example.com/x",
  "/abs",
  "\\\\example.com\\x",
  "\\abs",
  "a\\b",
  "..",
  ".",
  "../up",
  "a/../up",
  "a/./b",
  "a/..",
  "%2e%2e/up",
  "%2E%2E/up",
  ".%2e/up",
  "a/%2e/b",
  "%2f%2fexample.com",
  "a%5cb",
  "with space",
  "",
  "\u0000javascript:alert(1)",
  "\u0001javascript:alert(1)",
  "\u001fjavascript:alert(1)",
  "java\u0001script:alert(1)",
  "\u007fjavascript:alert(1)",
  "https://example.com/\u0000",
  "a\u0001b",
  "..?x",
  "..#x",
  ".?x",
  "%2e%2e?x",
  "%2E.#x",
  "a/..?x",
  "a/.#x",
];

test("links[].href follows the allowlist", () => {
  for (const href of accepted) {
    const s = section();
    s.links[0].href = href;
    assert.deepEqual(validate(s, schema), [], href);
  }
  for (const href of rejected) {
    const s = section();
    s.links[0].href = href;
    assert.notDeepEqual(validate(s, schema), [], href);
  }
});

test("the shell's fixtures: valid except the dangerous one", () => {
  for (const name of ["section-ok.json", "section-old-ok.json", "section-future.json"]) {
    assert.deepEqual(validate(read(`fixtures/${name}`), schema), [], name);
  }
  const errors = validate(read("fixtures/section-dangerous-links.json"), schema);
  assert.ok(errors.some((e) => e.includes("$.links[")), errors.join("; "));
  assert.ok(errors.some((e) => e.includes("href_columns")), errors.join("; "));
});

test("an unknown schema keyword is an error, not a pass", () => {
  assert.notDeepEqual(validate(1, { "not-a-keyword": true }), []);
});
