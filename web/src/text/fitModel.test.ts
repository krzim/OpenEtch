// Text auto-fit in the UI (fitModel.ts): the area by alignment, its outline on the canvas, the text.update fields and
// Fit to stock.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { DocObject, Stock, TextGeometry } from "../api/types.ts";
import { fitFields, fitOutline, fitRect, fitToStock, startSize } from "./fitModel.ts";

const box = { shape: "box" as const, width_mm: 40, height_mm: 10, mode: "contain" as const };

function text(fit: TextGeometry["fit"], at: [number, number] = [0, 0]) {
  const geometry = { kind: "text", text: "Ada", fit, align_h: "middle", align_v: "middle" } as unknown as TextGeometry;
  return { id: "t1", name: "Ada", layer: "l", geometry, transform: [1, 0, 0, 1, at[0], at[1]] } as DocObject & { geometry: TextGeometry };
}

test("the box sits on the origin by the alignment, as the engine places it", () => {
  assert.deepEqual(fitRect({ align_h: "middle", align_v: "middle" }, box), [-20, -5, 20, 5]);
  assert.deepEqual(fitRect({ align_h: "left", align_v: "top" }, box), [0, -10, 40, 0]);
  assert.deepEqual(fitRect({ align_h: "right", align_v: "bottom" }, box), [-40, 0, 0, 10]);
  assert.deepEqual(fitRect({ align_h: "left", align_v: "top" }, { ...box, shape: "circle", width_mm: 30 }), [-15, -15, 15, 15], "a circle is centred");
});

test("the outline follows the text's transform", () => {
  const o = text(box, [5, 7]);
  assert.deepEqual(fitOutline(o), [
    [-15, 2],
    [25, 2],
    [25, 12],
    [-15, 12],
  ]);
  const c = fitOutline(text({ ...box, shape: "circle", width_mm: 20 }))!;
  assert.equal(c.length, 48);
  assert.ok(c.every(([x, y]) => Math.abs(Math.hypot(x, y) - 10) < 1e-9));
  assert.equal(fitOutline(text(null)), null);
});

test("fields: off, a box needs both sizes, a circle never stretches", () => {
  assert.deepEqual(fitFields("none", null, null, "contain"), { shape: "none" });
  assert.equal(fitFields("box", 10, null, "contain"), null);
  assert.equal(fitFields("box", 0, 5, "contain"), null);
  assert.deepEqual(fitFields("box", 10, 5, "stretch"), { shape: "box", width_mm: 10, height_mm: 5, mode: "stretch" });
  assert.deepEqual(fitFields("circle", 30, null, "stretch"), { shape: "circle", width_mm: 30, mode: "contain" });
  assert.deepEqual(startSize({ w: 12.2, h: 3.1 }), { w: 13, h: 4 });
  assert.deepEqual(startSize(null), { w: 30, h: 10 });
});

test("fit to stock centres the text and fits 80 % of a disc or a rectangle in one batch", () => {
  const disc: Stock = { name: "coin", shape: { kind: "circle", diameter_mm: 40 }, thickness_mm: 1, material: null, center: [10, -5], rotation_deg: 0 };
  const inv = fitToStock(text(null, [2, 3]), disc, "shrink");
  assert.deepEqual(inv, [
    { id: "text.update", params: { id: "t1", fit: { shape: "circle", width_mm: 32, mode: "shrink" }, align_h: "middle", align_v: "middle" } },
    { id: "object.transform", params: { ids: ["t1"], translate: [8, -8] } },
  ]);
  const tag: Stock = { ...disc, shape: { kind: "rect", width_mm: 50, height_mm: 20, corner_radius_mm: 2 }, center: [2, 3] };
  const r = fitToStock(text(null, [2, 3]), tag, "stretch");
  assert.equal(r.length, 1, "already centred: no move");
  assert.deepEqual(r[0].params.fit, { shape: "box", width_mm: 40, height_mm: 16, mode: "stretch" });
});
