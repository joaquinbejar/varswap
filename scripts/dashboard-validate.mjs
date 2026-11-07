// The subset of JSON Schema that docs/dashboard/section-1.schema.json uses, with no npm package.
// An unknown keyword is an error, so the subset cannot silently stop checking something.

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

function walk(v, s, defs, path) {
  const errors = [];
  for (const [k, want] of Object.entries(s)) {
    if (ANNOTATIONS.has(k)) continue;
    const add = (ok, what) => { if (!ok) errors.push(`${path}: ${what}`); };
    switch (k) {
      case "$ref":
        errors.push(...walk(v, defs[want.replace("#/$defs/", "")], defs, path));
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
            if (p in v) errors.push(...walk(v[p], ps, defs, `${path}.${p}`));
          }
        }
        break;
      case "additionalProperties":
        if (isType(v, "object")) {
          for (const [p, pv] of Object.entries(v)) {
            if (!(p in (s.properties ?? {}))) errors.push(...walk(pv, want, defs, `${path}.${p}`));
          }
        }
        break;
      case "items":
        if (Array.isArray(v)) v.forEach((e, i) => errors.push(...walk(e, want, defs, `${path}[${i}]`)));
        break;
      case "prefixItems":
        if (Array.isArray(v)) want.forEach((ps, i) => { if (i < v.length) errors.push(...walk(v[i], ps, defs, `${path}[${i}]`)); });
        break;
      case "oneOf": {
        const passing = want.filter((o) => walk(v, o, defs, path).length === 0).length;
        add(passing === 1, `${passing} alternatives match`);
        break;
      }
      default:
        errors.push(`schema keyword ${k} is not implemented here`);
    }
  }
  return errors;
}

/** The errors of `v` against `schema`, empty when it is valid. */
export const validate = (v, schema) => walk(v, schema, schema.$defs ?? {}, "$");
