// Unit tests for the shell rules (shell.ts): the chrome budget per layout mode (and that tokens.css says the same), how
// the timeline and status bar share the bottom, where toasts sit, and the window title.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { LAYOUT_MODES, type LayoutMode } from "./layoutMode.ts";
import { bottomRow, chromeBudget, chromeHeight, toastBottom, windowTitle, type ChromeBudget } from "./shell.ts";
import { SHEET_SNAPS, SNAP_FRACTION, snapFor } from "./phone.ts";

test("chromeBudget: the plan's numbers per mode", () => {
  assert.deepEqual(chromeBudget("wide"), { top: 44, tool: 64, context: 32, palette: 24, timeline: 52, status: 24, nav: 0, bottom: 76, toolRail: 0 });
  assert.deepEqual(chromeBudget("compact"), { top: 40, tool: 40, context: 32, palette: 24, timeline: 28, status: 0, nav: 0, bottom: 28, toolRail: 0 });
  assert.equal(chromeBudget("compact", true).timeline, 52, "a pinned compact timeline is full height");
  assert.equal(chromeBudget("compact", true).status, 24, "... with its own status bar");
  assert.deepEqual(chromeBudget("tablet"), { top: 48, tool: 0, context: 44, palette: 36, timeline: 44, status: 0, nav: 0, bottom: 44, toolRail: 52 });
  assert.deepEqual(chromeBudget("phone"), { top: 48, tool: 0, context: 0, palette: 0, timeline: 0, status: 0, nav: 56, bottom: 56, toolRail: 0 });
});

test("chromeBudget: compact leaves 1280x800 a canvas of at least 600 px height (170 px of chrome or less)", () => {
  const h = chromeHeight(chromeBudget("compact"));
  assert.ok(h <= 170, `compact chrome is ${h}px`);
  assert.ok(800 - h >= 600);
  // 1440x900 needs 720 px of canvas.
  assert.ok(900 - h >= 720);
  // Wide at 1920x1080 needs 820 px.
  assert.ok(1080 - chromeHeight(chromeBudget("wide")) >= 820);
  // Tablet 1024x768 needs 560 px; phone 844 px tall with sheets closed needs 700.
  assert.ok(768 - chromeHeight(chromeBudget("tablet")) >= 560);
  assert.ok(844 - chromeHeight(chromeBudget("phone")) >= 700);
});

test("bottomRow: split in wide and pinned compact, merged in compact and tablet, none on phones", () => {
  assert.equal(bottomRow("wide"), "split");
  assert.equal(bottomRow("wide", true), "split");
  assert.equal(bottomRow("compact"), "merged");
  assert.equal(bottomRow("compact", true), "split");
  assert.equal(bottomRow("tablet"), "merged");
  assert.equal(bottomRow("tablet", true), "merged", "the pin is a compact-only control");
  assert.equal(bottomRow("phone"), "none");
});

test("toastBottom: 18 px above everything under the canvas", () => {
  assert.equal(toastBottom(chromeBudget("wide")), 94);
  assert.equal(toastBottom(chromeBudget("compact")), 46);
  assert.equal(toastBottom(chromeBudget("phone")), 74);
});

/** The `--name: Npx` declarations of the block whose selector is exactly `selector` in `css`. */
function block(css: string, selector: string): Record<string, number> {
  const out: Record<string, number> = {};
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(css))) {
    const sel = m[1].replace(/\/\*[\s\S]*?\*\//g, "").trim();
    if (sel !== selector) continue;
    for (const d of m[2].matchAll(/(--[\w-]+)\s*:\s*(-?\d+(?:\.\d+)?)px\s*;/g)) out[d[1]] = Number(d[2]);
  }
  return out;
}

test("tokens.css: the chrome budget tokens match chromeBudget in every mode", () => {
  const css = readFileSync(new URL("../tokens.css", import.meta.url), "utf8");
  const root = block(css, ":root");
  const fromTokens = (mode: LayoutMode, pinned = false): Partial<ChromeBudget> => {
    const v = { ...root, ...(mode === "wide" ? {} : block(css, `:root[data-layout="${mode}"]`)) };
    if (pinned) Object.assign(v, block(css, `:root[data-layout="${mode}"][data-bottom="split"]`));
    return {
      top: v["--bar-top"],
      tool: v["--bar-tool"],
      context: v["--bar-context"],
      palette: v["--bar-palette"],
      timeline: v["--bar-timeline"],
      status: v["--bar-status"],
      nav: v["--bar-nav"],
      toolRail: v["--tool-rail-w"],
    };
  };
  for (const mode of LAYOUT_MODES) {
    const { bottom: _b, ...want } = chromeBudget(mode);
    assert.deepEqual(fromTokens(mode), want, mode);
  }
  const { bottom: _b, ...pinned } = chromeBudget("compact", true);
  assert.deepEqual(fromTokens("compact", true), pinned, "compact, pinned");
});

test("windowTitle: document name, unsaved marker, app name", () => {
  assert.equal(windowTitle("Plate", false), "Plate — OpenEtch");
  assert.equal(windowTitle("Plate", true), "Plate • — OpenEtch");
  assert.equal(windowTitle("  Plate  ", null), "Plate — OpenEtch");
  assert.equal(windowTitle(null, false), "OpenEtch");
  assert.equal(windowTitle("", true), "• — OpenEtch");
  assert.equal(windowTitle(undefined, undefined), "OpenEtch");
});

test("bottom sheet: a drag settles on the nearest height, or closes well under the peek", () => {
  assert.deepEqual(SHEET_SNAPS, ["peek", "half", "full"]);
  assert.equal(snapFor(0.3), "peek");
  assert.equal(snapFor(0.4), "peek");
  assert.equal(snapFor(0.45), "half");
  assert.equal(snapFor(0.6), "half");
  assert.equal(snapFor(0.8), "full");
  assert.equal(snapFor(1), "full");
  assert.equal(snapFor(0.2), "peek", "a little under the peek stays open");
  assert.equal(snapFor(0.1), null, "dragged well down: close");
  assert.ok(SNAP_FRACTION.peek < SNAP_FRACTION.half && SNAP_FRACTION.half < SNAP_FRACTION.full);
});

test("phone: the canvas column outweighs styles.css's `.app > * { flex: none }`, which loads after phone.css", () => {
  const phone = readFileSync(new URL("./phone.css", import.meta.url), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  const rule = /\.app > \.phone-main\s*\{[^}]*\}/.exec(phone)?.[0] ?? "";
  assert.match(rule, /flex:\s*1 1 0/);
  assert.doesNotMatch(phone, /(^|\})\s*\.phone-main\s*\{/);
});
