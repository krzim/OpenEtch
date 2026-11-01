// Node editing model for the Edit Nodes mode (LightBurn's Ctrl+`): the `node.list` answer parsed into typed nodes,
// segments and subpaths (field mm), path data drawn from it, hit testing of nodes, handles and segments, and the local
// preview of a drag (nodes moved with their handles, a handle moved with its smooth partner turning along), which the
// engine replaces with the real result after node.move. Pure; tested in nodes.test.ts.
//
// Numbering follows crates/geom/src/nodes.rs: nodes across all subpaths in order; a closed subpath of n segments has n
// nodes, an open one n + 1; node k of a subpath starts its segment k. Segments are numbered the same way.

import type { Vec2 } from "./api/types.ts";
import type { Box } from "./geometry.ts";

export type NodeType = "corner" | "smooth" | "symmetric";

export interface NodeInfo {
  index: number;
  subpath: number;
  point: Vec2;
  type: NodeType;
  /** Second control point of the incoming curve; null when that side is a line or absent. */
  handle_in: Vec2 | null;
  /** First control point of the outgoing curve. */
  handle_out: Vec2 | null;
  segment_in: number | null;
  segment_out: number | null;
  /** First or last node of an open subpath. */
  endpoint: boolean;
}

export interface SegInfo {
  index: number;
  subpath: number;
  type: "line" | "curve";
  from: number;
  to: number;
}

export interface SubInfo {
  index: number;
  closed: boolean;
  first_node: number;
  node_count: number;
}

export interface NodeList {
  id: string;
  /** Geometry kind: "path", "rect", "ellipse". */
  kind: string;
  /** A rectangle or ellipse: the first node edit converts it to a path. */
  converts_on_edit: boolean;
  nodes: NodeInfo[];
  segments: SegInfo[];
  subpaths: SubInfo[];
}

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const vec = (v: unknown): Vec2 | null => (Array.isArray(v) && v.length === 2 && isNum(v[0]) && isNum(v[1]) ? [v[0], v[1]] : null);
const intOrNull = (v: unknown): number | null => (isNum(v) ? v : null);

/** Parse a `node.list` result; null when it is not one. */
export function parseNodeList(r: unknown): NodeList | null {
  if (!r || typeof r !== "object") return null;
  const o = r as Record<string, unknown>;
  if (typeof o.id !== "string" || !Array.isArray(o.nodes) || !Array.isArray(o.segments)) return null;
  const nodes: NodeInfo[] = [];
  for (const n of o.nodes as Record<string, unknown>[]) {
    const p = vec(n?.point);
    if (!p || !isNum(n.index)) return null;
    const t = n.type === "smooth" || n.type === "symmetric" ? n.type : "corner";
    nodes.push({
      index: n.index,
      subpath: isNum(n.subpath) ? n.subpath : 0,
      point: p,
      type: t,
      handle_in: vec(n.handle_in),
      handle_out: vec(n.handle_out),
      segment_in: intOrNull(n.segment_in),
      segment_out: intOrNull(n.segment_out),
      endpoint: n.endpoint === true,
    });
  }
  const segments: SegInfo[] = [];
  for (const s of o.segments as Record<string, unknown>[]) {
    if (!isNum(s?.index) || !isNum(s.from) || !isNum(s.to)) return null;
    segments.push({ index: s.index, subpath: isNum(s.subpath) ? s.subpath : 0, type: s.type === "curve" ? "curve" : "line", from: s.from, to: s.to });
  }
  const subpaths: SubInfo[] = [];
  for (const s of (Array.isArray(o.subpaths) ? o.subpaths : []) as Record<string, unknown>[]) {
    if (!isNum(s?.index) || !isNum(s.first_node) || !isNum(s.node_count)) continue;
    subpaths.push({ index: s.index, closed: s.closed === true, first_node: s.first_node, node_count: s.node_count });
  }
  return {
    id: o.id,
    kind: typeof o.kind === "string" ? o.kind : "path",
    converts_on_edit: o.converts_on_edit === true,
    nodes,
    segments,
    subpaths,
  };
}

/** A segment's control polygon in field mm: [p0, p1, p2, p3]; a line has p1 = p0 and p2 = p3. */
export function segmentCtrl(nl: NodeList, s: SegInfo): [Vec2, Vec2, Vec2, Vec2] {
  const a = nl.nodes[s.from]?.point ?? [0, 0];
  const b = nl.nodes[s.to]?.point ?? [0, 0];
  if (s.type === "line") return [a, a, b, b];
  return [a, nl.nodes[s.from]?.handle_out ?? a, nl.nodes[s.to]?.handle_in ?? b, b];
}

/** Point at parameter t of a cubic. */
export function cubicAt(c: readonly [Vec2, Vec2, Vec2, Vec2], t: number): Vec2 {
  const u = 1 - t;
  const a = u * u * u;
  const b = 3 * u * u * t;
  const d = 3 * u * t * t;
  const e = t * t * t;
  return [a * c[0][0] + b * c[1][0] + d * c[2][0] + e * c[3][0], a * c[0][1] + b * c[1][1] + d * c[2][1] + e * c[3][1]];
}

const r4 = (v: number) => Math.round(v * 1e4) / 1e4;
const pt = (p: Vec2) => `${r4(p[0])},${r4(p[1])}`;

/** SVG path data (field mm) of the node list: every subpath, closed ones with Z. */
export function nodePathData(nl: NodeList): string {
  let d = "";
  const bySub = new Map<number, SegInfo[]>();
  for (const s of nl.segments) {
    const list = bySub.get(s.subpath) ?? [];
    list.push(s);
    bySub.set(s.subpath, list);
  }
  const subs = nl.subpaths.length ? nl.subpaths : [...bySub.keys()].map((index) => ({ index, closed: false, first_node: 0, node_count: 0 }));
  for (const sub of subs) {
    const segs = bySub.get(sub.index) ?? [];
    if (!segs.length) continue;
    d += `M${pt(nl.nodes[segs[0].from]?.point ?? [0, 0])}`;
    for (const s of segs) {
      const c = segmentCtrl(nl, s);
      d += s.type === "line" ? `L${pt(c[3])}` : `C${pt(c[1])} ${pt(c[2])} ${pt(c[3])}`;
    }
    if (sub.closed) d += "Z";
  }
  return d;
}

/** SVG path data of one segment. */
export function segmentPathData(nl: NodeList, s: SegInfo): string {
  const c = segmentCtrl(nl, s);
  return s.type === "line" ? `M${pt(c[0])}L${pt(c[3])}` : `M${pt(c[0])}C${pt(c[1])} ${pt(c[2])} ${pt(c[3])}`;
}

const add = (a: Vec2, d: Vec2): Vec2 => [a[0] + d[0], a[1] + d[1]];
const sub = (a: Vec2, b: Vec2): Vec2 => [a[0] - b[0], a[1] - b[1]];
const dist = (a: Vec2, b: Vec2) => Math.hypot(a[0] - b[0], a[1] - b[1]);

/** The node list with nodes `indices` moved by `d`, their handles moving along (node.move's behaviour). */
export function moveNodes(nl: NodeList, indices: ReadonlySet<number>, d: Vec2): NodeList {
  return {
    ...nl,
    nodes: nl.nodes.map((n) =>
      indices.has(n.index)
        ? { ...n, point: add(n.point, d), handle_in: n.handle_in && add(n.handle_in, d), handle_out: n.handle_out && add(n.handle_out, d) }
        : n,
    ),
  };
}

/**
 * The node list with one handle of node `index` moved to `to`. On a smooth node the other handle turns to stay
 * opposite (keeping its length); on a symmetric node it also takes the same length, as `NodePath::move_handle` does.
 */
export function moveHandle(nl: NodeList, index: number, side: "in" | "out", to: Vec2): NodeList {
  return {
    ...nl,
    nodes: nl.nodes.map((n) => {
      if (n.index !== index) return n;
      const mine = side === "in" ? "handle_in" : "handle_out";
      const other = side === "in" ? "handle_out" : "handle_in";
      if (!n[mine]) return n;
      const next: NodeInfo = { ...n, [mine]: to };
      const o = n[other];
      if (n.type !== "corner" && o) {
        const dir = sub(n.point, to);
        const len = Math.hypot(dir[0], dir[1]);
        if (len > 1e-12) {
          const l = n.type === "symmetric" ? len : dist(o, n.point);
          next[other] = [n.point[0] + (dir[0] / len) * l, n.point[1] + (dir[1] / len) * l];
        }
      }
      return next;
    }),
  };
}

/** Nearest point on segment `s` to `p`: Bézier parameter t, the point and its distance. */
export function nearestOnSegment(nl: NodeList, s: SegInfo, p: Vec2): { t: number; p: Vec2; d: number } {
  const c = segmentCtrl(nl, s);
  if (s.type === "line") {
    const a = c[0];
    const b = c[3];
    const vx = b[0] - a[0];
    const vy = b[1] - a[1];
    const l2 = vx * vx + vy * vy;
    const t = l2 > 0 ? Math.min(1, Math.max(0, ((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / l2)) : 0;
    const q: Vec2 = [a[0] + vx * t, a[1] + vy * t];
    return { t, p: q, d: dist(p, q) };
  }
  // Sample, then refine by halving the bracket around the best sample.
  const N = 48;
  let bt = 0;
  let bd = Infinity;
  for (let i = 0; i <= N; i++) {
    const t = i / N;
    const d = dist(cubicAt(c, t), p);
    if (d < bd) {
      bd = d;
      bt = t;
    }
  }
  let step = 1 / N;
  for (let k = 0; k < 24; k++) {
    step /= 2;
    for (const t of [bt - step, bt + step]) {
      if (t < 0 || t > 1) continue;
      const d = dist(cubicAt(c, t), p);
      if (d < bd) {
        bd = d;
        bt = t;
      }
    }
  }
  return { t: bt, p: cubicAt(c, bt), d: bd };
}

export type NodeHit =
  | { kind: "node"; index: number }
  | { kind: "handle"; index: number; side: "in" | "out" }
  | { kind: "segment"; index: number; t: number; p: Vec2 };

/** Whether a node's handles are shown: the node or a neighbour across a segment is selected (LightBurn-like). */
export function handlesShown(nl: NodeList, selected: ReadonlySet<number>): Set<number> {
  const out = new Set<number>();
  for (const i of selected) {
    out.add(i);
    for (const s of nl.segments) {
      if (s.from === i) out.add(s.to);
      if (s.to === i) out.add(s.from);
    }
  }
  return out;
}

/**
 * What is under field point `p` within `tol` mm: a shown handle first (it sits on top), then a node, then a segment.
 * Handles are those of `shown` nodes.
 */
export function hitNodes(nl: NodeList, p: Vec2, tol: number, shown: ReadonlySet<number>): NodeHit | null {
  let best: { d: number; h: NodeHit } | null = null;
  const take = (d: number, h: NodeHit) => {
    if (d <= tol && (!best || d < best.d)) best = { d, h };
  };
  for (const n of nl.nodes) {
    if (!shown.has(n.index)) continue;
    if (n.handle_in) take(dist(n.handle_in, p), { kind: "handle", index: n.index, side: "in" });
    if (n.handle_out) take(dist(n.handle_out, p), { kind: "handle", index: n.index, side: "out" });
  }
  if (best) return (best as { h: NodeHit }).h;
  for (const n of nl.nodes) take(dist(n.point, p), { kind: "node", index: n.index });
  if (best) return (best as { h: NodeHit }).h;
  for (const s of nl.segments) {
    const q = nearestOnSegment(nl, s, p);
    // Not at the very ends: those are the nodes.
    if (q.t > 1e-3 && q.t < 1 - 1e-3) take(q.d, { kind: "segment", index: s.index, t: q.t, p: q.p });
  }
  return best ? (best as { h: NodeHit }).h : null;
}

/** Node indices inside a box (marquee selection). */
export function nodesInBox(nl: NodeList, b: Box): number[] {
  return nl.nodes.filter((n) => n.point[0] >= b.x0 && n.point[0] <= b.x1 && n.point[1] >= b.y0 && n.point[1] <= b.y1).map((n) => n.index);
}

/** The two end nodes to join, when exactly two end nodes of open subpaths are selected; else an explanation. */
export function joinPair(nl: NodeList, selected: ReadonlySet<number>): [number, number] | string {
  const ends = [...selected].filter((i) => nl.nodes[i]?.endpoint);
  if (ends.length !== 2 || selected.size !== 2) return "Select exactly two end nodes of open paths to join them.";
  return [ends[0], ends[1]];
}

/** Segments whose both end nodes are selected (L / K act on these when no segment is hovered). */
export function segmentsBetween(nl: NodeList, selected: ReadonlySet<number>): number[] {
  return nl.segments.filter((s) => selected.has(s.from) && selected.has(s.to)).map((s) => s.index);
}

/** Node selection after a click on node `i`: Shift toggles it, a plain click selects it alone unless already selected. */
export function clickNode(selected: ReadonlySet<number>, i: number, shift: boolean): Set<number> {
  if (shift) {
    const next = new Set(selected);
    if (next.has(i)) next.delete(i);
    else next.add(i);
    return next;
  }
  return selected.has(i) ? new Set(selected) : new Set([i]);
}
