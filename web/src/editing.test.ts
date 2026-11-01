// Unit tests for editing.ts: nudge and rotate-key steps, Shift constraint, snapping (keypoints, grabbed point, Alt
// guides, grid), scale/rotate handles, align/distribute moves, micrometre rounding and typed maths.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  alignMoves,
  boxKeypoints,
  constrainDelta,
  distributeMoves,
  evalNumber,
  grabbedKeypoint,
  handlePoint,
  nudgeDelta,
  nudgeStep,
  rotateAbout,
  rotateFromHandle,
  rotateKeyDeg,
  rotatedRectTargets,
  roundMm,
  scaleAbout,
  scaleFromHandle,
  snapMove,
  targetsOf,
} from "./editing.ts";

const none = { shift: false, ctrl: false };
const box = { x0: 0, y0: 0, x1: 10, y1: 10 };

test("nudge steps follow LightBurn: 5, Ctrl 1, Shift 20, Ctrl+Shift 0.1 mm", () => {
  assert.equal(nudgeStep(none), 5);
  assert.equal(nudgeStep({ shift: false, ctrl: true }), 1);
  assert.equal(nudgeStep({ shift: true, ctrl: false }), 20);
  assert.equal(nudgeStep({ shift: true, ctrl: true }), 0.1);
  assert.deepEqual(nudgeDelta("ArrowUp", none), [0, 5], "Up is +Y");
  assert.deepEqual(nudgeDelta("ArrowLeft", { shift: true, ctrl: false }), [-20, 0]);
  assert.deepEqual(nudgeDelta("ArrowDown", { shift: false, ctrl: true }), [0, -1]);
  assert.equal(nudgeDelta("a", none), null);
});

test("rotate keys: . clockwise, , counter-clockwise; Shift 45, Ctrl 15, Ctrl+Shift 1.5", () => {
  assert.equal(rotateKeyDeg(".", none), -90);
  assert.equal(rotateKeyDeg(",", none), 90);
  assert.equal(rotateKeyDeg(">", { shift: true, ctrl: false }), -45);
  assert.equal(rotateKeyDeg(",", { shift: false, ctrl: true }), 15);
  assert.equal(rotateKeyDeg("<", { shift: true, ctrl: true }), 1.5);
  assert.equal(rotateKeyDeg("x", none), null);
});

test("Shift constrains to horizontal, vertical or 45 degrees", () => {
  assert.deepEqual(constrainDelta([10, 2]), [10, 0]);
  assert.deepEqual(constrainDelta([-1, -9]), [0, -9]);
  assert.deepEqual(constrainDelta([10, -8]), [9, -9]);
  assert.deepEqual(constrainDelta([0, 0]), [0, 0]);
});

test("keypoints and targets", () => {
  const k = boxKeypoints(box);
  assert.equal(k.length, 9);
  assert.deepEqual(k[8], { p: [5, 5], kind: "center" });
  assert.equal(targetsOf(box, "Stock")[8].label, "centre of Stock");
  const r = rotatedRectTargets([0, 0], 20, 10, 90, "Stock");
  assert.ok(Math.abs(r[0].p[0] - 5) < 1e-9 && Math.abs(r[0].p[1] + 10) < 1e-9, "corner rotated by 90 degrees");
});

test("snap to the nearest target keypoint within the radius", () => {
  const targets = targetsOf({ x0: 20, y0: 0, x1: 30, y1: 10 }, "B");
  // Moving right by 9.6: the right edge (x 19.6) is 0.4 from B's left edge.
  const r = snapMove(box, [9.6, 0.2], targets, { radius: 1 });
  assert.deepEqual(r.delta, [10, 0]);
  assert.equal(r.target?.kind, "corner");
  const far = snapMove(box, [5, 0], targets, { radius: 1 });
  assert.deepEqual(far.delta, [5, 0]);
  assert.equal(far.target, null);
});

test("a grabbed keypoint is the only one that snaps", () => {
  const targets = [{ p: [50, 50] as [number, number], kind: "center" as const, label: "x" }];
  const grab = grabbedKeypoint(box, [5.2, 4.9], 0.5);
  assert.deepEqual(grab, [5, 5]);
  const r = snapMove(box, [44.7, 45.2], targets, { radius: 1, grab });
  assert.deepEqual(r.delta, [45, 45]);
  // The corner (10,10) moved by the delta is near (50,50), but it is not the grabbed point.
  const r2 = snapMove(box, [40.2, 40.1], targets, { radius: 1, grab });
  assert.deepEqual(r2.delta, [40.2, 40.1]);
  assert.equal(grabbedKeypoint(box, [2.5, 2.5], 0.5), null);
});

test("Alt guides align edges on one axis; the grid snaps when nothing else does", () => {
  const targets = targetsOf({ x0: 100, y0: 40, x1: 110, y1: 50 }, "B");
  const r = snapMove(box, [30, 39.7], targets, { radius: 1, guides: true });
  assert.deepEqual(r.delta, [30, 40]);
  assert.equal(r.guides.length, 1);
  assert.equal(r.guides[0].a[1], 40);
  const g = snapMove(box, [1.3, 2.6], [], { radius: 1, grid: 1 });
  assert.deepEqual(g.delta, [1, 3]);
});

test("scale handles: corners keep aspect, Shift frees it, edges scale one axis, Ctrl about the centre", () => {
  assert.deepEqual(handlePoint(box, "ne"), [10, 10]);
  assert.deepEqual(handlePoint(box, "s"), [5, 0]);
  let s = scaleFromHandle(box, "ne", [20, 12], none);
  assert.deepEqual(s, { sx: 2, sy: 2, about: [0, 0] });
  s = scaleFromHandle(box, "ne", [20, 12], { shift: true, ctrl: false });
  assert.equal(s.sx, 2);
  assert.ok(Math.abs(s.sy - 1.2) < 1e-9);
  s = scaleFromHandle(box, "e", [15, 99], none);
  assert.deepEqual(s, { sx: 1.5, sy: 1, about: [0, 5] });
  s = scaleFromHandle(box, "e", [15, 0], { shift: false, ctrl: true });
  assert.deepEqual(s, { sx: 2, sy: 1, about: [5, 5] });
  s = scaleFromHandle(box, "w", [20, 5], none);
  assert.ok(s.sx > 0, "never flips");
});

test("rotate handle snaps to 15 degrees with Shift and 5 with Ctrl", () => {
  const c: [number, number] = [0, 0];
  assert.ok(Math.abs(rotateFromHandle(c, [10, 0], [10, 10], none) - 45) < 1e-9);
  assert.equal(rotateFromHandle(c, [10, 0], [10, 3], { shift: true, ctrl: false }), 15);
  assert.equal(rotateFromHandle(c, [10, 0], [10, 3], { shift: false, ctrl: true }), 15);
  assert.equal(rotateFromHandle(c, [10, 0], [10, 1.2], { shift: false, ctrl: true }), 5);
  const m = rotateAbout(90, [1, 1]);
  // (2,1) rotated 90 degrees about (1,1) is (1,2).
  assert.ok(Math.abs(m[0] * 2 + m[2] * 1 + m[4] - 1) < 1e-9 && Math.abs(m[1] * 2 + m[3] * 1 + m[5] - 2) < 1e-9);
  assert.deepEqual(scaleAbout(2, 3, [1, 1]), [2, 0, 0, 3, -1, -2]);
});

test("align moves units onto the reference keypoint", () => {
  const units = [
    { id: "a", box: { x0: 0, y0: 0, x1: 10, y1: 10 } },
    { id: "b", box: { x0: 20, y0: 5, x1: 26, y1: 9 } },
  ];
  const ref = units[0].box;
  assert.deepEqual(alignMoves(units, ref, "left"), [{ id: "b", d: [-20, 0] }]);
  assert.deepEqual(alignMoves(units, ref, "right", "top"), [{ id: "b", d: [-16, 1] }]);
  assert.deepEqual(alignMoves(units, { x0: -50, y0: -50, x1: 50, y1: 50 }, "center", "center"), [
    { id: "a", d: [-5, -5] },
    { id: "b", d: [-23, -7] },
  ]);
});

test("distribute keeps the outer units and evens centres or gaps", () => {
  const u = (id: string, x0: number, w: number) => ({ id, box: { x0, y0: 0, x1: x0 + w, y1: 1 } });
  const units = [u("a", 0, 10), u("c", 90, 10), u("b", 20, 2)];
  assert.deepEqual(distributeMoves(units, "x", "centers"), [{ id: "b", d: [29, 0] }]);
  // Gaps: span 0..100, sizes 22, gap 39: b starts at 49.
  assert.deepEqual(distributeMoves(units, "x", "gaps"), [{ id: "b", d: [29, 0] }]);
  const units2 = [u("a", 0, 10), u("b", 20, 30), u("c", 90, 10)];
  assert.deepEqual(distributeMoves(units2, "x", "gaps"), [{ id: "b", d: [15, 0] }]);
  assert.deepEqual(distributeMoves(units.slice(0, 2), "x", "gaps"), []);
});

test("rounding and typed maths", () => {
  assert.equal(roundMm(3.4930000000000003), 3.493);
  assert.equal(roundMm(-0.0000001), 0);
  assert.equal(evalNumber("12.5"), 12.5);
  assert.equal(evalNumber("10+2*3"), 16);
  assert.equal(evalNumber("(10+2)/4"), 3);
  assert.equal(evalNumber("-3 - -2"), -1);
  assert.equal(evalNumber("1,5"), 1.5);
  assert.equal(evalNumber("1e2"), 100);
  assert.equal(evalNumber("2*(3"), null);
  assert.equal(evalNumber("alert(1)"), null);
  assert.equal(evalNumber("1/0"), null);
  assert.equal(evalNumber(""), null);
});
