// Makes a built generation of the dashboard live (scripts/dashboard.sh):
//
//   node scripts/dashboard-publish.mjs <out-dir> <out-dir>/generations/<name>
//
// The one step that changes what readers see is the rename of the `current` symlink, which is
// atomic, so they get a whole generation, the new one or the old one. Everything before it can
// fail and leaves `current` as it was. dashboard.sh holds the lock of <out-dir> around this.
import { existsSync, readdirSync, readFileSync, readlinkSync, renameSync, rmSync, symlinkSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { pathToFileURL } from "node:url";

const SHELL = ["index.html", "dashboard.mjs", "dashboard.css", "sections.json"];

/** Throws unless everything the page and the section point at is in the generation. */
export function checkGeneration(generation) {
  const need = (path) => {
    if (!existsSync(path)) throw new Error(`the generation has no ${path}`);
    return path;
  };
  for (const f of SHELL) need(join(generation, f));
  const repo = join(generation, "varswap");
  const section = JSON.parse(readFileSync(need(join(repo, "section.json")), "utf8"));
  const local = section.links.map((l) => l.href).filter((href) => !/^[a-z]+:/i.test(href));
  for (const href of local) need(join(repo, href));
  const indexHref = local.find((href) => basename(href) === "index.json");
  if (indexHref === undefined) throw new Error("the section links no index.json");
  const indexPath = join(repo, indexHref);
  const index = JSON.parse(readFileSync(indexPath, "utf8"));
  for (const record of index.records) need(join(dirname(indexPath), record));
}

/** What `current` points at, or null before the first publication. A real directory is an error. */
function target(current, fs) {
  try {
    return fs.readlink(current);
  } catch (e) {
    if (e.code === "ENOENT") return null;
    throw new Error(`${current} is not a symlink: ${e.message}`);
  }
}

// The steps are parameters so that a test can fail each one.
const REAL = { readlink: readlinkSync, remove: rmSync, symlink: symlinkSync, rename: renameSync, list: readdirSync };

/** Publishes `generation`, then removes all but it and the one it replaced. */
export function publish(out, generation, fs = REAL) {
  checkGeneration(generation);
  const current = join(out, "current");
  const before = target(current, fs);
  const after = join("generations", basename(generation));
  const link = join(out, ".current.tmp");
  fs.remove(link, { force: true });
  fs.symlink(after, link);
  fs.rename(link, current);
  // It is live from here on: an old generation left behind is worth a warning, not a failure.
  // dashboard.sh's lock keeps a second publisher out. Should one get in anyway, `current` is read
  // again before each removal, so whatever another run has made live since is never removed.
  try {
    const keep = new Set([after, before]);
    for (const name of fs.list(join(out, "generations"))) {
      const path = join("generations", name);
      if (!keep.has(path) && path !== target(current, fs)) fs.remove(join(out, path), { recursive: true });
    }
  } catch (e) {
    console.error(`dashboard-publish: published, but could not remove an old generation: ${e.message}`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    const [out, generation] = process.argv.slice(2);
    if (out === undefined || generation === undefined) throw new Error("usage: <out-dir> <generation>");
    publish(out, generation);
  } catch (e) {
    console.error(`dashboard-publish: ${e.message}`);
    process.exit(1);
  }
}
