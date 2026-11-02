// Unit tests for laser/placePopover.ts: flip, clamp, max-height, and the "never cover Start or STOP" rule (V13).

import assert from "node:assert/strict";
import { test } from "node:test";
import { intersects, placePopover, type Rect } from "./placePopover.ts";

const VIEW = { w: 1280, h: 800 };
const r = (left: number, top: number, w: number, h: number): Rect => ({ left, top, right: left + w, bottom: top + h });
const boxOf = (p: { left: number; top: number; maxHeight: number }, w: number): Rect => r(p.left, p.top, w, p.maxHeight);

test("below the anchor when there is room, inside the window", () => {
  const p = placePopover(r(100, 100, 80, 30), { w: 300, h: 400 }, VIEW);
  assert.equal(p.side, "below");
  assert.equal(p.top, 134);
  assert.equal(p.maxHeight, 400);
  assert.ok(p.top + p.maxHeight <= VIEW.h);
});

test("flips above when below is too short", () => {
  const p = placePopover(r(100, 700, 80, 30), { w: 300, h: 400 }, VIEW);
  assert.equal(p.side, "above");
  assert.equal(p.top + p.maxHeight, 696);
});

test("clamps right and caps the height to the window (scrolls inside)", () => {
  const p = placePopover(r(1200, 40, 60, 30), { w: 300, h: 2000 }, VIEW);
  assert.ok(p.left + 300 <= VIEW.w - 8);
  assert.ok(p.top + p.maxHeight <= VIEW.h - 8, "bottom inside the window");
  assert.ok(p.maxHeight < 2000);
});

test("never covers Start or STOP: the Laser dock case of the audit (V13)", () => {
  // The right panel at x 960..1280; Frame at y 406; Start and the in-panel STOP right below it.
  const panel = r(960, 48, 320, 700);
  const frame = r(972, 406, 120, 32);
  const start = r(972, 470, 180, 40);
  const stop = r(1160, 470, 104, 48);
  const size = { w: 300, h: 551 };
  const p = placePopover(frame, size, VIEW, [start, stop], { sideOf: panel });
  const box = boxOf(p, 300);
  assert.ok(!intersects(box, start), "not over Start");
  assert.ok(!intersects(box, stop), "not over STOP");
  assert.ok(box.bottom <= VIEW.h && box.top >= 0, "inside the window");
  // Beside the panel, towards the canvas: it has room for the whole popover there.
  assert.equal(p.side, "left");
  assert.equal(p.maxHeight, 551);
});

test("prefers the clear side even when a covered side would be taller", () => {
  const anchor = r(400, 300, 100, 30);
  const below = r(380, 340, 200, 40);
  const p = placePopover(anchor, { w: 260, h: 250 }, VIEW, [below]);
  assert.equal(p.side, "above");
  assert.ok(!intersects(boxOf(p, 260), below));
});

test("when every side covers something, it covers the least", () => {
  const view = { w: 400, h: 400 };
  const anchor = r(150, 180, 100, 30);
  const avoid = [r(0, 0, 400, 170), r(0, 215, 400, 185)];
  const p = placePopover(anchor, { w: 300, h: 300 }, view, avoid);
  assert.ok(p.maxHeight > 0);
  assert.ok(p.top + p.maxHeight <= view.h);
});
