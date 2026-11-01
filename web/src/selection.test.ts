// Unit tests for selection.ts: LightBurn's click and box modifiers, box direction, groups as units, Ctrl+A, invert,
// Tab cycling, hidden layers, the align reference and hit testing (line tolerance, inside closed shapes, holes).

import assert from "node:assert/strict";
import { test } from "node:test";
import type { DocObject, GDocument } from "./api/types.ts";
import {
  EMPTY_SELECTION,
  alignReference,
  applySelect,
  boxHits,
  buildShapeIndex,
  clickObject,
  cycleSelection,
  expandToUnits,
  hitTest,
  invertSelection,
  pruneSelection,
  selectAll,
  selectLayerObjects,
  selectOpFor,
  topGroup,
  unitsOf,
} from "./selection.ts";

const rect = (id: string, x: number, y: number, w = 10, h = 10, extra: Partial<DocObject> = {}): DocObject => ({
  id,
  name: id,
  layer: "layer-1",
  geometry: { kind: "rect", width_mm: w, height_mm: h, corner_radius_mm: 0 },
  transform: [1, 0, 0, 1, x, y],
  ...extra,
});

const layer = (id: string, visible = true) => ({ id, name: id, color: "#000", visible, output: true, settings: {} as never, sub_layers: [] });

const doc: GDocument = {
  revision: 3,
  name: "t",
  stock: null,
  layers: [layer("layer-1"), layer("layer-2")],
  objects: [rect("obj-1", 0, 0), rect("obj-2", 30, 0), rect("obj-3", 60, 0, 10, 10, { layer: "layer-2" })],
  next_id: 4,
};

// obj-a and obj-b in group g-inner, which is inside g-outer together with obj-c; obj-d is loose.
const gdoc: GDocument = {
  ...doc,
  groups: [
    { id: "g-outer", name: "outer" },
    { id: "g-inner", name: "inner", parent: "g-outer" },
  ],
  objects: [
    rect("obj-a", 0, 0, 10, 10, { group: "g-inner" }),
    rect("obj-b", 20, 0, 10, 10, { group: "g-inner" }),
    rect("obj-c", 40, 0, 10, 10, { group: "g-outer" }),
    rect("obj-d", 80, 0),
  ],
};

test("modifiers: Shift adds, Ctrl toggles, Ctrl+Shift removes, none replaces", () => {
  assert.equal(selectOpFor({ shift: false, ctrl: false }), "replace");
  assert.equal(selectOpFor({ shift: true, ctrl: false }), "add");
  assert.equal(selectOpFor({ shift: false, ctrl: true }), "toggle");
  assert.equal(selectOpFor({ shift: true, ctrl: true }), "remove");
});

test("click selects one object and its layer", () => {
  const s = clickObject(EMPTY_SELECTION, doc, "obj-1", "replace");
  assert.deepEqual(s.objects, ["obj-1"]);
  assert.equal(s.layer, "layer-1");
  assert.equal(s.stock, false);
});

test("add, toggle and remove", () => {
  let s = clickObject(EMPTY_SELECTION, doc, "obj-1", "replace");
  s = clickObject(s, doc, "obj-2", "add");
  assert.deepEqual(s.objects, ["obj-1", "obj-2"]);
  s = clickObject(s, doc, "obj-2", "add");
  assert.deepEqual(s.objects, ["obj-1", "obj-2"], "add never removes");
  s = clickObject(s, doc, "obj-1", "toggle");
  assert.deepEqual(s.objects, ["obj-2"]);
  s = clickObject(s, doc, "obj-1", "toggle");
  assert.deepEqual(s.objects, ["obj-2", "obj-1"]);
  s = clickObject(s, doc, "obj-3", "remove");
  assert.deepEqual(s.objects, ["obj-2", "obj-1"], "remove of an unselected object changes nothing");
  s = clickObject(s, doc, "obj-2", "remove");
  assert.deepEqual(s.objects, ["obj-1"]);
  // The boolean form used by the browser tree still works.
  assert.deepEqual(clickObject(s, doc, "obj-3", true).objects, ["obj-1", "obj-3"]);
});

test("groups select as their top-level unit", () => {
  assert.equal(topGroup(gdoc, "obj-a"), "g-outer");
  assert.equal(topGroup(gdoc, "obj-d"), null);
  const s = clickObject(EMPTY_SELECTION, gdoc, "obj-b", "replace");
  assert.deepEqual(s.objects, ["obj-a", "obj-b", "obj-c"]);
  assert.deepEqual(clickObject(s, gdoc, "obj-c", "toggle").objects, []);
  assert.deepEqual(expandToUnits(gdoc, ["obj-d", "obj-c"]), ["obj-d", "obj-a", "obj-b", "obj-c"]);
  assert.deepEqual(unitsOf(gdoc, ["obj-a", "obj-b", "obj-d"]), ["g-outer", "obj-d"]);
  // A cycle in parents does not hang.
  const cyc: GDocument = { ...gdoc, groups: [{ id: "x", name: "x", parent: "y" }, { id: "y", name: "y", parent: "x" }], objects: [rect("o", 0, 0, 1, 1, { group: "x" })] };
  assert.ok(["x", "y"].includes(topGroup(cyc, "o") ?? ""));
});

test("box selection: enclosing vs crossing, and groups need all or any member", () => {
  const idx = buildShapeIndex(doc);
  // obj-1 spans x -5..5, obj-2 25..35. A box from -6 to 30 encloses obj-1 and crosses obj-2.
  const box = { x0: -6, y0: -6, x1: 30, y1: 6 };
  assert.deepEqual(boxHits(doc, idx, box, false), ["obj-1"]);
  assert.deepEqual(boxHits(doc, idx, box, true), ["obj-1", "obj-2"]);
  // A crossing box inside a rectangle that touches none of its lines selects nothing.
  assert.deepEqual(boxHits(doc, idx, { x0: -2, y0: -2, x1: 2, y1: 2 }, true), []);
  const gidx = buildShapeIndex(gdoc);
  const part = { x0: -6, y0: -6, x1: 30, y1: 6 }; // encloses obj-a and obj-b only
  assert.deepEqual(boxHits(gdoc, gidx, part, false), []);
  assert.deepEqual(boxHits(gdoc, gidx, part, true), ["obj-a", "obj-b", "obj-c"]);
});

test("box results combine with modifiers", () => {
  let s = clickObject(EMPTY_SELECTION, doc, "obj-3", "replace");
  s = applySelect(s, doc, ["obj-1", "obj-2"], "add");
  assert.deepEqual(s.objects, ["obj-3", "obj-1", "obj-2"]);
  s = applySelect(s, doc, ["obj-1", "obj-2"], "remove");
  assert.deepEqual(s.objects, ["obj-3"]);
  s = applySelect(s, doc, ["obj-3", "obj-1"], "toggle");
  assert.deepEqual(s.objects, ["obj-1"]);
  s = applySelect(s, doc, [], "replace");
  assert.deepEqual(s.objects, []);
});

test("select all, invert, layer and Tab cycle skip hidden layers", () => {
  const hidden: GDocument = { ...doc, layers: [layer("layer-1"), layer("layer-2", false)] };
  assert.deepEqual(selectAll(hidden).objects, ["obj-1", "obj-2"]);
  const one = clickObject(EMPTY_SELECTION, hidden, "obj-1", "replace");
  assert.deepEqual(invertSelection(one, hidden).objects, ["obj-2"]);
  assert.deepEqual(selectLayerObjects(doc, "layer-2").objects, ["obj-3"]);
  let s = cycleSelection(EMPTY_SELECTION, hidden, 1);
  assert.deepEqual(s.objects, ["obj-1"]);
  s = cycleSelection(s, hidden, 1);
  assert.deepEqual(s.objects, ["obj-2"]);
  s = cycleSelection(s, hidden, 1);
  assert.deepEqual(s.objects, ["obj-1"], "wraps around");
  assert.deepEqual(cycleSelection(EMPTY_SELECTION, hidden, -1).objects, ["obj-2"]);
  assert.deepEqual(cycleSelection(EMPTY_SELECTION, gdoc, 1).objects, ["obj-a", "obj-b", "obj-c"]);
});

test("prune drops deleted ids and objects on hidden layers, and keeps identity when unchanged", () => {
  const s = { objects: ["obj-1", "obj-9", "obj-3"], layer: "layer-1", stock: true };
  const hidden: GDocument = { ...doc, layers: [layer("layer-1"), layer("layer-2", false)] };
  const p = pruneSelection(s, hidden);
  assert.deepEqual(p.objects, ["obj-1"]);
  assert.equal(p.stock, false);
  assert.equal(pruneSelection(p, hidden), p);
});

test("align reference: last added, first in draw order among a batch, or the Ctrl+click-twice pick", () => {
  let s = applySelect(EMPTY_SELECTION, doc, ["obj-2", "obj-1"], "replace");
  assert.equal(alignReference(s, doc), "obj-1", "first in draw order among those selected together");
  s = clickObject(s, doc, "obj-3", "add");
  assert.equal(alignReference(s, doc), "obj-3");
  // Ctrl+click twice on obj-2: off, then on again, so it is the last added.
  s = clickObject(s, doc, "obj-2", "toggle");
  s = clickObject(s, doc, "obj-2", "toggle");
  assert.equal(alignReference(s, doc), "obj-2");
  assert.equal(alignReference(EMPTY_SELECTION, doc), null);
});

test("hit test: line tolerance, inside closed shapes, smallest wins, holes do not count", () => {
  const ring: GDocument = {
    ...doc,
    objects: [
      rect("big", 0, 0, 40, 40),
      rect("small", 0, 0, 10, 10),
      {
        id: "donut",
        name: "donut",
        layer: "layer-1",
        // Outer 20x20 square at (100,0) with a 10x10 hole.
        geometry: { kind: "path", path: "M90,-10 L110,-10 L110,10 L90,10 Z M95,-5 L105,-5 L105,5 L95,5 Z" },
        transform: [1, 0, 0, 1, 0, 0],
      },
      { id: "open", name: "open", layer: "layer-1", geometry: { kind: "path", path: "M200,0 L220,0 L220,20" }, transform: [1, 0, 0, 1, 0, 0] },
    ],
  };
  const idx = buildShapeIndex(ring);
  assert.equal(hitTest(idx, [20.5, 0], 1), "big", "near the big square's edge");
  assert.equal(hitTest(idx, [1, 1], 0.5), "small", "inside both: the smaller one");
  assert.equal(hitTest(idx, [15, 15], 0.5), "big");
  assert.equal(hitTest(idx, [92, 0], 0.5), "donut", "in the ring");
  assert.equal(hitTest(idx, [100, 0], 0.5), null, "in the hole");
  assert.equal(hitTest(idx, [210, 10], 0.5), null, "inside an open path's hull does not count");
  assert.equal(hitTest(idx, [220.3, 10], 0.5), "open");
  assert.equal(hitTest(idx, [500, 500], 1), null);
});
