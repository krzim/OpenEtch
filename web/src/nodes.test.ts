// Unit tests for nodes.ts: parsing node.list, path data, drag previews (nodes with their handles, smooth and symmetric
// handle partners), segment hit testing and parameters, marquee, join pairs and click selection.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  clickNode,
  cubicAt,
  handlesShown,
  hitNodes,
  joinPair,
  moveHandle,
  moveNodes,
  nearestOnSegment,
  nodePathData,
  nodesInBox,
  parseNodeList,
  segmentsBetween,
  type NodeList,
} from "./nodes.ts";

/** A closed 10 mm square with node 0 at the origin, as node.list reports it. */
function square(): NodeList {
  const nl = parseNodeList({
    id: "obj-1",
    kind: "path",
    converts_on_edit: false,
    nodes: [
      { index: 0, subpath: 0, point: [0, 0], type: "corner", handle_in: null, handle_out: null, segment_in: 3, segment_out: 0, endpoint: false },
      { index: 1, subpath: 0, point: [10, 0], type: "corner", handle_in: null, handle_out: null, segment_in: 0, segment_out: 1, endpoint: false },
      { index: 2, subpath: 0, point: [10, 10], type: "corner", handle_in: null, handle_out: null, segment_in: 1, segment_out: 2, endpoint: false },
      { index: 3, subpath: 0, point: [0, 10], type: "corner", handle_in: null, handle_out: null, segment_in: 2, segment_out: 3, endpoint: false },
    ],
    segments: [
      { index: 0, subpath: 0, type: "line", from: 0, to: 1 },
      { index: 1, subpath: 0, type: "line", from: 1, to: 2 },
      { index: 2, subpath: 0, type: "line", from: 2, to: 3 },
      { index: 3, subpath: 0, type: "line", from: 3, to: 0 },
    ],
    subpaths: [{ index: 0, closed: true, first_node: 0, node_count: 4 }],
  });
  assert.ok(nl);
  return nl;
}

/** An open S-curve: node 1 is smooth with handles on both sides. */
function curve(type: "smooth" | "symmetric" = "smooth"): NodeList {
  const nl = parseNodeList({
    id: "obj-2",
    kind: "path",
    converts_on_edit: false,
    nodes: [
      { index: 0, subpath: 0, point: [0, 0], type: "corner", handle_in: null, handle_out: [0, 5], segment_in: null, segment_out: 0, endpoint: true },
      { index: 1, subpath: 0, point: [10, 0], type, handle_in: [5, 0], handle_out: [12, 0], segment_in: 0, segment_out: 1, endpoint: false },
      { index: 2, subpath: 0, point: [20, 0], type: "corner", handle_in: [20, -5], handle_out: null, segment_in: 1, segment_out: null, endpoint: true },
    ],
    segments: [
      { index: 0, subpath: 0, type: "curve", from: 0, to: 1 },
      { index: 1, subpath: 0, type: "curve", from: 1, to: 2 },
    ],
    subpaths: [{ index: 0, closed: false, first_node: 0, node_count: 3 }],
  });
  assert.ok(nl);
  return nl;
}

test("parseNodeList refuses what is not a node list and keeps the conversion flag", () => {
  assert.equal(parseNodeList(null), null);
  assert.equal(parseNodeList({ id: "x" }), null);
  assert.equal(parseNodeList({ id: "x", nodes: [{ index: 0 }], segments: [] }), null);
  const r = parseNodeList({ id: "r", kind: "rect", converts_on_edit: true, nodes: [], segments: [], subpaths: [] });
  assert.equal(r?.converts_on_edit, true);
  assert.equal(r?.kind, "rect");
});

test("path data draws lines and curves, closing closed subpaths", () => {
  assert.equal(nodePathData(square()), "M0,0L10,0L10,10L0,10L0,0Z");
  assert.equal(nodePathData(curve()), "M0,0C0,5 5,0 10,0C12,0 20,-5 20,0");
});

test("moving nodes carries their handles along", () => {
  const m = moveNodes(curve(), new Set([1]), [1, 2]);
  assert.deepEqual(m.nodes[1].point, [11, 2]);
  assert.deepEqual(m.nodes[1].handle_in, [6, 2]);
  assert.deepEqual(m.nodes[1].handle_out, [13, 2]);
  assert.deepEqual(m.nodes[0].point, [0, 0], "other nodes stay");
});

test("a smooth node's other handle turns opposite and keeps its length; a symmetric one also matches it", () => {
  const s = moveHandle(curve("smooth"), 1, "in", [10, -4]);
  assert.deepEqual(s.nodes[1].handle_in, [10, -4]);
  const o = s.nodes[1].handle_out as [number, number];
  assert.ok(Math.abs(o[0] - 10) < 1e-9 && Math.abs(o[1] - 2) < 1e-9, `out handle ${o}`);
  const y = moveHandle(curve("symmetric"), 1, "in", [10, -4]);
  const q = y.nodes[1].handle_out as [number, number];
  assert.ok(Math.abs(q[0] - 10) < 1e-9 && Math.abs(q[1] - 4) < 1e-9, `out handle ${q}`);
  const c = moveHandle(curve("smooth"), 0, "in", [3, 3]);
  assert.deepEqual(c.nodes[0], curve().nodes[0], "a missing handle is not created");
});

test("nearest point on a line and on a cubic segment", () => {
  const sq = square();
  const a = nearestOnSegment(sq, sq.segments[0], [2.5, 3]);
  assert.equal(a.t, 0.25);
  assert.deepEqual(a.p, [2.5, 0]);
  assert.equal(a.d, 3);
  const cv = curve();
  const ctrl: [[number, number], [number, number], [number, number], [number, number]] = [
    [0, 0],
    [0, 5],
    [5, 0],
    [10, 0],
  ];
  const on = cubicAt(ctrl, 0.37);
  const b = nearestOnSegment(cv, cv.segments[0], on);
  assert.ok(Math.abs(b.t - 0.37) < 1e-6, `t ${b.t}`);
  assert.ok(b.d < 1e-6);
});

test("hit testing prefers shown handles, then nodes, then segments", () => {
  const cv = curve();
  const none = new Set<number>();
  assert.deepEqual(hitNodes(cv, [10.2, 0.1], 0.5, none), { kind: "node", index: 1 });
  assert.equal(hitNodes(cv, [5, 0], 0.5, none)?.kind === "handle", false, "handles of unselected nodes are hidden");
  assert.deepEqual(hitNodes(cv, [5, 0.2], 0.5, handlesShown(cv, new Set([1]))), { kind: "handle", index: 1, side: "in" });
  const sq = square();
  const h = hitNodes(sq, [5, 0.3], 0.5, none);
  assert.equal(h?.kind, "segment");
  assert.equal(h?.kind === "segment" && h.index, 0);
  assert.equal(hitNodes(sq, [5, 5], 0.5, none), null);
});

test("handles show for selected nodes and their neighbours", () => {
  assert.deepEqual([...handlesShown(curve(), new Set([0]))].sort(), [0, 1]);
});

test("marquee, join pairs, segments between selected nodes and click selection", () => {
  const sq = square();
  assert.deepEqual(nodesInBox(sq, { x0: -1, y0: -1, x1: 11, y1: 1 }), [0, 1]);
  assert.deepEqual(segmentsBetween(sq, new Set([0, 1])), [0]);
  assert.deepEqual(segmentsBetween(sq, new Set([0, 3])), [3]);
  assert.equal(typeof joinPair(sq, new Set([0, 1])), "string", "closed paths have no ends");
  assert.deepEqual(joinPair(curve(), new Set([0, 2])), [0, 2]);
  assert.deepEqual([...clickNode(new Set([1, 2]), 2, false)], [1, 2], "a click on a selected node keeps the selection for a drag");
  assert.deepEqual([...clickNode(new Set([1]), 2, false)], [2]);
  assert.deepEqual([...clickNode(new Set([1, 2]), 2, true)], [1]);
  assert.deepEqual([...clickNode(new Set([1]), 2, true)].sort(), [1, 2]);
});
