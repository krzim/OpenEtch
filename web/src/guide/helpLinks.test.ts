// Every "?" help button and every literal openGuide("...") call in the app points at a page of docs/guide and one of
// its {#anchor} headings: a renamed anchor or a deleted page fails here, not in front of a user. HelpLink's `to` must
// be a string literal so this test can read it.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { buildGuide, resolveTarget } from "./pages.ts";

const src = fileURLToPath(new URL("../", import.meta.url));
const docs = fileURLToPath(new URL("../../../docs/guide/", import.meta.url));
const guide = buildGuide(Object.fromEntries(readdirSync(docs).filter((f) => f.endsWith(".md")).map((f) => [f, readFileSync(join(docs, f), "utf8")])));

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) return f === "wasm" ? [] : sources(p);
    return /\.tsx?$/.test(f) && !/\.test\.ts$/.test(f) ? [p] : [];
  });
}

/** [file, target] of every HelpLink and literal openGuide call; [file, null] for a HelpLink whose `to` is not a literal. */
export function helpTargets(): [string, string | null][] {
  const out: [string, string | null][] = [];
  for (const file of sources(src)) {
    // Comments left out (they show examples), JSX comments too.
    const text = readFileSync(file, "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^\s*\/\/.*$/gm, "");
    const rel = relative(src, file).split("\\").join("/");
    for (const m of text.matchAll(/<HelpLink\b([^>]*?)\/?>/gs)) {
      const to = /\bto=(?:"([^"]*)"|\{\s*"([^"]*)"\s*\}|\{\s*'([^']*)'\s*\})/.exec(m[1]);
      out.push([rel, to ? (to[1] ?? to[2] ?? to[3]) : null]);
    }
    if (rel === "guide/route.ts" || rel === "guide/HelpLink.tsx") continue;
    for (const m of text.matchAll(/\bopenGuide\(\s*(["'])([^"']*)\1\s*\)/g)) out.push([rel, m[2]]);
  }
  return out;
}

test("every HelpLink and openGuide target exists in docs/guide", () => {
  const targets = helpTargets();
  assert.ok(targets.length >= 1, "at least the demo HelpLink is found (the scan works)");
  for (const [file, to] of targets) {
    assert.ok(to !== null, `${file}: <HelpLink to="page#anchor"> must be a string literal`);
    const r = resolveTarget(guide, to);
    assert.ok(r.ok, `${file}: HelpLink to="${to}": ${r.ok ? "" : r.why}`);
  }
});

test("resolveTarget names what is missing", () => {
  assert.deepEqual(resolveTarget(guide, "index#open-the-guide"), { ok: true });
  assert.deepEqual(resolveTarget(guide, "index"), { ok: true });
  assert.equal(resolveTarget(guide, "no-such-page").ok, false);
  const r = resolveTarget(guide, "index#no-such-anchor");
  assert.ok(!r.ok && /no heading \{#no-such-anchor\}/.test(r.why));
  assert.equal(resolveTarget(guide, "Bad Target").ok, false);
});
