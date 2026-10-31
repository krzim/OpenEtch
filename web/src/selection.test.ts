// Unit tests for selection.ts.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument } from "./api/types.ts";
import { EMPTY_SELECTION, clickObject, pruneSelection } from "./selection.ts";

const doc: GDocument = {
  revision: 3,
  name: "t",
  stock: null,
  layers: [{ id: "layer-1", name: "L1", color: "#000", visible: true, output: true, settings: {} as never, sub_layers: [] }],
  objects: [
    { id: "obj-1", name: "a", layer: "layer-1", geometry: { kind: "ellipse", rx_mm: 1, ry_mm: 1 }, transform: [1, 0, 0, 1, 0, 0] },
    { id: "obj-2", name: "b", layer: "layer-1", geometry: { kind: "ellipse", rx_mm: 1, ry_mm: 1 }, transform: [1, 0, 0, 1, 0, 0] },
  ],
  next_id: 4,
};

test("click selects one object and its layer", () => {
  const s = clickObject(EMPTY_SELECTION, doc, "obj-1", false);
  assert.deepEqual(s, { objects: ["obj-1"], layer: "layer-1", stock: false });
});

test("additive click toggles", () => {
  let s = clickObject(EMPTY_SELECTION, doc, "obj-1", false);
  s = clickObject(s, doc, "obj-2", true);
  assert.deepEqual(s.objects, ["obj-1", "obj-2"]);
  s = clickObject(s, doc, "obj-1", true);
  assert.deepEqual(s.objects, ["obj-2"]);
});

test("prune drops deleted ids and keeps identity when unchanged", () => {
  const s = { objects: ["obj-1", "obj-9"], layer: "layer-1", stock: true };
  const p = pruneSelection(s, doc);
  assert.deepEqual(p, { objects: ["obj-1"], layer: "layer-1", stock: false });
  assert.equal(pruneSelection(p, doc), p);
});
