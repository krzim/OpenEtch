// Unit tests for geometry.ts. Run with `npm test` (Node 22's built-in test runner with type stripping; no extra deps).

import assert from "node:assert/strict";
import { test } from "node:test";
import type { DocObject, KurboPathEl } from "./api/types.ts";
import {
  apply,
  cmdsBounds,
  geometryCmds,
  mul,
  normDeg,
  objectBounds,
  parseBezPath,
  parseSvgPath,
  rotate,
  rotationDeg,
  toPathData,
  transformParts,
  translate,
} from "./geometry.ts";

const near = (a: number, b: number, eps = 1e-6) => assert.ok(Math.abs(a - b) < eps, `${a} != ${b}`);

test("affine apply matches kurbo/SVG matrix convention", () => {
  // [a,b,c,d,e,f]: x' = a x + c y + e, y' = b x + d y + f
  const p = apply([2, 0, 1, 3, 10, 20], [1, 1]);
  assert.deepEqual(p, [13, 23]);
});

test("mul applies the right operand first", () => {
  const a = mul(translate(5, 0), rotate(90));
  const p = apply(a, [1, 0]);
  near(p[0], 5);
  near(p[1], 1);
  near(rotationDeg(a), 90);
});

test("normDeg wraps into (-180, 180]", () => {
  near(normDeg(270), -90);
  near(normDeg(-180), 180);
  near(normDeg(540), 180);
});

test("rect object bounds follow the transform (mirrors the doc crate test)", () => {
  const obj: DocObject = {
    id: "obj-1",
    name: "r",
    layer: "layer-1",
    geometry: { kind: "rect", width_mm: 10, height_mm: 4, corner_radius_mm: 0 },
    transform: [1, 0, 0, 1, 5, -2],
  };
  const b = objectBounds(obj);
  assert.ok(b);
  near(b.x0, 0);
  near(b.x1, 10);
  near(b.y0, -4);
  near(b.y1, 0);
});

test("ellipse bounds are tight", () => {
  const b = cmdsBounds(geometryCmds({ kind: "ellipse", rx_mm: 5, ry_mm: 2 }));
  assert.ok(b);
  near(b.x0, -5, 1e-3);
  near(b.x1, 5, 1e-3);
  near(b.y0, -2, 1e-3);
  near(b.y1, 2, 1e-3);
});

test("kurbo element-array BezPath parses", () => {
  const els: KurboPathEl[] = [
    { MoveTo: { x: 0, y: 0 } },
    { LineTo: { x: 10, y: 0 } },
    { QuadTo: [{ x: 10, y: 10 }, { x: 0, y: 10 }] },
    "ClosePath",
  ];
  const cmds = parseBezPath(els);
  assert.equal(cmds.length, 4);
  assert.equal(toPathData(cmds), "M0,0 L10,0 Q10,10 0,10 Z");
});

test("kurbo to_svg string parses", () => {
  const cmds = parseBezPath("M0,0 L10,0 C10,5 5,10 0,10 Z");
  assert.equal(toPathData(cmds), "M0,0 L10,0 C10,5 5,10 0,10 Z");
});

test("relative and H/V commands parse", () => {
  const cmds = parseSvgPath("m1 1 h4 v2 l-4 0 z");
  assert.equal(toPathData(cmds), "M1,1 L5,1 L5,3 L1,3 Z");
});

test("malformed path data yields no NaN points", () => {
  const cmds = parseSvgPath("M0,0 L5");
  assert.ok(toPathData(cmds).indexOf("NaN") === -1);
});

test("Properties shows a flip apart from the rotation", () => {
  // Verify finding 7: a horizontally flipped object showed Rotation 180.
  assert.deepEqual(transformParts([-1, 0, 0, 1, 5, 0]), { rotationDeg: 0, flip: "horizontal" });
  assert.deepEqual(transformParts([1, 0, 0, -1, 0, 0]), { rotationDeg: 0, flip: "vertical" });
  // Not mirrored: the rotation as before.
  const r = transformParts(mul(translate(3, 4), rotate(30)));
  assert.equal(r.flip, "none");
  near(r.rotationDeg, 30);
  // Flipped horizontally, then rotated 30 degrees (with a scale): flip and 30.
  const fr = transformParts(mul(rotate(30), [-2, 0, 0, 2, 0, 0]));
  assert.equal(fr.flip, "horizontal");
  near(fr.rotationDeg, 30);
  // Flipped vertically, then rotated -45 degrees.
  const fv = transformParts(mul(rotate(-45), [1, 0, 0, -1, 0, 0]));
  assert.equal(fv.flip, "vertical");
  near(fv.rotationDeg, -45);
  // Rotating a flip by the difference reaches the typed angle (the Rotation field's edit).
  const typed = transformParts(mul(rotate(60), [-1, 0, 0, 1, 0, 0]));
  assert.equal(typed.flip, "horizontal");
  near(typed.rotationDeg, 60);
});
