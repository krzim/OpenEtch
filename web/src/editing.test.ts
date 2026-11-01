// Unit tests for editing.ts: nudge and rotate-key steps, Shift constraint, scale/rotate handles (and the locked-circle
// uniform scale), align/distribute moves, micrometre rounding and typed maths.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  alignMoves,
  constrainDelta,
  constraintLine,
  constraintSegment,
  distributeMoves,
  evalNumber,
  handlePoint,
  keepRound,
  lineAcrossRect,
  mergesNudge,
  nudgeDelta,
  nudgeKeyId,
  nudgeStep,
  queueNudge,
  rotateAbout,
  rotateFromHandle,
  rotateKeyDeg,
  roundMm,
  scaleAbout,
  scaleFromHandle,
  signedScaleParams,
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

test("the Shift guide follows the locked axis: horizontal, vertical or the active 45 degree line", () => {
  const p: [number, number] = [3, 4];
  assert.deepEqual(constraintLine(p, constrainDelta([-10, 2])), { p: [3, 4], dir: [1, 0] }, "left or right shows the X axis");
  assert.deepEqual(constraintLine(p, constrainDelta([1, -9]))?.dir, [0, 1], "up or down shows the Y axis");
  const r = Math.SQRT1_2;
  const up = constraintLine(p, constrainDelta([10, 8]))?.dir ?? [0, 0];
  assert.ok(Math.abs(up[0] - r) < 1e-12 && Math.abs(up[1] - r) < 1e-12, "up-right drag shows the +45 degree line");
  const down = constraintLine(p, constrainDelta([-10, 8]))?.dir ?? [0, 0];
  assert.ok(Math.abs(down[0] - r) < 1e-12 && Math.abs(down[1] + r) < 1e-12, "up-left drag shows the -45 degree line");
  assert.equal(constraintLine(p, [0, 0]), null, "no guide before the drag has a direction");
  // A 15 degree step from constrainAngle (drawTools.ts) gives that angle's line.
  const a = (30 * Math.PI) / 180;
  const l = constraintLine(p, [-2 * Math.cos(a), -2 * Math.sin(a)])?.dir ?? [0, 0];
  assert.ok(Math.abs(l[0] - Math.cos(a)) < 1e-12 && Math.abs(l[1] - Math.sin(a)) < 1e-12);
});

test("the Shift guide spans the visible canvas", () => {
  assert.deepEqual(lineAcrossRect([30, 20], [1, 0], 100, 50), [[0, 20], [100, 20]]);
  assert.deepEqual(lineAcrossRect([30, 20], [0, -2], 100, 50), [[30, 50], [30, 0]]);
  assert.deepEqual(lineAcrossRect([10, 10], [1, 1], 100, 50), [[0, 0], [50, 50]], "a diagonal stops at the nearer edges");
  assert.equal(lineAcrossRect([30, 80], [1, 0], 100, 50), null, "off-screen horizontal line");
  assert.equal(lineAcrossRect([200, 0], [1, 1], 100, 50), null, "diagonal that misses the canvas");
  assert.equal(lineAcrossRect([30, 20], [0, 0], 100, 50), null);
  // Field +Y is up and screen +Y down: a +45 degree field line runs from bottom-left to top-right on screen.
  const toScreen = (q: [number, number]): [number, number] => [(q[0] - 0) * 2 + 50, -(q[1] - 0) * 2 + 25];
  const line = constraintLine([0, 0], [5, 5]);
  assert.ok(line);
  const seg = constraintSegment(line, toScreen, [100, 50])?.flat() ?? [];
  [25, 50, 75, 0].forEach((v, i) => assert.ok(Math.abs((seg[i] ?? NaN) - v) < 1e-9, `end point coordinate ${i}`));
  // The start point may be off-screen: the guide still crosses the canvas when its line does.
  assert.deepEqual(constraintSegment({ p: [-100, 0], dir: [1, 0] }, toScreen, [100, 50]), [[0, 25], [100, 25]]);
});

test("a locked circle scales uniformly from any handle", () => {
  const e = keepRound(scaleFromHandle(box, "e", [20, 5], none), "e");
  assert.deepEqual([e.sx, e.sy, e.about], [2, 2, [0, 5]], "edge handle drives both axes about the opposite edge");
  const n = keepRound(scaleFromHandle(box, "n", [5, 5], none), "n");
  assert.deepEqual([n.sx, n.sy], [0.5, 0.5]);
  const c = keepRound(scaleFromHandle(box, "ne", [20, 12], { shift: true, ctrl: false }), "ne");
  assert.equal(c.sx, c.sy, "Shift cannot free the aspect of a locked circle");
  const flip = keepRound(scaleFromHandle(box, "e", [-10, 5], none), "e");
  assert.ok(flip.sx < 0 && flip.sy > 0, "past the opposite edge it mirrors and the other axis stays positive");
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
  // Dragging the left edge past the right one mirrors (verify finding 3: it used to collapse to 0.001x).
  s = scaleFromHandle(box, "w", [20, 5], none);
  assert.deepEqual(s, { sx: -1, sy: 1, about: [10, 5] });
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

test("a handle dragged past the opposite corner mirrors, and never collapses below 0.1 mm", () => {
  // Corner "ne" dragged to the far side of the fixed corner (0,0): both axes flip, aspect kept.
  let s = scaleFromHandle(box, "ne", [-20, -20], none);
  assert.deepEqual(s, { sx: -2, sy: -2, about: [0, 0] });
  // Shift frees the aspect: only X crosses over.
  s = scaleFromHandle(box, "ne", [-5, 20], { shift: true, ctrl: false });
  assert.equal(s.sx, -0.5);
  assert.equal(s.sy, 2);
  // Onto the fixed corner: kept at 0.1 mm of a 10 mm box (1%), not 0.001x (0.01 mm).
  s = scaleFromHandle(box, "ne", [0, 0], none);
  assert.equal(s.sx, 0.01);
  assert.equal(s.sy, 0.01);
  assert.ok((box.x1 - box.x0) * Math.abs(s.sx) >= 0.1 - 1e-12);
  // Just past it: the minimum keeps the flip's sign.
  s = scaleFromHandle(box, "e", [-0.0001, 3], none);
  assert.deepEqual(s, { sx: -0.01, sy: 1, about: [0, 5] });
  // A box already thinner than the minimum may not grow by itself (factor at most 1), and a zero-width line keeps 1.
  const thin = { x0: 0, y0: 0, x1: 0.05, y1: 10 };
  assert.equal(scaleFromHandle(thin, "e", [0, 5], none).sx, 1);
  assert.equal(scaleFromHandle({ x0: 0, y0: 0, x1: 0, y1: 10 }, "e", [5, 5], none).sx, 1);
  // Aspect-locked corners keep one factor even when the two sides need different minimums.
  const wide = { x0: 0, y0: 0, x1: 100, y1: 1 };
  s = scaleFromHandle(wide, "ne", [0, 0], none);
  assert.equal(s.sx, s.sy);
  assert.equal(s.sy, 0.1, "the 1 mm side keeps 0.1 mm");
});

test("signed scale factors become positive scale plus mirror (the engine refuses factors <= 0)", () => {
  assert.deepEqual(signedScaleParams(2, 3), { scale: [2, 3] });
  assert.deepEqual(signedScaleParams(-1, 1), { scale: [1, 1], mirror: "x" });
  assert.deepEqual(signedScaleParams(0.5, -2), { scale: [0.5, 2], mirror: "y" });
  assert.deepEqual(signedScaleParams(-2, -2), { scale: [2, 2], rotate_deg: 180 });
  // Each matches scaleAbout(sx, sy) about the origin: scale, then mirror, then rotate (engine order).
  const apply = (p: { scale: [number, number]; mirror?: "x" | "y"; rotate_deg?: number }): [number, number] => {
    let [x, y] = [p.scale[0], p.scale[1]];
    if (p.mirror === "x") x = -x;
    if (p.mirror === "y") y = -y;
    if (p.rotate_deg === 180) [x, y] = [-x, -y];
    return [x, y];
  };
  for (const [sx, sy] of [
    [-1.5, 2],
    [3, -0.25],
    [-0.5, -4],
  ] as [number, number][]) {
    assert.deepEqual(apply(signedScaleParams(sx, sy)), [sx, sy]);
  }
});

test("nudges merge into one undo step only for repeats of the same held key", () => {
  type Q = { target: string; key: string; d: [number, number] };
  const ctrlRight = nudgeKeyId("ArrowRight", { shift: false, ctrl: true });
  const shiftUp = nudgeKeyId("ArrowUp", { shift: true, ctrl: false });
  assert.notEqual(ctrlRight, shiftUp);
  assert.notEqual(ctrlRight, nudgeKeyId("ArrowRight", { shift: false, ctrl: false }), "modifiers count");

  // Verify finding 6: Ctrl+Right then Shift+Up while a command was in flight became one "move [1, 20]" step.
  let q: Q[] = [];
  queueNudge(q, { target: "obj-1", key: ctrlRight, d: [1, 0] }, false);
  queueNudge(q, { target: "obj-1", key: shiftUp, d: [0, 20] }, false);
  assert.deepEqual(q.map((n) => n.d), [[1, 0], [0, 20]], "two keys, two steps");

  // Two separate presses of the same key are two steps too (one undo step per press).
  q = [];
  queueNudge(q, { target: "obj-1", key: ctrlRight, d: [1, 0] }, false);
  queueNudge(q, { target: "obj-1", key: ctrlRight, d: [1, 0] }, false);
  assert.equal(q.length, 2);

  // Holding the key: its auto-repeats merge into the waiting nudge.
  queueNudge(q, { target: "obj-1", key: ctrlRight, d: [1, 0] }, true);
  queueNudge(q, { target: "obj-1", key: ctrlRight, d: [1, 0] }, true);
  assert.deepEqual(q.map((n) => n.d), [[1, 0], [3, 0]]);

  // A repeat for another selection or another key does not merge.
  assert.equal(mergesNudge({ target: "obj-1", key: ctrlRight }, { target: "obj-2", key: ctrlRight, repeat: true }), false);
  assert.equal(mergesNudge({ target: "obj-1", key: ctrlRight }, { target: "obj-1", key: shiftUp, repeat: true }), false);
  assert.equal(mergesNudge(null, { target: "obj-1", key: ctrlRight, repeat: true }), false);
});
