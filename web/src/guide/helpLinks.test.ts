// Every "?" help button and every literal openGuide("...") call in the app points at a page of docs/guide and one of
// its headings, by the id GitHub gives that heading ("## Cut settings" -> "cut-settings"): a reworded heading or a
// deleted page fails here, not in front of a user. HelpLink's `to` must be a string literal so this test can read it.

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
      // Dialog.tsx and Dock.tsx draw the "?" of their `help` prop (checked where it is given, below).
      if (/\bto=\{\s*p\.help\s*\}/.test(m[1])) continue;
      const to = /\bto=(?:"([^"]*)"|\{\s*"([^"]*)"\s*\}|\{\s*'([^']*)'\s*\})/.exec(m[1]);
      out.push([rel, to ? (to[1] ?? to[2] ?? to[3]) : null]);
    }
    // A dialog's or a dock window's `help="page#anchor"` (components/Dialog.tsx, workspace/Dock.tsx): the "?" beside
    // its title. A wrapper dialog may pass its own prop on (`help={p.help}`); any other `help={...}` cannot be checked.
    for (const m of text.matchAll(/\shelp=(?:"([^"]*)"|\{\s*"([^"]*)"\s*\}|\{([^}]*)\})/g)) {
      if (m[3] !== undefined && /^\s*(p|props)\.help\s*$/.test(m[3])) continue;
      out.push([rel, m[3] !== undefined ? null : (m[1] ?? m[2])]);
    }
    if (rel === "guide/route.ts" || rel === "guide/HelpLink.tsx") continue;
    for (const m of text.matchAll(/\bopenGuide\(\s*(["'])([^"']*)\1\s*\)/g)) out.push([rel, m[2]]);
  }
  return out;
}

/** The longest heading id a "?" may link to. */
const MAX_HELP_ID = 34;

test("every HelpLink and openGuide target exists in docs/guide", () => {
  const targets = helpTargets();
  assert.ok(targets.length >= 1, "at least the demo HelpLink is found (the scan works)");
  for (const [file, to] of targets) {
    assert.ok(to !== null, `${file}: <HelpLink to="page#anchor"> must be a string literal`);
    const r = resolveTarget(guide, to);
    assert.ok(r.ok, `${file}: HelpLink to="${to}": ${r.ok ? "" : r.why}`);
    const anchor = to.split("#")[1] ?? "";
    // A "?" depends on the heading's wording: keep such headings short, so the id is short and a small rewording is
    // not tempting. And not a repeated heading's numbered id ("options-1"): another "Options" above it renumbers it.
    assert.ok(anchor.length <= MAX_HELP_ID, `${file}: "${to}": the heading's id is longer than ${MAX_HELP_ID} characters: shorten the heading`);
    const page = guide.bySlug.get(to.split("#")[0])!;
    const base = /^(.*)-\d+$/.exec(anchor)?.[1];
    assert.ok(!base || !page.headings.some((h) => h.id === base), `${file}: "${to}" is a repeated heading's numbered id: give the heading its own words`);
  }
});

test("resolveTarget names what is missing", () => {
  assert.deepEqual(resolveTarget(guide, "index#open-the-guide"), { ok: true });
  assert.deepEqual(resolveTarget(guide, "index"), { ok: true });
  assert.equal(resolveTarget(guide, "no-such-page").ok, false);
  const r = resolveTarget(guide, "index#no-such-anchor");
  assert.ok(!r.ok && /no heading with the id "no-such-anchor"/.test(r.why));
  // Ids are GitHub's, made from the heading's text: the old {#custom} ids are gone.
  assert.ok(resolveTarget(guide, "layers-and-settings#the-cuts--layers-window").ok);
  assert.equal(resolveTarget(guide, "layers-and-settings#cuts-layers-window").ok, false);
  assert.equal(resolveTarget(guide, "Bad Target").ok, false);
});
