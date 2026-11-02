// Unit tests for the canvas HUD's slot layout (hud/hudLayout.ts): items stack instead of overlapping, shrink into
// narrow rows, keep clear of obstacles (rulers, the bottom sheet, dock overlays) and of the safe-area bands.

import assert from "node:assert/strict";
import { test } from "node:test";
import { freeSpans, intersects, layoutHud, safeArea, type Box, type HudItemSpec, type HudPlacement } from "./hudLayout.ts";

const AREA: Box = { x: 0, y: 0, w: 393, h: 500 };
const box = (p: HudPlacement): Box => ({ x: p.x, y: p.y, w: p.w, h: p.h });

function noOverlaps(out: Record<string, HudPlacement>, obstacles: Box[] = []) {
  const shown = Object.entries(out).filter(([, p]) => !p.hidden);
  for (let i = 0; i < shown.length; i++) {
    for (let j = i + 1; j < shown.length; j++)
      assert.ok(!intersects(box(shown[i][1]), box(shown[j][1])), `${shown[i][0]} overlaps ${shown[j][0]}`);
    for (const o of obstacles) assert.ok(!intersects(box(shown[i][1]), o), `${shown[i][0]} overlaps an obstacle`);
  }
}

function inside(p: HudPlacement, a: Box) {
  assert.ok(p.x >= a.x - 0.01 && p.y >= a.y - 0.01 && p.x + p.w <= a.x + a.w + 0.01 && p.y + p.h <= a.y + a.h + 0.01, "inside the area");
}

test("free spans skip the blocked stretches", () => {
  assert.deepEqual(freeSpans(0, 100, []), [[0, 100]]);
  assert.deepEqual(
    freeSpans(0, 100, [
      { x: 10, y: 0, w: 10, h: 1 },
      { x: 15, y: 0, w: 20, h: 1 },
      { x: 90, y: 0, w: 20, h: 1 },
    ]),
    [
      [0, 10],
      [35, 90],
    ],
  );
});

test("a corner item sits in its corner, a gap from the edges", () => {
  const out = layoutHud(AREA, [], [
    { id: "fab", slot: "br", rank: 1, w: 44, h: 150 },
    { id: "view", slot: "tr", rank: 1, w: 98, h: 48 },
    { id: "label", slot: "bl", rank: 2, w: 120, h: 24 },
  ]);
  assert.deepEqual(box(out.fab), { x: 393 - 8 - 44, y: 500 - 8 - 150, w: 44, h: 150 });
  assert.deepEqual(box(out.view), { x: 393 - 8 - 98, y: 8, w: 98, h: 48 });
  assert.deepEqual(box(out.label), { x: 8, y: 500 - 8 - 24, w: 120, h: 24 });
});

test("the owner's phone case: FABs, field label, player row and modifier bar stack without overlapping", () => {
  const items: HudItemSpec[] = [
    { id: "fabs", slot: "br", rank: 10, w: 46, h: 150 },
    { id: "label", slot: "bl", rank: 15, w: 130, h: 24 },
    { id: "player", slot: "bc", rank: 20, w: 520, h: 52, minW: 240 },
    { id: "mods", slot: "bl", rank: 30, w: 220, h: 54 },
    { id: "view", slot: "tr", rank: 10, w: 98, h: 48 },
    { id: "warn", slot: "tr", rank: 60, w: 300, h: 50, minW: 160 },
  ];
  const left = { x: 0, y: 0, w: 28, h: 500 }; // the left ruler
  const out = layoutHud(AREA, [left], items);
  noOverlaps(out, [left]);
  for (const p of Object.values(out)) inside(p, AREA);
  // The player row shrank beside the FAB column, above the field label.
  assert.ok(out.player.w < 520 && out.player.w >= 240);
  assert.ok(out.player.y + out.player.h <= out.label.y);
  // The modifier bar stacked above it, still at the left edge (clear of the ruler).
  assert.ok(out.mods.y + out.mods.h <= out.player.y);
  assert.equal(out.mods.x, 28 + 8);
  // The warning went beside the view buttons, not over them.
  assert.ok(out.warn.x + out.warn.w <= out.view.x || out.warn.y >= out.view.y + out.view.h);
});

test("a wide row puts edge items side by side", () => {
  const out = layoutHud({ x: 0, y: 0, w: 1200, h: 700 }, [], [
    { id: "label", slot: "bl", rank: 1, w: 130, h: 24 },
    { id: "player", slot: "bc", rank: 2, w: 600, h: 40, minW: 240 },
    { id: "mods", slot: "bl", rank: 3, w: 220, h: 54 },
  ]);
  noOverlaps(out);
  assert.equal(out.player.w, 600, "room for its natural width");
  assert.equal(out.player.y + out.player.h, 700 - 8, "on the bottom row");
  assert.equal(out.player.x, (1200 - 600) / 2, "centred");
  // The modifier bar did not fit between the label and the player row: it stacked above the label.
  assert.ok(out.mods.y + out.mods.h <= out.label.y);
  assert.equal(out.mods.x, 8);
});

test("toasts and warnings, placed last, never cover the controls", () => {
  const out = layoutHud(AREA, [], [
    { id: "view", slot: "tr", rank: 10, w: 98, h: 48 },
    { id: "tool", slot: "tl", rank: 20, w: 330, h: 48, minW: 180 },
    { id: "warn", slot: "tr", rank: 60, w: 300, h: 50, minW: 160 },
    { id: "fabs", slot: "br", rank: 10, w: 46, h: 150 },
    { id: "toast", slot: "bc", rank: 90, w: 360, h: 40, minW: 200 },
  ]);
  noOverlaps(out);
  assert.ok(out.tool.w < 330, "the tool bar shrinks beside the view buttons");
  assert.ok(out.tool.x + out.tool.w <= out.view.x);
  assert.ok(out.warn.y >= Math.max(out.view.y + out.view.h, out.tool.y + out.tool.h));
  assert.ok(out.toast.w >= 200 && out.toast.x + out.toast.w <= out.fabs.x, "beside the FABs");
});

test("bottom items stay above the phone's bottom sheet at its current height", () => {
  const sheet = { x: 0, y: 300, w: 393, h: 200 };
  const out = layoutHud(AREA, [sheet], [
    { id: "fabs", slot: "br", rank: 10, w: 46, h: 150 },
    { id: "mods", slot: "bl", rank: 30, w: 220, h: 54 },
  ]);
  noOverlaps(out, [sheet]);
  assert.equal(out.fabs.y + out.fabs.h, 300 - 8);
  assert.equal(out.mods.y + out.mods.h, 300 - 8);
});

test("a side overlay pushes edge items aside", () => {
  const overlay = { x: 600, y: 0, w: 400, h: 700 };
  const out = layoutHud({ x: 0, y: 0, w: 1000, h: 700 }, [overlay], [
    { id: "view", slot: "tr", rank: 10, w: 98, h: 48 },
    { id: "pill", slot: "br", rank: 10, w: 120, h: 44 },
  ]);
  noOverlaps(out, [overlay]);
  assert.equal(out.view.x + out.view.w, 600 - 8);
  assert.equal(out.pill.x + out.pill.w, 600 - 8);
});

test("no room: optional items hide, required ones keep their spot", () => {
  const tiny = { x: 0, y: 0, w: 393, h: 120 };
  const out = layoutHud(tiny, [], [
    { id: "fabs", slot: "br", rank: 10, w: 46, h: 150, optional: true },
    { id: "view", slot: "tr", rank: 10, w: 98, h: 48 },
    { id: "warn", slot: "tr", rank: 60, w: 380, h: 80 },
  ]);
  assert.equal(out.fabs.hidden, true);
  assert.equal(out.view.hidden, false);
  assert.equal(out.warn.hidden, false);
  assert.equal(out.warn.overlaps, true);
});

test("an anchored item sits above its anchor, or below it when there is no room, and avoids the controls", () => {
  const sel = { x: 150, y: 200, w: 100, h: 80 };
  const items: HudItemSpec[] = [
    { id: "view", slot: "tr", rank: 10, w: 98, h: 48 },
    { id: "bar", slot: "bc", rank: 80, w: 300, h: 52, anchor: { box: sel, side: "above" }, optional: true },
  ];
  const out = layoutHud(AREA, [], items);
  assert.equal(out.bar.y + out.bar.h, 200 - 12);
  assert.equal(out.bar.x, 200 - 150);
  // The selection at the top: the bar goes below it.
  const high = layoutHud(AREA, [], [{ ...items[1], anchor: { box: { x: 150, y: 20, w: 100, h: 60 }, side: "above" } }]);
  assert.equal(high.bar.y, 80 + 12);
  // A control right above the selection: the bar moves past it, not onto it.
  const blocked = layoutHud(AREA, [], [{ id: "warn", slot: "tc", rank: 1, w: 377, h: 60 }, { ...items[1], anchor: { box: { x: 150, y: 120, w: 100, h: 60 }, side: "above" } }]);
  noOverlaps(blocked);
});

test("a centre item takes the largest free rectangle and scrolls when it is shorter", () => {
  const out = layoutHud(AREA, [], [
    { id: "fabs", slot: "br", rank: 10, w: 46, h: 150 },
    { id: "mods", slot: "bl", rank: 30, w: 220, h: 54 },
    { id: "card", slot: "c", rank: 70, w: 420, h: 600, minW: 260 },
  ]);
  noOverlaps(out);
  assert.ok(out.card.w >= 260 && out.card.w <= 393 - 16);
  assert.ok(out.card.maxH !== undefined && out.card.maxH < 600);
  // The phone's Undo / Redo column is required: the card leaves it room (an import undone by mistake can be redone).
  assert.equal(out.fabs.hidden, false);
  assert.ok(out.card.x + out.card.w <= out.fabs.x, "the card stays left of the column");
});

test("ranks decide the stacking order, not the list order", () => {
  const a = layoutHud(AREA, [], [
    { id: "late", slot: "bl", rank: 5, w: 100, h: 30 },
    { id: "early", slot: "bl", rank: 1, w: 380, h: 30 },
  ]);
  assert.ok(a.early.y > a.late.y, "the lower rank is nearer the bottom");
});

test("the safe area trims only the bands the box reaches into", () => {
  const vp = { w: 852, h: 330 };
  const safe = { top: 0, right: 59, bottom: 21, left: 59 };
  // A canvas from x 40 (after a 40 px rail) to the right edge, from y 48 down to 300.
  assert.deepEqual(safeArea({ x: 40, y: 48, w: 812, h: 252 }, vp, safe), { x: 19, y: 0, w: 812 - 19 - 59, h: 252 - 0 - 0 });
  assert.deepEqual(safeArea({ x: 0, y: 0, w: 852, h: 330 }, vp, safe), { x: 59, y: 0, w: 852 - 118, h: 330 - 21 });
});

test("messages come after the required controls and before the optional ones", () => {
  const out = layoutHud(AREA, [], [
    { id: "mods", slot: "bl", rank: 30, w: 220, h: 54, optional: true },
    { id: "toast", slot: "bl", rank: 90, w: 300, h: 40, minW: 200, late: true },
    { id: "player", slot: "bc", rank: 20, w: 377, h: 52 },
  ]);
  noOverlaps(out);
  // The player row keeps the bottom; the toast goes right above it; the optional bar moves above the toast.
  assert.ok(out.toast.y + out.toast.h <= out.player.y);
  assert.ok(out.mods.y + out.mods.h <= out.toast.y);
});

test("an optional edge item hides rather than cross the middle; a required one goes on", () => {
  const area = { x: 0, y: 0, w: 393, h: 260 };
  const out = layoutHud(area, [], [
    { id: "player", slot: "bc", rank: 20, w: 377, h: 100 },
    { id: "mods", slot: "bl", rank: 30, w: 220, h: 54, optional: true },
    { id: "tool", slot: "bl", rank: 25, w: 220, h: 54 },
  ]);
  noOverlaps(out);
  assert.equal(out.mods.hidden, true);
  assert.equal(out.tool.hidden, false);
  assert.ok(out.tool.y + out.tool.h <= out.player.y);
});

test("a stack of toasts shows only the newest ones when the full stack has no room", () => {
  const area = { x: 0, y: 0, w: 393, h: 200 };
  const out = layoutHud(area, [], [
    { id: "view", slot: "tr", rank: 10, w: 98, h: 48 },
    { id: "player", slot: "bc", rank: 20, w: 377, h: 100 },
    { id: "toasts", slot: "tc", rank: 90, w: 360, h: 96, minW: 200, late: true, shorter: [44] },
  ]);
  noOverlaps(out);
  assert.equal(out.toasts.h, 44);
  assert.equal(out.toasts.maxH, 44);
});

test("with no room at all a required item still keeps clear of the obstacles", () => {
  const ruler = { x: 0, y: 0, w: 393, h: 20 };
  const out = layoutHud({ x: 0, y: 0, w: 393, h: 140 }, [ruler], [
    { id: "player", slot: "bc", rank: 20, w: 377, h: 90 },
    { id: "toast", slot: "tc", rank: 90, w: 300, h: 44, late: true },
  ]);
  assert.equal(out.toast.overlaps, true);
  assert.ok(!intersects(box(out.toast), ruler), "not over the ruler");
});

test("with the canvas all but covered by the sheet, even a required item hides instead of cutting into the sheet", () => {
  const sheet = { x: 0, y: 10, w: 852, h: 40 };
  const out = layoutHud({ x: 0, y: 0, w: 852, h: 50 }, [sheet], [{ id: "warn", slot: "tr", rank: 60, w: 400, h: 30 }]);
  assert.equal(out.warn.hidden, true);
});
