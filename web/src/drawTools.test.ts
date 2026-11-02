// Unit tests for drawTools.ts: 15° constraint, rectangle/ellipse drag modifiers, polygon drags and points, pen path
// data, measure readout and ruler ticks.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  cleanPoints,
  constrainAngle,
  dragCorners,
  measure,
  measureText,
  normalizeDeg,
  penHasCurves,
  penNode,
  penPathData,
  polygonFromDrag,
  polygonPoints,
  rulerTicks,
} from "./drawTools.ts";

const near = (a: number, b: number, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps, `${a} != ${b}`);

test("Shift constrains to 15° steps, keeping the distance along the constrained direction", () => {
  assert.deepEqual(constrainAngle([0, 0], [10, 1]), [10, 0]);
  assert.deepEqual(constrainAngle([0, 0], [0.5, -8]), [0, -8]);
  const p = constrainAngle([1, 1], [11, 11.5]);
  near(p[0] - 1, p[1] - 1);
  assert.deepEqual(constrainAngle([2, 3], [2, 3]), [2, 3]);
});

test("rectangle drags: corner to corner, Shift square, Ctrl from the centre", () => {
  const none = { shift: false, ctrl: false };
  assert.deepEqual(dragCorners([0, 0], [4, -2], none), { a: [0, 0], b: [4, -2] });
  assert.deepEqual(dragCorners([0, 0], [4, -2], { shift: true, ctrl: false }), { a: [0, 0], b: [4, -4] });
  assert.deepEqual(dragCorners([1, 1], [4, 3], { shift: false, ctrl: true }), { a: [-2, -1], b: [4, 3] });
  assert.deepEqual(dragCorners([0, 0], [-1, 3], { shift: true, ctrl: true }), { a: [3, -3], b: [-3, 3] });
});

test("polygon drags put the first corner under the pointer", () => {
  const up = polygonFromDrag([0, 0], [0, 10], false);
  assert.equal(up.radius, 10);
  assert.equal(up.rotationDeg, 0);
  const right = polygonFromDrag([0, 0], [5, 0], false);
  assert.equal(right.rotationDeg, -90);
  const snapped = polygonFromDrag([0, 0], [10, 0.9], true);
  assert.equal(snapped.rotationDeg, -90);
  const pts = polygonPoints([0, 0], 6, 10, up.rotationDeg, null);
  assert.equal(pts.length, 6);
  near(pts[0][0], 0);
  near(pts[0][1], 10);
  const star = polygonPoints([0, 0], 5, 10, 0, 0.5);
  assert.equal(star.length, 10);
  near(Math.hypot(star[1][0], star[1][1]), 5);
  assert.equal(normalizeDeg(270), -90);
  assert.equal(normalizeDeg(-180), 180);
});

test("pen paths: lines between plain nodes, cubics where a handle was dragged", () => {
  const a = { p: [0, 0] as [number, number] };
  const b = penNode([10, 0], [12, 3]);
  assert.deepEqual(b, { p: [10, 0], hout: [12, 3], hin: [8, -3] });
  const c = { p: [20, 0] as [number, number] };
  assert.equal(penPathData([a, c], false), "M0,0L20,0");
  assert.equal(penPathData([a, b, c], false), "M0,0C0,0 8,-3 10,0C12,3 20,0 20,0");
  assert.equal(penPathData([a, c, { p: [10, 10] }], true), "M0,0L20,0L10,10Z");
  assert.equal(penHasCurves([a, c], true), false);
  assert.equal(penHasCurves([a, b], false), true);
  assert.deepEqual(penNode([1, 1], [1, 1]), { p: [1, 1] }, "a press without a drag is a corner");
  assert.deepEqual(cleanPoints([[0, 0], [0, 0], [1, 1], [1, 1 + 1e-9]]), [[0, 0], [1, 1]]);
});

test("measure: distance, deltas, angle and text", () => {
  const m = measure([0, 0], [3, 4]);
  assert.equal(m.dist, 5);
  assert.equal(m.dx, 3);
  assert.equal(m.dy, 4);
  near(m.angleDeg, 53.130102, 1e-6);
  assert.equal(measure([0, 0], [-1, 0]).angleDeg, 180);
  assert.equal(measureText(m), "5.00 mm · ΔX 3.00 · ΔY 4.00 · 53.1°");
  assert.equal(measureText(measure([0, 0], [25.4, -50.8]), "in"), "2.236 in · ΔX 1.000 · ΔY -2.000 · -63.4°");
});

test("ruler ticks: labelled majors on round numbers, minors between them", () => {
  const t = rulerTicks(-12, 37, 3);
  const majors = t.filter((x) => x.major);
  assert.ok(majors.length >= 2);
  const step = majors[1].v - majors[0].v;
  assert.ok(step * 3 >= 60, "labels at least 60 px apart");
  assert.ok([20, 50].includes(step), `step ${step}`);
  assert.ok(majors.every((x) => x.label === String(x.v)));
  assert.ok(t.some((x) => x.v === 0 && x.major && x.label === "0"));
  const fine = rulerTicks(0, 1, 400);
  assert.ok(fine.filter((x) => x.major).every((x) => /^\d\.\d+$|^\d$/.test(x.label ?? "")), "fractional labels");
  assert.deepEqual(rulerTicks(0, 10, 0), []);
});

test("ruler ticks in inches: labels in inches on round inch values, positions in mm", () => {
  // 3 px/mm = 76.2 px/in: a label every inch.
  const t = rulerTicks(-30, 80, 3, 60, "in");
  const majors = t.filter((x) => x.major);
  assert.deepEqual(
    majors.map((x) => x.label),
    ["-1", "0", "1", "2", "3"],
  );
  assert.ok(majors.every((x) => Math.abs(x.v - Number(x.label) * 25.4) < 1e-9));
  assert.ok(t.filter((x) => !x.major).length > 0, "minor ticks between");
  // Zoomed in: tenths of an inch.
  const fine = rulerTicks(0, 10, 40, 60, "in");
  assert.ok(fine.filter((x) => x.major).some((x) => x.label === "0.2"));
});

// ---- point presses: commit on lift for touch, roll back on cancel (touch/toolPress.ts, audit T4) -------------------

import { beginPress, cancelPress, commitOn, endPress, movePress } from "./touch/toolPress.ts";

type Pt = [number, number];
const pushPt = (s: { pts: Pt[] }, q: Pt) => ({ pts: [...s.pts, q] });

test("commitOn: touch commits on lift; mouse and pen on the press, as LightBurn", () => {
  for (const tool of ["line", "measure", "pick", "trim"] as const) {
    assert.equal(commitOn(tool, "touch"), "up", tool);
    assert.equal(commitOn(tool, "mouse"), "down", tool);
    assert.equal(commitOn(tool, "pen"), "down", tool);
  }
});

test("line on touch: nothing on the press, dragging adjusts, the lift commits", () => {
  const s0 = { pts: [[0, 0]] as Pt[] };
  const b = beginPress(s0, [10, 0] as Pt, commitOn("line", "touch"), pushPt);
  assert.equal(b.state, s0, "no point yet");
  const moved = movePress(b.press, [12, 1] as Pt);
  assert.deepEqual(endPress(b.state, moved, pushPt).pts, [[0, 0], [12, 1]], "the point lands where the finger lifted");
});

test("line, measure and pick: cancel (a second finger) rolls back what the press did, mouse or touch", () => {
  const s0 = { pts: [[0, 0]] as Pt[] };
  // Mouse: applied on the press, rolled back by the cancel.
  const m = beginPress(s0, [5, 5] as Pt, commitOn("line", "mouse"), pushPt);
  assert.equal(m.state.pts.length, 2);
  assert.equal(cancelPress(m.state, m.press), s0);
  // Touch: never applied; the cancel leaves the state alone.
  const t = beginPress(s0, [5, 5] as Pt, commitOn("line", "touch"), pushPt);
  assert.equal(cancelPress(t.state, t.press), s0);

  // Measure: the first point, then the second.
  type M = { a: Pt; b: Pt; done: boolean } | null;
  const measureAt = (s: M, q: Pt): M => (s && !s.done ? { a: s.a, b: q, done: true } : { a: q, b: q, done: false });
  const first = beginPress<M, Pt>(null, [1, 1], "down", measureAt);
  assert.deepEqual(first.state, { a: [1, 1], b: [1, 1], done: false });
  assert.equal(cancelPress(first.state, first.press), null, "no stray measure start");
  const second = beginPress<M, Pt>(first.state, [4, 5], "up", measureAt);
  assert.deepEqual(endPress(second.state, second.press, measureAt), { a: [1, 1], b: [4, 5], done: true });

  // Pick: two points for a mirror line.
  const pick = { pts: [[0, 0]] as Pt[] };
  const pk = beginPress(pick, [3, 3] as Pt, "down", pushPt);
  assert.equal(pk.state.pts.length, 2);
  assert.equal(cancelPress(pk.state, pk.press), pick);
});

test("a mouse press keeps the point it pressed, whatever the pointer does next", () => {
  const s0 = { pts: [] as Pt[] };
  const m = beginPress(s0, [5, 5] as Pt, "down", pushPt);
  const p2 = movePress(m.press, [9, 9] as Pt);
  assert.equal(p2, m.press);
  assert.equal(endPress(m.state, p2, pushPt), m.state, "nothing more on release");
});
