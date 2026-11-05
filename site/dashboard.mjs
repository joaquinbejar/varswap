// The shared dashboard's shell (docs/dashboard.md). Every section is another repo's data: all of
// it goes in as text nodes, and links pass the checks below whatever the section says.

const SCHEMA = "solana-projects-dashboard/section/1";
const HOUR_MS = 3_600_000;
const FUTURE_TOLERANCE_MS = HOUR_MS;
const TIME = /^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?Z$/;
const DECIMAL = /^-?[0-9]+(\.[0-9]+)?$/;
const SCHEME = /^[A-Za-z][A-Za-z0-9+.-]*:/;
// A URL parser strips some of these before it looks at the scheme.
const CONTROL = /[\u0000-\u001f\u007f]/;
const DOT_SEGMENT = /(?:^|\/)(?:\.|%2e){1,2}(?:[/?#]|$)/i;
/** Values closer than this, relative to their size, are drawn as one flat value. */
const CLOSEST_RANGE = 1e-12;
/** `toFixed` takes no more digits than this. */
const MAX_DECIMALS = 100;
/** Lines sharing one chart: past this they go to another chart, never to a generated hue. */
const MAX_SERIES_PER_CHART = 8;
/** Direct labels at the line ends up to this many series; the legend is always there. */
const MAX_DIRECT_LABELS = 4;

/**
 * Whether a resolved URL is in the directory of the section's URL, on its origin. The string
 * checks in `linkTarget` already imply it; this is the check on what the browser will open.
 */
export function staysWithSection(target, base) {
  const directory = base.pathname.slice(0, base.pathname.lastIndexOf("/") + 1);
  return ["https:", "http:"].includes(base.protocol) && target.origin === base.origin && target.pathname.startsWith(directory);
}

/**
 * Where a `links[].href` may go, or null to render it as text: absolute https, or a relative path
 * with no scheme, leading slash, backslash, encoded slash or dot segment, which must resolve to
 * the section's own directory, on the protocol the section itself was served with. No form may
 * hold a control character. Mirrors `$defs.href` in docs/dashboard/section-1.schema.json.
 */
export function linkTarget(href, sectionUrl) {
  if (typeof href !== "string" || href === "" || /\s/.test(href) || CONTROL.test(href) || href.includes("\\")) {
    return null;
  }
  if (SCHEME.test(href)) {
    if (!href.startsWith("https://")) return null;
    try {
      return new URL(href).protocol === "https:" ? href : null;
    } catch {
      return null;
    }
  }
  if (href.startsWith("/") || /%(?:2f|5c)/i.test(href) || DOT_SEGMENT.test(href)) return null;
  try {
    const base = new URL(sectionUrl);
    const target = new URL(href, base);
    return staysWithSection(target, base) ? target.href : null;
  } catch {
    return null;
  }
}

/** A table cell's link from its `href_columns` template, or null to render it as text. */
export function cellTarget(template, cell) {
  if (typeof template !== "string" || typeof cell !== "string" || cell === "") return null;
  if (!template.startsWith("https://") || !template.includes("{}")) return null;
  const href = template.replace("{}", () => encodeURIComponent(cell));
  try {
    return new URL(href).protocol === "https:" ? href : null;
  } catch {
    return null;
  }
}

/**
 * A time as its millisecond count, NaN if it is not one. Engines disagree on `2026-02-31`, and some
 * roll it into March: a date must come back from the parser as it went in.
 */
function parseTime(text) {
  if (typeof text !== "string" || !TIME.test(text)) return NaN;
  const ms = Date.parse(text);
  if (Number.isNaN(ms)) return NaN;
  return new Date(ms).toISOString().slice(0, 19) === text.slice(0, 19) ? ms : NaN;
}

/**
 * What the shell shows for a section's age. `status.level` is written by a generator, so a stopped
 * workflow keeps saying `ok` forever: age is computed here, with the browser's clock.
 * Returns { level, stale, ageMs }, or { error } if `generated_at` does not parse.
 */
export function freshness(generatedAt, level, maxAgeHours, nowMs) {
  const at = parseTime(generatedAt);
  if (Number.isNaN(at)) return { error: "generated_at does not parse" };
  if (typeof maxAgeHours !== "number" || !(maxAgeHours > 0)) return { error: "no max_age_hours" };
  const ageMs = nowMs - at;
  const stale = ageMs > maxAgeHours * HOUR_MS || ageMs < -FUTURE_TOLERANCE_MS;
  return { level: stale ? "stale" : level, stale, ageMs };
}

/** "3 h", "2.5 days": an age for people. */
export function formatAge(ms) {
  const abs = Math.abs(ms);
  const text =
    abs < HOUR_MS
      ? `${Math.round(abs / 60_000)} min`
      : abs < 48 * HOUR_MS
        ? `${(abs / HOUR_MS).toFixed(1)} h`
        : `${(abs / (24 * HOUR_MS)).toFixed(1)} days`;
  return ms < 0 ? `${text} in the future` : `${text} ago`;
}

/** Series grouped for one axis each: same unit together, at most MAX_SERIES_PER_CHART per chart. */
export function chartGroups(series) {
  const byUnit = new Map();
  for (const s of series) {
    const unit = s.unit ?? "";
    if (!byUnit.has(unit)) byUnit.set(unit, []);
    byUnit.get(unit).push(s);
  }
  const groups = [];
  for (const [unit, list] of byUnit) {
    for (let i = 0; i < list.length; i += MAX_SERIES_PER_CHART) {
      groups.push({ unit, series: list.slice(i, i + MAX_SERIES_PER_CHART) });
    }
  }
  return groups;
}

/**
 * Round axis ticks covering [min, max], rising. Values too far apart or too small to draw are an
 * error, which the caller turns into the section's error card, never a blank or a hung page.
 */
export function ticks(min, max, count = 4) {
  if (!Number.isFinite(min) || !Number.isFinite(max) || max < min) throw new Error("the values cannot be drawn");
  let [low, high] = [min, max];
  if (!(high - low > Math.max(Math.abs(low), Math.abs(high)) * CLOSEST_RANGE)) {
    // One flat value. A value at or above zero keeps a floor of zero: a count is never negative.
    const pad = Math.abs(low) * 0.1 || 1;
    low = low >= 0 ? Math.max(0, low - pad) : low - pad;
    high += pad;
  }
  const rough = (high - low) / count;
  const base = 10 ** Math.floor(Math.log10(rough));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * base).find((s) => s >= rough);
  if (!Number.isFinite(rough) || !(rough > 0) || !Number.isFinite(step) || !(step > 0)) {
    throw new Error("the values cannot be drawn");
  }
  // Enough digits for this step and not a fixed number, so small values do not round to zero.
  const decimals = Math.min(MAX_DECIMALS, Math.max(0, Math.ceil(-Math.log10(step)) + 1));
  const out = [];
  // At most `count + 3` turns, as step >= rough; the flat case above keeps `k` far below 2^53.
  for (let k = Math.floor(low / step); k * step < high + step; k += 1) {
    out.push(Number((k * step).toFixed(decimals)));
  }
  if (!(out[out.length - 1] > out[0])) throw new Error("the values cannot be drawn");
  return out;
}

/**
 * One series' points for a chart. A malformed time or value throws: `Number()` alone would give
 * NaN or Infinity, and with them a broken axis. A value keeps its text for the tooltip.
 */
export function chartPoints(series) {
  if (!Array.isArray(series.points)) throw new Error(`${series.id}: points is not a list`);
  return series.points.map((point) => {
    if (!Array.isArray(point) || point.length !== 2) throw new Error(`${series.id}: a malformed point`);
    const [at, raw] = point;
    const t = parseTime(at);
    if (Number.isNaN(t)) throw new Error(`${series.id}: a point's time does not parse`);
    if (raw === null) return { t, v: null, raw, at };
    const v = typeof raw === "string" && DECIMAL.test(raw) ? Number(raw) : NaN;
    if (!Number.isFinite(v)) throw new Error(`${series.id}: a point's value is not a finite decimal`);
    return { t, v, raw, at };
  });
}

/** The points and axis of one chart, checked before anything is drawn. */
export function chartData(group) {
  const parsed = group.series.map(chartPoints);
  const values = parsed.flat().filter((p) => p.v !== null).map((p) => p.v);
  const axis = values.length ? ticks(values.reduce((a, b) => Math.min(a, b)), values.reduce((a, b) => Math.max(a, b))) : [0, 1];
  return { parsed, axis };
}

// --- the page ---

const SVG_NS = "http://www.w3.org/2000/svg";

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined && text !== null) node.textContent = String(text);
  return node;
}

function svg(tag, attrs = {}, text) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  if (text !== undefined) node.textContent = String(text);
  return node;
}

const STATUS_MARK = { ok: "✓", warn: "!", stale: "!", error: "×", pending: "…" };

function statusPill(level, label) {
  const pill = el("span", "status");
  const mark = el("span", "status-mark", STATUS_MARK[level] ?? "?");
  mark.dataset.level = level;
  mark.setAttribute("aria-hidden", "true");
  pill.append(mark, document.createTextNode(label));
  return pill;
}

function linkOrText(text, target) {
  if (target === null) return document.createTextNode(text ?? "");
  const a = el("a", "", text);
  a.href = target;
  a.rel = "noopener noreferrer";
  return a;
}

function renderTiles(tiles) {
  const box = el("div", "tiles");
  for (const t of tiles) {
    const tile = el("div", "tile");
    tile.append(el("div", "tile-label", t.label));
    const value = el("div", "tile-value", t.value ?? "no value");
    if (t.value === null || t.value === undefined) value.dataset.empty = "";
    tile.append(value);
    const foot = el("div", "tile-foot");
    if (t.quality) foot.append(el("span", "badge", t.quality));
    if (t.unit) foot.append(el("span", "", t.unit));
    if (t.note) foot.append(el("span", "", t.note));
    if (foot.childNodes.length) tile.append(foot);
    box.append(tile);
  }
  return box;
}

function renderTable(columns, rows, hrefColumns) {
  const scroll = el("div", "table-scroll");
  const table = el("table");
  const head = el("tr");
  for (const c of columns) head.append(el("th", "", c));
  const thead = el("thead");
  thead.append(head);
  const tbody = el("tbody");
  for (const row of rows) {
    const tr = el("tr");
    row.forEach((cell, i) => {
      const td = el("td");
      const template = hrefColumns?.[columns[i]];
      td.append(linkOrText(cell ?? "", template ? cellTarget(template, cell) : null));
      tr.append(td);
    });
    tbody.append(tr);
  }
  table.append(thead, tbody);
  scroll.append(table);
  return scroll;
}

function paint(box, group) {
  const stale = box.querySelector("svg");
  if (stale) stale.remove();
  const width = Math.max(box.clientWidth, 280);
  const height = 220;
  const labelled = group.series.length >= 2 && group.series.length <= MAX_DIRECT_LABELS && width >= 480;
  const m = { top: 10, right: labelled ? 96 : 14, bottom: 24, left: 52 };
  const { parsed, axis } = group.data;
  const times = [...new Set(parsed.flat().map((p) => p.t))].sort((a, b) => a - b);
  if (times.length === 0) return;
  const [y0, y1] = [axis[0], axis[axis.length - 1]];
  const [t0, t1] = [times[0], times[times.length - 1]];
  const xOf = (t) => (t1 === t0 ? m.left + (width - m.left - m.right) / 2 : m.left + ((t - t0) / (t1 - t0)) * (width - m.left - m.right));
  const yOf = (v) => height - m.bottom - ((v - y0) / (y1 - y0)) * (height - m.top - m.bottom);
  const root = svg("svg", { viewBox: `0 0 ${width} ${height}`, height, tabindex: 0, role: "img", "aria-label": group.series.map((s) => s.label).join(", ") });

  for (const v of axis) {
    root.append(svg("line", { class: "grid-line", x1: m.left, x2: width - m.right, y1: yOf(v), y2: yOf(v) }));
    root.append(svg("text", { class: "axis-text", x: m.left - 8, y: yOf(v) + 4, "text-anchor": "end" }, v));
  }
  const xLabels = times.length === 1 ? [t0] : [t0, t1];
  xLabels.forEach((t, i) => {
    const anchor = times.length === 1 ? "middle" : i === 0 ? "start" : "end";
    root.append(svg("text", { class: "axis-text", x: xOf(t), y: height - 6, "text-anchor": anchor }, new Date(t).toISOString().slice(0, 10)));
  });

  const ends = [];
  parsed.forEach((points, i) => {
    const color = `var(--series-${i + 1})`;
    let d = "";
    let run = 0;
    points.forEach((p, k) => {
      if (p.v === null) {
        run = 0;
        return;
      }
      d += `${run === 0 ? "M" : "L"}${xOf(p.t).toFixed(1)} ${yOf(p.v).toFixed(1)}`;
      run += 1;
      // A point with no neighbour draws no line: it needs a marker to exist at all.
      const lonely = (points[k - 1]?.v ?? null) === null && (points[k + 1]?.v ?? null) === null;
      if (lonely) root.append(svg("circle", { cx: xOf(p.t), cy: yOf(p.v), r: 4, fill: color }));
    });
    if (d) root.append(svg("path", { d, fill: "none", stroke: color, "stroke-width": 2, "stroke-linejoin": "round", "stroke-linecap": "round" }));
    const last = [...points].reverse().find((p) => p.v !== null);
    if (labelled && last) ends.push({ y: yOf(last.v), x: xOf(last.t), text: group.series[i].label });
  });
  ends.sort((a, b) => a.y - b.y);
  ends.forEach((e, i) => {
    if (i > 0 && e.y - ends[i - 1].y < 14) e.y = ends[i - 1].y + 14;
    root.append(svg("text", { class: "end-label", x: e.x + 8, y: e.y + 4 }, e.text.length > 12 ? `${e.text.slice(0, 11)}…` : e.text));
  });

  // Crosshair and tooltip: snap to the nearest time, every series listed, keyboard too.
  const hair = svg("line", { class: "crosshair", y1: m.top, y2: height - m.bottom, visibility: "hidden" });
  root.append(hair);
  const tip = box.querySelector(".tooltip");
  const show = (index) => {
    const t = times[index];
    hair.setAttribute("x1", xOf(t));
    hair.setAttribute("x2", xOf(t));
    hair.setAttribute("visibility", "visible");
    tip.replaceChildren(el("div", "tooltip-time", new Date(t).toISOString().slice(0, 16).replace("T", " ") + "Z"));
    group.series.forEach((s, i) => {
      const point = parsed[i].find((p) => p.t === t);
      const quality = s.quality?.[parsed[i].indexOf(point)];
      const row = el("div", "tooltip-row");
      const key = el("span", "key");
      key.style.background = `var(--series-${i + 1})`;
      row.append(key, el("span", "tooltip-value", point?.raw ?? "no value"), el("span", "tooltip-name", quality ? `${s.label} (${quality})` : s.label));
      tip.append(row);
    });
    tip.hidden = false;
    const left = xOf(t) + 12;
    tip.style.left = `${left + tip.offsetWidth > width ? Math.max(0, xOf(t) - tip.offsetWidth - 12) : left}px`;
    tip.style.top = `${root.offsetTop + m.top}px`;
  };
  const hide = () => {
    hair.setAttribute("visibility", "hidden");
    tip.hidden = true;
  };
  let current = times.length - 1;
  root.addEventListener("pointermove", (e) => {
    const scale = root.getBoundingClientRect().width / width;
    const x = (e.clientX - root.getBoundingClientRect().left) / scale;
    let best = 0;
    times.forEach((t, i) => {
      if (Math.abs(xOf(t) - x) < Math.abs(xOf(times[best]) - x)) best = i;
    });
    current = best;
    show(best);
  });
  root.addEventListener("pointerleave", hide);
  root.addEventListener("focus", () => show(current));
  root.addEventListener("blur", hide);
  root.addEventListener("keydown", (e) => {
    const step = { ArrowLeft: -1, ArrowRight: 1 }[e.key];
    if (!step) return;
    e.preventDefault();
    current = Math.min(times.length - 1, Math.max(0, current + step));
    show(current);
  });
  box.prepend(root);
}

/** The charts of the page, painted when attached and again when their width changes. */
const plots = new WeakMap();

function paintCharts(section) {
  for (const plot of section.querySelectorAll("[data-chart]")) {
    const group = plots.get(plot);
    paint(plot, group);
    let width = plot.clientWidth;
    new ResizeObserver(() => {
      if (plot.clientWidth !== width) {
        width = plot.clientWidth;
        paint(plot, group);
      }
    }).observe(plot);
  }
}

function renderChart(chart) {
  // Checked here, in the section's own try: it fails as its error card and draws nothing.
  const group = { ...chart, data: chartData(chart) };
  const box = el("div", "chart");
  if (group.series.length === 1) {
    box.append(el("p", "chart-title", group.unit ? `${group.series[0].label} (${group.unit})` : group.series[0].label));
  } else {
    if (group.unit) box.append(el("p", "chart-title", group.unit));
    const legend = el("ul", "legend");
    group.series.forEach((s, i) => {
      const item = el("li");
      const key = el("span", "key");
      key.style.background = `var(--series-${i + 1})`;
      item.append(key, document.createTextNode(s.label));
      legend.append(item);
    });
    box.append(legend);
  }
  const plot = el("div");
  plot.style.position = "relative";
  const tip = el("div", "tooltip");
  tip.hidden = true;
  plot.append(tip);
  box.append(plot);
  // A section is built detached, with no width yet: main paints it once it is in the page.
  plot.dataset.chart = "";
  plots.set(plot, group);

  // The table view: every value without hovering.
  const details = el("details");
  details.append(el("summary", "", "Table"));
  const times = [...new Set(group.series.flatMap((s) => s.points.map((p) => p[0])))].sort();
  const columns = ["time", ...group.series.map((s) => s.label)];
  const rows = times.map((t) => [
    t,
    ...group.series.map((s) => {
      const k = s.points.findIndex((p) => p[0] === t);
      if (k < 0) return null;
      const q = s.quality?.[k];
      return `${s.points[k][1] ?? "no value"}${q ? ` (${q})` : ""}`;
    }),
  ]);
  details.append(renderTable(columns, rows));
  box.append(details);
  return box;
}

function card(title, level, label, ...rest) {
  const section = el("section", "section");
  const head = el("div", "section-head");
  head.append(el("h2", "", title), statusPill(level, label));
  section.append(head, ...rest);
  return section;
}

function requireArrays(section) {
  if (section === null || typeof section !== "object") throw new Error("not an object");
  if (section.schema !== SCHEMA) throw new Error(`unknown schema ${String(section.schema)}`);
  for (const key of ["tiles", "series", "tables", "links"]) {
    if (!Array.isArray(section[key])) throw new Error(`${key} is not a list`);
  }
}

function renderSection(section, entry, sectionUrl, nowMs) {
  requireArrays(section);
  const f = freshness(section.generated_at, section.status?.level, entry.max_age_hours, nowMs);
  if (f.error) throw new Error(f.error);
  const label = f.stale ? `stale: generated ${formatAge(f.ageMs)}` : f.level;
  const out = card(section.title, f.level, label);
  const source = section.source ?? {};
  out.append(el("p", "section-meta", `${section.repo} on ${section.cluster}, generated ${section.generated_at} (${formatAge(f.ageMs)}), slot ${source.slot}, commit ${String(source.commit).slice(0, 7)}`));
  if (section.status?.message) out.append(el("p", "status-message", section.status.message));
  if (section.tiles.length) out.append(renderTiles(section.tiles));
  for (const group of chartGroups(section.series)) out.append(renderChart(group));
  for (const t of section.tables) {
    out.append(el("p", "table-title", t.title ?? t.id));
    out.append(renderTable(t.columns, t.rows, t.href_columns));
  }
  if (section.links.length) {
    const list = el("ul", "links");
    for (const l of section.links) {
      const item = el("li");
      item.append(linkOrText(l.label, linkTarget(l.href, sectionUrl)));
      list.append(item);
    }
    out.append(list);
  }
  return out;
}

function errorCard(repo, url, message) {
  return card(repo, "error", "cannot show this section", el("p", "error-text", `${url}: ${message}`));
}

/** Browsers word this error differently, and some not usefully. */
function parseJson(text) {
  try {
    return JSON.parse(text);
  } catch {
    throw new Error("not valid JSON");
  }
}

/** One section, alone: its failure is its own card, never a blank page. */
async function loadSection(entry, nowMs) {
  if (entry.pending) {
    return card(entry.repo, "pending", "pending", el("p", "error-text", "This repo has not published a section yet."));
  }
  const url = new URL(entry.url, document.baseURI).href;
  try {
    const response = await fetch(url, { cache: "no-store" });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return renderSection(parseJson(await response.text()), entry, url, nowMs);
  } catch (e) {
    return errorCard(entry.repo, entry.url, e.message);
  }
}

async function main() {
  const root = document.getElementById("sections");
  let entries;
  try {
    const response = await fetch("sections.json", { cache: "no-store" });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    entries = parseJson(await response.text());
    if (!Array.isArray(entries)) throw new Error("not a list");
  } catch (e) {
    root.append(errorCard("sections", "sections.json", e.message));
    return;
  }
  const slots = entries.map(() => el("div"));
  root.append(...slots);
  await Promise.all(
    entries.map(async (entry, i) => {
      const section = await loadSection(entry, Date.now());
      slots[i].replaceChildren(section);
      try {
        paintCharts(section);
      } catch (e) {
        slots[i].replaceChildren(errorCard(entry.repo, entry.url, e.message));
      }
    }),
  );
}

if (typeof document !== "undefined") main();
