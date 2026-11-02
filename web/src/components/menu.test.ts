// Unit tests for the Menu primitive's pure half (menuModel.ts): keyboard navigation and placement.

import { test } from "node:test";
import assert from "node:assert/strict";
import { firstIndex, lastIndex, menuNav, placeMenu, stepIndex, typeAhead, type NavItem } from "./menuModel.ts";

const items: NavItem[] = [
  { label: "New" },
  { label: "Open" },
  { label: "", sep: true },
  { label: "Save", disabled: true },
  { label: "Save As" },
  { label: "Export", submenu: true },
  { label: "Settings" },
];

test("roving focus skips separators and disabled rows and wraps", () => {
  assert.equal(firstIndex(items), 0);
  assert.equal(lastIndex(items), 6);
  assert.equal(stepIndex(items, 1, 1), 4, "Open -> Save As (separator and disabled Save skipped)");
  assert.equal(stepIndex(items, 4, -1), 1);
  assert.equal(stepIndex(items, 6, 1), 0, "wraps to the top");
  assert.equal(stepIndex(items, 0, -1), 6, "wraps to the bottom");
  assert.equal(stepIndex([{ label: "x", disabled: true }], 0, 1), -1);
  assert.equal(firstIndex([]), -1);
});

test("arrows, Home and End move; nothing focused starts at the ends", () => {
  assert.deepEqual(menuNav(items, -1, "ArrowDown"), { kind: "move", index: 0 });
  assert.deepEqual(menuNav(items, -1, "ArrowUp"), { kind: "move", index: 6 });
  assert.deepEqual(menuNav(items, 1, "ArrowDown"), { kind: "move", index: 4 });
  assert.deepEqual(menuNav(items, 4, "Home"), { kind: "move", index: 0 });
  assert.deepEqual(menuNav(items, 0, "End"), { kind: "move", index: 6 });
});

test("submenus open with Right / Enter and close with Left / Esc, never the menu itself", () => {
  assert.deepEqual(menuNav(items, 5, "ArrowRight"), { kind: "openSubmenu", index: 5 });
  assert.deepEqual(menuNav(items, 5, "Enter"), { kind: "openSubmenu", index: 5 });
  assert.deepEqual(menuNav(items, 5, " "), { kind: "openSubmenu", index: 5 });
  assert.deepEqual(menuNav(items, 4, "ArrowRight"), { kind: "none" }, "Right on a plain row does nothing");
  assert.deepEqual(menuNav(items, 0, "ArrowLeft", { level: 0 }), { kind: "none" }, "Left in the top menu does not close it");
  assert.deepEqual(menuNav(items, 0, "ArrowLeft", { level: 1 }), { kind: "closeSubmenu" });
  assert.deepEqual(menuNav(items, 0, "Escape", { level: 1 }), { kind: "closeSubmenu" }, "Esc in a submenu closes only it");
  assert.deepEqual(menuNav(items, 0, "Escape", { level: 0 }), { kind: "close" });
  assert.deepEqual(menuNav(items, 5, "ArrowLeft", { level: 0, rtl: true }), { kind: "openSubmenu", index: 5 }, "right-to-left swaps the arrows");
});

test("Enter activates; disabled rows and separators never activate; Tab closes everything", () => {
  assert.deepEqual(menuNav(items, 4, "Enter"), { kind: "activate", index: 4 });
  assert.deepEqual(menuNav(items, 3, "Enter"), { kind: "none" });
  assert.deepEqual(menuNav(items, 2, " "), { kind: "none" });
  assert.deepEqual(menuNav(items, 0, "Tab"), { kind: "closeAll" });
  assert.deepEqual(menuNav(items, 0, "q"), { kind: "none" });
});

test("type-ahead finds the next match, cycles on a repeated letter and skips disabled rows", () => {
  assert.equal(typeAhead(items, 0, "s"), 4, "Save is disabled: Save As");
  assert.equal(typeAhead(items, 4, "s"), 6, "next S row: Settings");
  assert.equal(typeAhead(items, 6, "ss"), 4, "repeating the letter cycles");
  assert.equal(typeAhead(items, 0, "se"), 6, "Settings (Save As does not start with 'se')");
  assert.equal(typeAhead(items, 0, "EXP"), 5, "case-insensitive");
  assert.equal(typeAhead(items, 0, "zz"), -1);
  assert.equal(typeAhead(items, 0, ""), -1);
});

const view = { width: 1280, height: 800 };

test("a context menu opens at the point and flips up near the bottom", () => {
  const p = placeMenu({ x: 100, y: 100 }, { width: 200, height: 300 }, view);
  assert.deepEqual([p.left, p.top, p.side], [100, 100, "point"]);
  const low = placeMenu({ x: 100, y: 760 }, { width: 200, height: 300 }, view);
  assert.equal(low.side, "above");
  assert.equal(low.top, 460);
  assert.ok(low.top + 300 <= view.height - 4);
});

test("a context menu flips left at the right edge and clamps when nothing fits", () => {
  const p = placeMenu({ x: 1250, y: 100 }, { width: 200, height: 100 }, view);
  assert.equal(p.left, 1050, "opens to the left of the point");
  // A menu taller than the window: clamped to the top, max-height the window, so it scrolls.
  const tall = placeMenu({ x: 100, y: 400 }, { width: 200, height: 2000 }, view);
  assert.equal(tall.top, 4);
  assert.equal(tall.maxHeight, 792);
  // Wider than the window: clamped to the margin.
  const wide = placeMenu({ x: 10, y: 10 }, { width: 2000, height: 100 }, view);
  assert.equal(wide.left, 4);
});

test("bottom-bar context menus stay inside the window", () => {
  for (const y of [790, 799, 800]) {
    const p = placeMenu({ x: 640, y }, { width: 240, height: 500 }, view);
    assert.ok(p.top >= 4 && p.top + Math.min(500, p.maxHeight) <= view.height - 4, `y=${y}`);
  }
});

test("a dropdown opens below its button, flips up when there is more room above, and gets a max-height", () => {
  const btn = { left: 100, top: 40, right: 180, bottom: 80 };
  const p = placeMenu(btn, { width: 260, height: 400 }, view, "below");
  assert.deepEqual([p.left, p.top, p.side], [100, 82, "below"]);
  assert.equal(p.maxHeight, 800 - 4 - 82);
  // A long File menu under a top-bar button on a short window: still below, scrolling inside.
  const long = placeMenu(btn, { width: 260, height: 1200 }, view, "below");
  assert.equal(long.side, "below");
  assert.equal(long.maxHeight, 714);
  // A button at the bottom: opens above.
  const bottom = placeMenu({ left: 100, top: 740, right: 180, bottom: 770 }, { width: 260, height: 300 }, view, "below");
  assert.equal(bottom.side, "above");
  assert.equal(bottom.top, 740 - 2 - 300);
  // Clamped against the right edge.
  const edge = placeMenu({ left: 1200, top: 40, right: 1270, bottom: 80 }, { width: 260, height: 100 }, view, "below");
  assert.equal(edge.left, 1280 - 4 - 260);
  // Right-aligned to its button.
  const end = placeMenu({ left: 600, top: 40, right: 700, bottom: 80 }, { width: 260, height: 100 }, view, "below", 4, "end");
  assert.equal(end.left, 440);
});

test("a submenu opens to the right, flips left at the edge and clamps its top", () => {
  const row = { left: 300, top: 100, right: 540, bottom: 132 };
  const p = placeMenu(row, { width: 200, height: 200 }, view, "right");
  assert.deepEqual([p.left, p.top, p.side], [540, 100, "right"]);
  const edge = placeMenu({ left: 1000, top: 100, right: 1240, bottom: 132 }, { width: 200, height: 200 }, view, "right");
  assert.deepEqual([edge.left, edge.side], [800, "left"]);
  const low = placeMenu({ left: 300, top: 700, right: 540, bottom: 732 }, { width: 200, height: 300 }, view, "right");
  assert.equal(low.top, 800 - 4 - 300, "moved up to fit");
});

test("placeMenu: a DOMRect-like anchor (with x and y) opens beside the rectangle, not at its corner", () => {
  const rect = { x: 6, y: 404, left: 6, top: 404, right: 45, bottom: 434, width: 39, height: 30 };
  const p = placeMenu(rect, { width: 80, height: 66 }, { width: 1024, height: 768 }, "right");
  assert.equal(p.left, 45);
  assert.equal(p.top, 404);
  const below = placeMenu(rect, { width: 80, height: 66 }, { width: 1024, height: 768 }, "below");
  assert.equal(below.top, 436);
});
