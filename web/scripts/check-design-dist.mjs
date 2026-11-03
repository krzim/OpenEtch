#!/usr/bin/env node
// Check OpenEtch Design's built site (docs/design-only/plan.md WP5): `npm run build:design` runs this after
// `vite build --mode design`. The published bundle must carry no machine control and nothing worked out from
// LightBurn's program: none of FORBIDDEN in any file (JS, CSS, HTML, the manifest, the worker), no source maps and no
// precompressed copies. Exit 1 with the hits.
//
//   node scripts/check-design-dist.mjs [dir]     (default web/dist-design)

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { DESIGN_FORBIDDEN as FORBIDDEN } from "../src/guide/designFilter.ts";

const web = resolve(fileURLToPath(new URL(".", import.meta.url)), "..");
const dir = resolve(process.argv[2] ?? join(web, "dist-design"));

// FORBIDDEN: machine routes, MCP, and the .lbzip bundle format (its names come from LightBurn's executable).

/** Text files worth reading (everything else is an image or the wasm). */
const TEXT = /\.(js|mjs|css|html|json|webmanifest|svg|txt|map)$/;

function walk(d) {
  const out = [];
  for (const name of readdirSync(d)) {
    const p = join(d, name);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else out.push(p);
  }
  return out;
}

let files;
try {
  files = walk(dir);
} catch {
  console.error(`check-design-dist: ${dir} not found (run npm run build:design)`);
  process.exit(1);
}
const problems = [];
for (const f of files) {
  const rel = relative(dir, f).replaceAll("\\", "/");
  if (rel.endsWith(".map")) problems.push(`${rel}: a source map (would publish the source)`);
  if (/\.(br|gz)$/.test(rel)) problems.push(`${rel}: a precompressed copy (Pages compresses by itself)`);
  if (!TEXT.test(rel)) continue;
  const text = readFileSync(f, "utf8");
  for (const s of FORBIDDEN) {
    const i = text.indexOf(s);
    if (i >= 0) problems.push(`${rel}: contains "${s}" (…${text.slice(Math.max(0, i - 60), i + s.length + 40).replace(/\s+/g, " ")}…)`);
  }
}
if (problems.length) {
  console.error(`check-design-dist: ${problems.length} problem(s) in ${relative(web, dir) || dir}:`);
  for (const p of problems) console.error(`  ${p}`);
  process.exit(1);
}
console.log(`check-design-dist: ${files.length} files in ${relative(web, dir) || dir}, none of ${FORBIDDEN.join(", ")}`);
