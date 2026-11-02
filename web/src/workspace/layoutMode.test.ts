// Unit tests for the layout modes (layoutMode.ts): the viewport table of docs/ux/ui-plan-2026-11.md 1.1, the
// Preferences overrides and the density rule.

import { test } from "node:test";
import assert from "node:assert/strict";
import { applyLayoutAttributes, autoDensity, autoLayout, getLayoutMode, isTextEntry, keyboardResize, resolveLayout, sameLayout } from "./layoutMode.ts";

test("the plan's test viewports land in their modes", () => {
  const cases: [number, number, boolean, string][] = [
    [390, 844, true, "phone"],
    [390, 844, false, "phone"],
    [1024, 768, false, "tablet"],
    [820, 1180, false, "tablet"],
    [820, 1180, true, "tablet"],
    [1280, 800, false, "compact"],
    [1440, 900, false, "compact"],
    [1280, 720, false, "compact"],
    [1920, 1080, false, "wide"],
  ];
  for (const [w, h, coarse, want] of cases) assert.equal(resolveLayout({ w, h, coarse }).layout, want, `${w}x${h} coarse=${coarse}`);
});

test("edges of each rule", () => {
  assert.equal(autoLayout(599, 1000, false), "phone");
  assert.equal(autoLayout(600, 1000, false), "tablet");
  assert.equal(autoLayout(1099, 1000, false), "tablet");
  assert.equal(autoLayout(1100, 1000, false), "compact");
  assert.equal(autoLayout(1599, 1000, false), "compact");
  assert.equal(autoLayout(1600, 1000, false), "wide");
  assert.equal(autoLayout(1920, 899, false), "compact", "short windows are compact");
  assert.equal(autoLayout(1920, 900, false), "wide");
});

test("landscape phone, touch laptop and a big touch screen", () => {
  assert.equal(resolveLayout({ w: 844, h: 390, coarse: true }).layout, "phone", "landscape phone");
  assert.equal(resolveLayout({ w: 844, h: 390, coarse: false }).layout, "tablet", "a short fine-pointer window is not a phone");
  assert.equal(resolveLayout({ w: 1440, h: 900, coarse: true }).layout, "tablet", "touch laptop");
  assert.equal(resolveLayout({ w: 2560, h: 1440, coarse: true }).layout, "tablet", "coarse at any width");
  assert.equal(resolveLayout({ w: 1366, h: 1024, coarse: true }).layout, "tablet", "iPad Pro 12.9 landscape");
});

test("density follows the pointer, then the layout", () => {
  assert.equal(autoDensity("wide", false), "comfortable");
  assert.equal(autoDensity("compact", false), "compact");
  assert.equal(autoDensity("tablet", false), "compact");
  assert.equal(autoDensity("phone", false), "compact");
  for (const l of ["wide", "compact", "tablet", "phone"] as const) assert.equal(autoDensity(l, true), "touch");
  assert.deepEqual(resolveLayout({ w: 1920, h: 1080, coarse: false }), { layout: "wide", pointer: "fine", density: "comfortable" });
  assert.deepEqual(resolveLayout({ w: 390, h: 844, coarse: true }), { layout: "phone", pointer: "coarse", density: "touch" });
});

test("overrides win; auto falls back to the rules", () => {
  assert.equal(resolveLayout({ w: 1920, h: 1080, coarse: false, override: "phone" }).layout, "phone");
  assert.equal(resolveLayout({ w: 390, h: 844, coarse: true, override: "wide" }).layout, "wide");
  assert.equal(resolveLayout({ w: 1280, h: 800, coarse: false, override: "auto" }).layout, "compact");
  // The density override is independent of the layout; the pointer always reports the real pointer.
  const s = resolveLayout({ w: 1024, h: 768, coarse: false, density: "touch" });
  assert.deepEqual(s, { layout: "tablet", pointer: "fine", density: "touch" });
  assert.equal(resolveLayout({ w: 390, h: 844, coarse: true, density: "comfortable" }).density, "comfortable");
  // A forced layout picks its own auto density.
  assert.equal(resolveLayout({ w: 1280, h: 800, coarse: false, override: "wide" }).density, "comfortable");
});

test("garbage sizes fall back to a desktop window", () => {
  assert.equal(resolveLayout({ w: Number.NaN, h: 0, coarse: false }).layout, "wide");
  assert.equal(resolveLayout({ w: -5, h: Infinity, coarse: false }).layout, "wide");
});

test("attributes and the store outside a browser", () => {
  const el = { dataset: {} as DOMStringMap };
  applyLayoutAttributes({ layout: "compact", pointer: "coarse", density: "touch" }, el);
  assert.deepEqual({ ...el.dataset }, { layout: "compact", pointer: "coarse", density: "touch" });
  // No window: the store reports a 1920x1080 fine-pointer desktop, and the same object while nothing changes.
  const a = getLayoutMode();
  assert.equal(a.layout, "wide");
  assert.equal(getLayoutMode(), a);
  assert.ok(sameLayout(a, { ...a }));
  assert.ok(!sameLayout(a, { ...a, density: "touch" }));
});

test("the on-screen keyboard (same width, shorter, while typing) keeps the layout", () => {
  const ipad = { w: 1180, h: 746 };
  assert.equal(keyboardResize(ipad, { w: 1180, h: 400 }, true), true);
  assert.equal(keyboardResize(ipad, { w: 1180, h: 400 }, false), false, "not typing: a real resize");
  assert.equal(keyboardResize(ipad, { w: 820, h: 400 }, true), false, "rotated");
  assert.equal(keyboardResize(ipad, { w: 1180, h: 800 }, true), false, "taller");
  const el = (tagName: string, extra: Record<string, unknown> = {}) => ({ tagName, ...extra }) as unknown as Element;
  assert.equal(isTextEntry(el("INPUT", { type: "number" })), true);
  assert.equal(isTextEntry(el("INPUT", { type: "checkbox" })), false);
  assert.equal(isTextEntry(el("TEXTAREA")), true);
  assert.equal(isTextEntry(el("BUTTON")), false);
  assert.equal(isTextEntry(null), false);
});
