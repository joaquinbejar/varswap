// `make dashboard-check`: docs/dashboard/section-1.schema.json against its fixtures, with
// `node --test` and no npm package. `validate` is the subset of JSON Schema the file uses: an
// unknown keyword fails, so the subset cannot silently stop checking something.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const root = new URL("../../docs/dashboard/", import.meta.url);
const read = (name) => JSON.parse(readFileSync(new URL(name, root), "utf8"));
const schema = read("section-1.schema.json");

const isType = (v, t) =>
  t === "array"
    ? Array.isArray(v)
    : t === "null"
      ? v === null
      : t === "object"
        ? typeof v === "object" && v !== null && !Array.isArray(v)
        : t === "integer"
          ? Number.isInteger(v)
          : typeof v === t;

const ANNOTATIONS = new Set(["$schema", "$id", "title", "description", "$defs"]);

function validate(v, s, path = "$") {
  const errors = [];
  for (const [k, want] of Object.entries(s)) {
    if (ANNOTATIONS.has(k)) continue;
    const add = (ok, what) => { if (!ok) errors.push(`${path}: ${what}`); };
    switch (k) {
      case "$ref":
        errors.push(...validate(v, schema.$defs[want.replace("#/$defs/", "")], path));
        break;
      case "type":
        add([want].flat().some((t) => isType(v, t)), `not ${want}`);
        break;
      case "const":
        add(v === want, `not ${want}`);
        break;
      case "enum":
        add(want.includes(v), `not one of ${want}`);
        break;
      case "pattern":
        if (typeof v === "string") add(new RegExp(want, "u").test(v), `${JSON.stringify(v)} fails its pattern`);
        break;
      case "minLength":
        if (typeof v === "string") add(v.length >= want, "too short");
        break;
      case "minimum":
        if (typeof v === "number") add(v >= want, "too small");
        break;
      case "minItems":
        if (Array.isArray(v)) add(v.length >= want, "too few items");
        break;
      case "maxItems":
        if (Array.isArray(v)) add(v.length <= want, "too many items");
        break;
      case "required":
        if (isType(v, "object")) for (const r of want) add(r in v, `no ${r}`);
        break;
      case "properties":
        if (isType(v, "object")) {
          for (const [p, ps] of Object.entries(want)) {
            if (p in v) errors.push(...validate(v[p], ps, `${path}.${p}`));
          }
        }
        break;
      case "additionalProperties":
        if (isType(v, "object")) {
          for (const [p, pv] of Object.entries(v)) {
            if (!(p in (s.properties ?? {}))) errors.push(...validate(pv, want, `${path}.${p}`));
          }
        }
        break;
      case "items":
        if (Array.isArray(v)) v.forEach((e, i) => errors.push(...validate(e, want, `${path}[${i}]`)));
        break;
      case "prefixItems":
        if (Array.isArray(v)) want.forEach((ps, i) => { if (i < v.length) errors.push(...validate(v[i], ps, `${path}[${i}]`)); });
        break;
      case "oneOf": {
        const passing = want.filter((o) => validate(v, o, path).length === 0).length;
        add(passing === 1, `${passing} alternatives match`);
        break;
      }
      default:
        errors.push(`schema keyword ${k} is not implemented here`);
    }
  }
  return errors;
}

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

test("an unknown schema keyword is an error, not a pass", () => {
  assert.notDeepEqual(validate(1, { "not-a-keyword": true }), []);
});
