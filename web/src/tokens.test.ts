// WCAG 2.2 AA over the colour tokens (docs/ux/ui-plan-2026-11.md 1.5, UX-09): text 4.5:1, UI components and borders
// 3:1, in the light and the dark theme. Reads styles.css (the colour tokens) with fs; the dark theme is the light
// tokens overridden by the `:root[data-theme="dark"]` block, as in the browser.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { contrast, parseHex } from "./workspace/theme.ts";

const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** The custom properties declared in the block whose selector is exactly `selector`. */
function tokens(selector: string): Record<string, string> {
  const out: Record<string, string> = {};
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(css))) {
    if (m[1].trim() !== selector) continue;
    for (const d of m[2].matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) out[d[1]] = d[2].trim();
  }
  return out;
}

const light = tokens(":root");
const dark = { ...light, ...tokens(':root[data-theme="dark"]') };
const THEMES = { light, dark } as const;

/** [foreground, background, minimum ratio, what it is]. */
const PAIRS: [string, string, number, string][] = [
  // Text.
  ...["--bg", "--panel", "--chrome", "--chrome-2", "--surface"].map((bg): [string, string, number, string] => ["--text", bg, 4.5, "body text"]),
  ...["--panel", "--chrome", "--chrome-2", "--surface"].map((bg): [string, string, number, string] => ["--text-2", bg, 4.5, "secondary text"]),
  ...["--panel", "--chrome", "--chrome-2", "--surface"].map((bg): [string, string, number, string] => ["--muted", bg, 4.5, "muted text (hints, units, menu hints)"]),
  ["--accent", "--panel", 4.5, "accent text and links"],
  ["--accent", "--surface", 4.5, "accent text on fields"],
  ["--on-accent", "--accent-btn", 4.5, "primary buttons, active segments, badges"],
  ["--danger", "--panel", 4.5, "danger text (Delete, errors)"],
  ["--on-accent", "--stop-bg", 4.5, "STOP"],
  ["--on-warn", "--warn", 4.5, "warning chips"],
  ["--warn-text", "--warn-bg", 4.5, "warning buttons and panels"],
  ["--ok", "--panel", 4.5, "ready / ok text"],
  ["--on-accent", "--ok-fill", 4.5, "query badges"],
  ["--toast-fg", "--toast-bg", 4.5, "toasts"],
  ["--on-accent", "--danger-dark", 4.5, "error toasts"],
  ["--topbar-fg", "--topbar-bg", 4.5, "top bar text"],
  ["--topbar-muted", "--topbar-bg", 4.5, "project name in the top bar"],
  ["--topbar-tab-fg", "--topbar-tabs", 4.5, "workspace switcher"],
  ["--topbar-tab-active-fg", "--topbar-tab-active-bg", 4.5, "active workspace"],
  // UI components (1.4.11).
  ["--input-border", "--surface", 3, "field borders"],
  ["--run-icon", "--chrome", 3, "Request run / Start icon"],
  ["--icon", "--chrome", 3, "ribbon icons"],
  ["--accent", "--chrome", 3, "focus ring and active outlines"],
];

for (const [name, theme] of Object.entries(THEMES)) {
  test(`tokens: every pair meets WCAG AA in the ${name} theme`, () => {
    const fails: string[] = [];
    for (const [fg, bg, min, what] of PAIRS) {
      const a = parseHex(theme[fg] ?? "");
      const b = parseHex(theme[bg] ?? "");
      assert.ok(a && b, `${name}: ${fg} (${theme[fg]}) and ${bg} (${theme[bg]}) are hex colours`);
      const r = contrast(a, b);
      if (r < min) fails.push(`${what}: ${fg} ${theme[fg]} on ${bg} ${theme[bg]} is ${r.toFixed(2)}, needs ${min}`);
    }
    assert.deepEqual(fails, []);
  });
}

test("tokens: STOP is the same red in both themes; the top bar follows the theme", () => {
  assert.equal(light["--stop-bg"], "#c62828");
  assert.equal(dark["--stop-bg"], "#c62828");
  const lum = (hex: string) => {
    const c = parseHex(hex);
    assert.ok(c);
    return contrast(c, [0, 0, 0]);
  };
  assert.ok(lum(light["--topbar-bg"]) > lum(dark["--topbar-bg"]) * 4, "light top bar in the light theme, dark in the dark one");
});

test("tokens: one dark palette (no prefers-color-scheme copy to keep in step)", () => {
  assert.ok(!/prefers-color-scheme/.test(css), "main.tsx sets data-theme before the first paint instead");
  // Every dark override names a token the light theme has.
  for (const k of Object.keys(tokens(':root[data-theme="dark"]'))) assert.ok(k in light, `${k} exists in the light theme`);
});
