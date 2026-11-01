// Unit tests for geomCommands.ts: parameter shapes match the engine's input structs (optional keys left out when
// default) and edit results are read for the selection.

import assert from "node:assert/strict";
import { test } from "node:test";
import * as G from "./geomCommands.ts";

test("booleans, weld and cut shapes keep the selection order", () => {
  assert.deepEqual(G.pathBoolean(["a", "b"], "subtract"), { id: "path.boolean", params: { ids: ["a", "b"], op: "subtract" } });
  assert.deepEqual(G.weld(["a"]), { id: "path.weld", params: { ids: ["a"] } });
  assert.deepEqual(G.cutShapes(["a", "t"]).params, { ids: ["a", "t"] });
});

test("optional parameters are only sent when set", () => {
  assert.deepEqual(G.offset(["a"], { distance_mm: 1, join: undefined, direction: "both" }).params, { ids: ["a"], distance_mm: 1, direction: "both" });
  assert.deepEqual(G.autoJoin(["a"]).params, { ids: ["a"] });
  assert.deepEqual(G.closePaths(["a"]).params, { ids: ["a"] });
  assert.deepEqual(G.closePaths(["a"], { tolerance_mm: 0.5, mode: "move_ends" }).params, { ids: ["a"], tolerance_mm: 0.5, mode: "move_ends" });
  assert.deepEqual(G.deleteDuplicates(null).params, {});
  assert.deepEqual(G.deleteDuplicates(["a"], true).params, { ids: ["a"], segments: true });
  assert.deepEqual(G.fillet(["a"], 2).params, { ids: ["a"], radius_mm: 2 });
  assert.deepEqual(G.fillet(["a"], -1, [0, 2]).params, { ids: ["a"], radius_mm: -1, corners: [0, 2] });
  assert.deepEqual(G.trim("a", [1, 2]).params, { id: "a", at: [1, 2] });
  assert.deepEqual(G.nodeMove("a", 3, [1, 1]).params, { id: "a", index: 3, to: [1, 1] });
  assert.deepEqual(G.nodeMove("a", 3, [1, 1], "out").params, { id: "a", index: 3, to: [1, 1], handle: "out" });
  assert.deepEqual(G.nodeSetType("a", [1], "smooth").params, { id: "a", indices: [1], type: "smooth" });
  assert.deepEqual(G.nodeSegmentType("a", 0, "curve").params, { id: "a", segment: 0, type: "curve" });
});

test("generators", () => {
  assert.deepEqual(G.polygon({ sides: 6, radius_mm: 10, center: [0, 0], rotation_deg: 0, corner_radius_mm: 0, layer: null }).params, {
    sides: 6,
    radius_mm: 10,
    center: [0, 0],
  });
  assert.deepEqual(G.polygon({ sides: 5, radius_mm: 4, center: [1, 2], rotation_deg: 15, star_inner_ratio: 0.5, layer: "layer-1" }).params, {
    sides: 5,
    radius_mm: 4,
    center: [1, 2],
    rotation_deg: 15,
    star_inner_ratio: 0.5,
    layer: "layer-1",
  });
  assert.deepEqual(G.line([[0, 0], [1, 1]], false).params, { points: [[0, 0], [1, 1]] });
  assert.deepEqual(G.line([[0, 0], [1, 1], [1, 0]], true, "layer-2").params, { points: [[0, 0], [1, 1], [1, 0]], closed: true, layer: "layer-2" });
});

test("arrays, nesting and arrange", () => {
  assert.deepEqual(G.circularArray(["a"], { count: 4, center: [0, 0], angle_deg: 360, rotate_copies: true }).params, { ids: ["a"], count: 4, center: [0, 0] });
  assert.deepEqual(G.circularArray(["a"], { count: 3, center: [1, 1], angle_deg: 180, rotate_copies: false }).params, {
    ids: ["a"],
    count: 3,
    center: [1, 1],
    angle_deg: 180,
    rotate_copies: false,
  });
  assert.deepEqual(G.alongPath(["a"], "p", { count: 4 }).params, { ids: ["a"], path_id: "p", count: 4 });
  assert.deepEqual(G.alongPath(["a"], "p", { spacing_mm: 7, rotate_to_path: false }).params, { ids: ["a"], path_id: "p", spacing_mm: 7, rotate_to_path: false });
  assert.deepEqual(G.nest(["a"], "stock", { spacing_mm: 1, rotations: [0, 90] }).params, { ids: ["a"], region: "stock", spacing_mm: 1, rotations: [0, 90] });
  assert.deepEqual(G.order(["a"], "front").params, { ids: ["a"], to: "front" });
  assert.deepEqual(G.makeSame(["a", "b"], "both", true).params, { ids: ["a", "b"], dimension: "both" });
  assert.deepEqual(G.makeSame(["a", "b"], "width", true).params, { ids: ["a", "b"], dimension: "width", keep_aspect: true });
  assert.deepEqual(G.scaleTo(["a"], [0, 0], [1, 0], [2, 0]).params, { ids: ["a"], pivot: [0, 0], from: [1, 0], to: [2, 0] });
  assert.deepEqual(G.transformEx(["a"], { mirror_line: { a: [0, 0], b: [1, 1] } }).params, { ids: ["a"], mirror_line: { a: [0, 0], b: [1, 1] } });
});

test("result ids: created and affected, minus deleted, batches flattened", () => {
  assert.deepEqual(G.resultIds({ created: ["c"], affected: ["a", "b"], deleted: ["b"] }), ["c", "a"]);
  assert.deepEqual(G.resultIds({ results: [{ created: ["x"] }, { affected: ["x", "y"] }] }), ["x", "y"]);
  assert.deepEqual(G.resultIds(null), []);
});
