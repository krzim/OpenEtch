import assert from "node:assert/strict";
import { test } from "node:test";
import type { Vec2 } from "../api/types.ts";
import { ACTIONS } from "../workspace/hotkeys.ts";
import { ALIASES, searchCommands, type Command } from "../components/commandSearch.ts";
import { CANVAS_TOOLS } from "../canvasTool.ts";
import {
  FaceCache,
  HoverGate,
  IDLE,
  areaText,
  faceContains,
  facePathData,
  fillErrorText,
  fillPreview,
  fillRegion,
  onAdd,
  onClick,
  onCommitted,
  onHover,
  onKey,
  onPreview,
  parsePreview,
  readyCount,
  type Face,
  type FillState,
} from "./fillRegion.ts";

const sq = (x0: number, y0: number, x1: number, y1: number): Vec2[] => [
  [x0, y0],
  [x1, y0],
  [x1, y1],
  [x0, y1],
];
/** An eye white: a 10 mm square with a 2 mm pupil hole. */
const EYE: Face = { contours: [sq(0, 0, 10, 10), sq(4, 4, 6, 6)], areaMm2: 96 };
const BODY: Face = { contours: [sq(20, 0, 40, 20)], areaMm2: 400 };

test("invocations send only what differs from the engine's defaults", () => {
  assert.deepEqual(fillRegion([[1.234567, 2]], "layer-2"), { id: "path.fill_region", params: { points: [[1.2346, 2]], layer: "layer-2" } });
  assert.deepEqual(fillRegion([[0, 0]], null, { gapMm: 0.2, holes: false }).params, { points: [[0, 0]], gap_mm: 0.2, holes: false });
  assert.deepEqual(fillPreview([[5, 5]]), { id: "path.fill_region_preview", params: { points: [[5, 5]] } });
  assert.equal(fillPreview([[5, 5]], { gapMm: 50, holes: true }).params.gap_mm, 10, "clamped to the engine's limit");
});

test("preview answers become one answer per point", () => {
  const r = {
    faces: [{ contours: [sq(0, 0, 10, 10), sq(4, 4, 6, 6)], area_mm2: 96 }],
    points: [{ point: [1, 1], face: 0 }, { point: [50, 50], face: null, error: "no closed area", code: "not_enclosed" }],
  };
  const a = parsePreview(r, 3);
  assert.ok("face" in a[0] && a[0].face.areaMm2 === 96 && a[0].face.contours.length === 2);
  assert.deepEqual(a[1], { error: "no closed area", code: "not_enclosed" });
  assert.deepEqual(a[2], { error: "no answer", code: "error" }, "a missing answer is an error, not a crash");
  assert.deepEqual(parsePreview(null, 1), [{ error: "no answer", code: "error" }]);
  // A face with a broken outline is not shown.
  assert.ok("error" in parsePreview({ faces: [{ contours: [[[0, 0], [1, 1]]] }], points: [{ face: 0 }] }, 1)[0]);
});

test("faces: inside, holes and path data", () => {
  assert.ok(faceContains(EYE, [1, 1]));
  assert.ok(!faceContains(EYE, [5, 5]), "in the pupil hole");
  assert.ok(!faceContains(EYE, [11, 5]));
  assert.equal(facePathData({ contours: [sq(0, 0, 1, 1)], areaMm2: 1 }), "M0,0L1,0L1,1L0,1Z");
  assert.equal(areaText(96, "mm"), "96.00 mm²");
  assert.equal(areaText(645.16, "in"), "1.0000 in²");
});

test("the face cache holds faces for one revision and one set of options", () => {
  const c = new FaceCache();
  c.sync(3, { gapMm: 0, holes: true });
  c.add(EYE);
  c.add(EYE);
  assert.equal(c.size, 1);
  assert.equal(c.find([1, 1]), EYE);
  assert.equal(c.find([5, 5]), null, "a point in a hole is not in the face");
  assert.equal(c.sync(3, { gapMm: 0, holes: true }), false, "same revision: kept");
  assert.equal(c.sync(4, { gapMm: 0, holes: true }), true, "an edit drops it");
  assert.equal(c.find([1, 1]), null);
  c.add(EYE);
  assert.equal(c.sync(4, { gapMm: 0.5, holes: true }), true, "other options drop it");
  for (let i = 0; i < 250; i++) c.add({ contours: [sq(i, 0, i + 1, 1)], areaMm2: 1 });
  assert.ok(c.size <= 200, "bounded");
});

test("hover questions go one at a time; only the latest point waits", () => {
  const g = new HoverGate();
  assert.equal(g.request([1, 1]), true);
  assert.equal(g.request([2, 2]), false, "one is out");
  assert.equal(g.request([3, 3]), false);
  assert.deepEqual(g.done(), [3, 3], "the latest point waited, the older one was dropped");
  assert.equal(g.request([3, 3]), true, "the gate is free again");
  assert.equal(g.done(), null);
  assert.equal(g.request([4, 4]), true);
  g.request([5, 5]);
  g.clear();
  assert.equal(g.done(), null, "leaving the tool forgets the waiting point");
  assert.equal(g.request([6, 6]), true);
});

test("mouse: hover asks once per area, a click fills", () => {
  let [s, fx] = onHover(IDLE, [1, 1], null);
  assert.deepEqual(fx, { kind: "preview", at: [1, 1], why: "hover" });
  s = onPreview(s, [1, 1], { face: EYE });
  assert.equal(s.hover?.face, EYE);
  // Moving inside the same area asks nothing.
  [s, fx] = onHover(s, [2, 2], null);
  assert.deepEqual(fx, { kind: "none" });
  assert.equal(s.hover?.face, EYE);
  // Into the pupil: a new question (unless the cache knows the pupil).
  [s, fx] = onHover(s, [5, 5], null);
  assert.equal(fx.kind, "preview");
  [s, fx] = onHover(s, [30, 10], BODY);
  assert.deepEqual(fx, { kind: "none" }, "the cache answered");
  // A stale answer for a point no longer under the pointer changes nothing.
  assert.equal(onPreview(s, [5, 5], { face: EYE }).hover?.face, BODY);
  [s, fx] = onClick(s, { at: [1, 1], shift: false, touch: false, known: EYE });
  assert.deepEqual(fx, { kind: "commit", points: [[1, 1]] });
  assert.ok(s.busy);
  // While busy, clicks are ignored.
  assert.deepEqual(onClick(s, { at: [2, 2], shift: false, touch: false, known: EYE })[1], { kind: "none" });
  s = onCommitted(s, null);
  assert.ok(!s.busy && s.pending.length === 0 && s.error === null);
});

test("mouse: Shift+click collects, Enter fills all in one command, Esc clears then leaves", () => {
  let s: FillState = IDLE;
  let fx;
  [s, fx] = onClick(s, { at: [1, 1], shift: true, touch: false, known: EYE });
  assert.deepEqual(fx, { kind: "none" });
  [s, fx] = onClick(s, { at: [30, 10], shift: true, touch: false, known: null });
  assert.deepEqual(fx, { kind: "preview", at: [30, 10], why: "add" });
  s = onPreview(s, [30, 10], { face: BODY });
  assert.equal(readyCount(s), 2);
  // Shift+click in an area already collected takes it out again.
  const [less] = onClick(s, { at: [2, 2], shift: true, touch: false, known: EYE });
  assert.equal(less.pending.length, 1);
  // A plain click fills the collection plus the clicked area (not twice when it is one of them).
  assert.deepEqual(onClick(s, { at: [3, 3], shift: false, touch: false, known: EYE })[1], { kind: "commit", points: [[1, 1], [30, 10]] });
  assert.deepEqual(onClick(s, { at: [60, 60], shift: false, touch: false, known: null })[1], { kind: "commit", points: [[1, 1], [30, 10], [60, 60]] });
  const enter = onKey(s, "Enter");
  assert.ok(enter);
  assert.deepEqual(enter[1], { kind: "commit", points: [[1, 1], [30, 10]] });
  assert.deepEqual(onAdd(s)[1], enter[1], "the bar's Add is Enter");
  // A failed fill keeps the collection and shows why.
  const failed = onCommitted(enter[0], "point 2 (30, 10): no closed area");
  assert.equal(failed.pending.length, 2);
  assert.equal(failed.error, "point 2 (30, 10): no closed area");
  const esc = onKey(failed, "Escape");
  assert.ok(esc);
  assert.deepEqual(esc[1], { kind: "none" });
  assert.equal(esc[0].pending.length, 0);
  assert.equal(esc[0].error, null);
  assert.deepEqual(onKey(esc[0], "Escape")?.[1], { kind: "exit" }, "a second Esc leaves the tool");
  assert.equal(onKey(IDLE, "Enter"), null, "Enter with nothing collected does nothing");
  assert.equal(onKey(IDLE, "a"), null);
});

test("Shift+click on an area with no region drops it and says why", () => {
  let [s] = onClick(IDLE, { at: [60, 60], shift: true, touch: false, known: null });
  s = onPreview(s, [60, 60], { error: "No closed area here", code: "not_enclosed" });
  assert.equal(s.pending.length, 0);
  assert.equal(s.error, "No closed area here");
  // Two Shift+clicks in one area whose answer came late keep one.
  let [t] = onClick(IDLE, { at: [1, 1], shift: true, touch: false, known: null });
  [t] = onClick(t, { at: [2, 2], shift: true, touch: false, known: null });
  t = onPreview(t, [1, 1], { face: EYE });
  t = onPreview(t, [2, 2], { face: EYE });
  assert.equal(t.pending.length, 1);
});

test("touch: a tap shows the area, a second tap in it (or Add) fills it", () => {
  let s: FillState = IDLE;
  let fx;
  [s, fx] = onClick(s, { at: [1, 1], shift: false, touch: true, known: null });
  assert.deepEqual(fx, { kind: "preview", at: [1, 1], why: "arm" });
  assert.equal(readyCount(s), 0, "not ready before the answer");
  assert.deepEqual(onAdd(s)[1], { kind: "none" });
  s = onPreview(s, [1, 1], { face: EYE });
  assert.equal(readyCount(s), 1);
  // A tap in another area moves the highlight there.
  const [moved, fx2] = onClick(s, { at: [30, 10], shift: false, touch: true, known: BODY });
  assert.deepEqual(fx2, { kind: "none" });
  assert.equal(moved.armed?.face, BODY);
  // A second tap in the same area fills it.
  [s, fx] = onClick(s, { at: [8, 8], shift: false, touch: true, known: EYE });
  assert.deepEqual(fx, { kind: "commit", points: [[1, 1]] });
  // Add does the same.
  const armed = onPreview(onClick(IDLE, { at: [1, 1], shift: false, touch: true, known: null })[0], [1, 1], { face: EYE });
  assert.deepEqual(onAdd(armed)[1], { kind: "commit", points: [[1, 1]] });
  // A tap with no area shows the reason and arms nothing.
  const none = onPreview(onClick(IDLE, { at: [60, 60], shift: false, touch: true, known: null })[0], [60, 60], { error: "No closed area here", code: "not_enclosed" });
  assert.equal(none.armed, null);
  assert.equal(none.error, "No closed area here");
});

test("touch: a second tap near the first counts even in an area smaller than the finger", () => {
  const tiny: Face = { contours: [sq(0, 0, 0.5, 0.5)], areaMm2: 0.25 };
  const s = onPreview(onClick(IDLE, { at: [0.25, 0.25], shift: false, touch: true, known: null })[0], [0.25, 0.25], { face: tiny });
  assert.equal(onClick(s, { at: [0.9, 0.25], shift: false, touch: true, known: null, tolMm: 1 })[1].kind, "commit");
  assert.equal(onClick(s, { at: [3, 0.25], shift: false, touch: true, known: null, tolMm: 1 })[1].kind, "preview");
});

test("touch with Shift (the modifier bar) collects; Add fills the collection and the armed area", () => {
  let [s] = onClick(IDLE, { at: [1, 1], shift: true, touch: true, known: EYE });
  [s] = onClick(s, { at: [30, 10], shift: false, touch: true, known: BODY });
  assert.equal(readyCount(s), 2);
  assert.deepEqual(onAdd(s)[1], { kind: "commit", points: [[1, 1], [30, 10]] });
});

test("engine errors lose the command id and the closing words", () => {
  assert.equal(fillErrorText("path.fill_region: point 1 (60, 60): no closed area around this point; nothing was created"), "point 1 (60, 60): no closed area around this point");
  const other = "invalid input for 'path.fill_region': gap_mm must be at most 10 (got 50)";
  assert.equal(fillErrorText(other), other, "other errors stay as they are");
});

test("the tool is a canvas tool, has a hotkey action and is found by command search", () => {
  assert.ok(CANVAS_TOOLS.includes("fill"));
  const a = ACTIONS.find((x) => x.id === "tool.fillRegion");
  assert.ok(a && a.group === "tools");
  const cmds: Command[] = [{ key: a.id, label: a.label, actionId: a.id, aliases: ALIASES[a.id], run: () => undefined }];
  assert.equal(searchCommands(cmds, "paint bucket", [])[0]?.key, "tool.fillRegion");
  assert.equal(searchCommands(cmds, "fill region", [])[0]?.key, "tool.fillRegion");
});
