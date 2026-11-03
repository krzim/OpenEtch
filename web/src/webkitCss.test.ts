// Safari guards over the style sheets (web/e2e/README.md "Real Safari"): CSS that Safari ignores or takes only under
// a WebKit name. Reads every .css file under src/ with fs.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL(".", import.meta.url));

function cssFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? cssFiles(join(dir, e.name)) : e.name.endsWith(".css") ? [join(dir, e.name)] : []));
}

/** [selector, body] of every plain rule (at-rule bodies are searched as well: the regex sees their inner rules). */
function rules(): [string, string][] {
  const out: [string, string][] = [];
  for (const f of cssFiles(root)) {
    const css = readFileSync(f, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    for (const m of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) out.push([m[1].trim().replace(/\s+/g, " "), m[2]]);
  }
  return out;
}

const all = rules();

test("safari: every scroll bar hidden with scrollbar-width is hidden the WebKit way too (Safari < 18.2)", () => {
  const webkit = new Set(
    all
      .filter(([, body]) => /display\s*:\s*none/.test(body))
      .flatMap(([sel]) => sel.split(",").map((s) => s.trim()))
      .filter((s) => s.endsWith("::-webkit-scrollbar"))
      .map((s) => s.slice(0, -"::-webkit-scrollbar".length)),
  );
  const missing = all
    .filter(([, body]) => /scrollbar-width\s*:\s*none/.test(body))
    .flatMap(([sel]) => sel.split(",").map((s) => s.trim()))
    .filter((s) => !webkit.has(s));
  assert.deepEqual(missing, [], "add `<selector>::-webkit-scrollbar { display: none }` for these");
});

test("safari: backdrop-filter comes with -webkit-backdrop-filter (Safari < 18)", () => {
  const bad = all.filter(([, body]) => /(^|[;\s])backdrop-filter\s*:/.test(body) && !/-webkit-backdrop-filter\s*:/.test(body)).map(([s]) => s);
  assert.deepEqual(bad, []);
});

test("safari: fields keep 16 px on touch screens (iOS zooms the page into a smaller focused field)", () => {
  const coarse = all.find(([sel, body]) => /^input, select, textarea$/.test(sel) && /font-size\s*:\s*max\(16px/.test(body));
  assert.ok(coarse, "the (pointer: coarse) rule giving input, select and textarea at least 16 px is gone");
});
