// Fusion 360-style snapping on real geometry, computed in the web from the document: snap candidates per object
// (centre, quadrants, vertices/endpoints, segment midpoints), intersections between outlines near the cursor,
// nearest point on an outline, stock and field points, alignment inference (dotted horizontal/vertical guides) and the
// grid. Pure functions, no DOM, so `npm test` covers them. All values are field-frame mm (+Y away from the operator).
//
// Behaviour (after Fusion 360's sketch snapping, Help "Sketch snapping and inference"):
// - the tolerance is in screen pixels (the caller converts it to mm with the zoom);
// - point snaps win over everything else; among them the closest wins, with a small bias by kind (endpoint, then
//   intersection, centre, quadrant, midpoint), so two points at about the same distance resolve predictably;
// - then the nearest point on an outline, then alignment guides (both axes: the guides' crossing), then the grid;
// - a dragged selection snaps by one reference point: the selection's own snap point nearest to where it was grabbed.
//
// Performance: candidate points live in a uniform grid hash, outlines are flattened once into segment arrays chunked
// with chunk boxes, and intersection and nearest searches touch only segments in the query box, with hard caps.

import type { DocObject, Stock, Vec2 } from "./api/types.ts";
import {
  apply,
  boxCenter,
  boxesIntersect,
  cmdsBounds,
  flattenCmds,
  geometryCmds,
  stockCmds,
  type Box,
  type PathCmd,
  type Polyline,
} from "./geometry.ts";

/** What a snap lands on. Each kind has its own glyph in the canvas. */
export type SnapKind = "center" | "quadrant" | "endpoint" | "midpoint" | "intersection" | "nearest" | "guide" | "grid";

/** Default snap tolerance in screen pixels (Fusion uses about this much). */
export const SNAP_TOL_PX = 8;

/** A candidate snap point. */
export interface SnapPoint {
  p: Vec2;
  kind: SnapKind;
  /** The word shown in the label: "Center", "Quadrant", "Vertex", "Endpoint", "Midpoint", ... */
  what: string;
  /** What it belongs to: an object's display name, "stock", "stock edge", "field". */
  owner: string;
  /** Object id, "stock" or "field". */
  id: string;
}

/** "Center · Ellipse 2". */
export function snapLabel(s: Pick<SnapPoint, "what" | "owner">): string {
  return s.owner ? `${s.what} · ${s.owner}` : s.what;
}

/** Point snaps by priority (lower wins ties); see `findSnap`. */
export const PRIORITY: Record<SnapKind, number> = {
  endpoint: 0,
  intersection: 1,
  center: 2,
  quadrant: 3,
  midpoint: 4,
  nearest: 5,
  guide: 6,
  grid: 7,
};

// --- circles -----------------------------------------------------------------------------------------------------

/** Whether an object is drawn as a circle: an ellipse with equal radii under a transform without skew or stretch. */
export function isRound(o: Pick<DocObject, "geometry" | "transform">): boolean {
  const g = o.geometry;
  if (g.kind !== "ellipse") return false;
  if (Math.abs(g.rx_mm - g.ry_mm) > 1e-9 * Math.max(g.rx_mm, g.ry_mm, 1)) return false;
  const [a, b, c, d] = o.transform;
  const l1 = Math.hypot(a, b);
  const l2 = Math.hypot(c, d);
  return Math.abs(l1 - l2) <= 1e-6 * Math.max(l1, l2, 1e-12) && Math.abs(a * c + b * d) <= 1e-6 * l1 * l2;
}

/** Field-frame centre and diameter of a round object, or null. */
export function circleOf(o: Pick<DocObject, "geometry" | "transform">): { center: Vec2; diameter: number } | null {
  if (!isRound(o) || o.geometry.kind !== "ellipse") return null;
  const [a, b] = o.transform;
  return { center: [o.transform[4], o.transform[5]], diameter: 2 * o.geometry.rx_mm * Math.hypot(a, b) };
}

/** What kind of shape an object is, for labels: "Circle" for a round ellipse. */
export function shapeKindName(o: Pick<DocObject, "geometry" | "transform"> & { extras?: DocObject["extras"] }): string {
  switch (o.geometry.kind) {
    case "ellipse":
      return isRound(o) ? "Circle" : "Ellipse";
    case "rect":
      return "Rectangle";
    case "image":
      return "Image";
    case "text":
      return "Text";
    default:
      // A barcode (shape.barcode) is a path that keeps its parameters for editing.
      return o.extras?.["openetch.barcode"] ? "Barcode" : "Path";
  }
}

/** Browser-tree icon of an object: its shape kind (any path, drawn or converted, is "path"; never the Import icon). */
export function objectIconName(o: Pick<DocObject, "geometry" | "transform">): "circle" | "ellipse" | "rect" | "image" | "path" {
  switch (o.geometry.kind) {
    case "ellipse":
      return isRound(o) ? "circle" : "ellipse";
    case "rect":
      return "rect";
    case "image":
      return "image";
    default:
      return "path";
  }
}

/**
 * Display name of an object: its name, with a default "Ellipse"/"Ellipse 3" name read as "Circle"/"Circle 3" while it
 * is round; an unnamed object gets its kind and id.
 */
export function displayName(o: Pick<DocObject, "id" | "name" | "geometry" | "transform">): string {
  const name = o.name.trim();
  if (!name) return `${shapeKindName(o)} ${o.id}`;
  if (o.geometry.kind === "ellipse" && isRound(o)) return name.replace(/^Ellipse(?=$|\s)/, "Circle");
  return name;
}

// --- candidates ------------------------------------------------------------------------------------------------------

/** One outline flattened into segments [x0, y0, x1, y1, ...] in chunks with boxes, for nearest and intersections. */
export interface Outline {
  id: string;
  owner: string;
  box: Box;
  segs: Float64Array;
  /** Chunks of `CHUNK` segments: [x0, y0, x1, y1, first, end) in segment indices. */
  chunks: { box: Box; first: number; end: number }[];
}

/** Snap data of one thing in the document. */
export interface SnapPart {
  points: SnapPoint[];
  outline: Outline | null;
}

const CHUNK = 32;
/** At most this many vertex/midpoint candidates per object (a traced bitmap can have tens of thousands of nodes). */
export const MAX_POINTS_PER_OBJECT = 4000;
/** Curve flattening for snapping: segments per bezier (a 10 mm quarter circle deviates under 0.003 mm). */
const FLATTEN_N = 32;

/** Build an outline from polylines (null when there is nothing to draw). */
export function outlineOf(id: string, owner: string, polys: Polyline[]): Outline | null {
  let n = 0;
  for (const pl of polys) n += Math.max(0, pl.pts.length - 1) + (pl.closed && pl.pts.length > 2 ? 1 : 0);
  if (n === 0) return null;
  const segs = new Float64Array(n * 4);
  let k = 0;
  const push = (a: Vec2, b: Vec2) => {
    segs[k++] = a[0];
    segs[k++] = a[1];
    segs[k++] = b[0];
    segs[k++] = b[1];
  };
  for (const pl of polys) {
    for (let i = 1; i < pl.pts.length; i++) push(pl.pts[i - 1], pl.pts[i]);
    if (pl.closed && pl.pts.length > 2) push(pl.pts[pl.pts.length - 1], pl.pts[0]);
  }
  const chunks: Outline["chunks"] = [];
  for (let first = 0; first < n; first += CHUNK) {
    const end = Math.min(n, first + CHUNK);
    const b: Box = { x0: Infinity, y0: Infinity, x1: -Infinity, y1: -Infinity };
    for (let i = first; i < end; i++) {
      const o = i * 4;
      b.x0 = Math.min(b.x0, segs[o], segs[o + 2]);
      b.x1 = Math.max(b.x1, segs[o], segs[o + 2]);
      b.y0 = Math.min(b.y0, segs[o + 1], segs[o + 3]);
      b.y1 = Math.max(b.y1, segs[o + 1], segs[o + 3]);
    }
    chunks.push({ box: b, first, end });
  }
  const box: Box = {
    x0: Math.min(...chunks.map((c) => c.box.x0)),
    y0: Math.min(...chunks.map((c) => c.box.y0)),
    x1: Math.max(...chunks.map((c) => c.box.x1)),
    y1: Math.max(...chunks.map((c) => c.box.y1)),
  };
  return { id, owner, box, segs, chunks };
}

const same = (a: Vec2, b: Vec2) => Math.abs(a[0] - b[0]) <= 1e-9 && Math.abs(a[1] - b[1]) <= 1e-9;

/**
 * Vertices, endpoints and straight-segment midpoints of path commands under transform `a`. A node of a closed subpath
 * is a "Vertex"; the first and last node of an open subpath are "Endpoint"s and the nodes between them "Vertex"es.
 * Both use the endpoint glyph. Curves give their end nodes but no midpoint.
 */
export function pathPoints(cmds: PathCmd[], a: Parameters<typeof apply>[0], owner: string, id: string, max = MAX_POINTS_PER_OBJECT): SnapPoint[] {
  const nodes: SnapPoint[] = [];
  const mids: SnapPoint[] = [];
  // Split into subpaths first, so open ends can be told apart.
  type Sub = { pts: Vec2[]; straight: boolean[]; closed: boolean };
  const subs: Sub[] = [];
  let cur: Sub | null = null;
  for (const k of cmds) {
    if (k.c === "M") {
      cur = { pts: [k.p], straight: [], closed: false };
      subs.push(cur);
    } else if (k.c === "Z") {
      if (cur) cur.closed = true;
      cur = null;
    } else {
      if (!cur) continue;
      cur.pts.push(k.p);
      cur.straight.push(k.c === "L");
    }
  }
  for (const s of subs) {
    const pts = s.pts;
    const straight = [...s.straight];
    // A closed subpath whose last node repeats its first: drop the repeat (keep its segment's straightness).
    if (s.closed && pts.length > 1 && same(pts[0], pts[pts.length - 1])) pts.pop();
    else if (s.closed) straight.push(true); // Z draws a straight closing line
    const n = pts.length;
    for (let i = 0; i < n; i++) {
      const end = !s.closed && (i === 0 || i === n - 1);
      if (i > 0 && same(pts[i], pts[i - 1])) continue;
      nodes.push({ p: apply(a, pts[i]), kind: "endpoint", what: end ? "Endpoint" : "Vertex", owner, id });
    }
    for (let i = 0; i < straight.length; i++) {
      if (!straight[i]) continue;
      const p0 = pts[i];
      const p1 = pts[(i + 1) % n];
      if (!p1 || same(p0, p1)) continue;
      mids.push({ p: apply(a, [(p0[0] + p1[0]) / 2, (p0[1] + p1[1]) / 2]), kind: "midpoint", what: "Midpoint", owner, id });
    }
  }
  // Nodes first, so a capped object keeps its corners before its midpoints.
  return [...nodes, ...mids].slice(0, max);
}

/** Area centroid of one closed polyline, or null when it has no area. */
export function polygonCentroid(pts: Vec2[]): Vec2 | null {
  let a2 = 0;
  let cx = 0;
  let cy = 0;
  const n = pts.length;
  for (let i = 0, j = n - 1; i < n; j = i++) {
    const cr = pts[j][0] * pts[i][1] - pts[i][0] * pts[j][1];
    a2 += cr;
    cx += (pts[j][0] + pts[i][0]) * cr;
    cy += (pts[j][1] + pts[i][1]) * cr;
  }
  if (Math.abs(a2) < 1e-12) return null;
  return [cx / (3 * a2), cy / (3 * a2)];
}

/** If a closed polyline is a circle (every point at the same distance from its centroid), its centre and radius. */
function circleFit(pts: Vec2[], c: Vec2): number | null {
  if (pts.length < 8) return null;
  let lo = Infinity;
  let hi = 0;
  for (const p of pts) {
    const d = Math.hypot(p[0] - c[0], p[1] - c[1]);
    lo = Math.min(lo, d);
    hi = Math.max(hi, d);
  }
  return hi > 0 && (hi - lo) / hi < 0.002 ? (hi + lo) / 2 : null;
}

/** Snap candidates and outline of one document object (field frame). */
export function objectSnaps(o: DocObject): SnapPart {
  const owner = displayName(o);
  const g = o.geometry;
  const t = o.transform;
  const cmds = geometryCmds(g);
  const polys = flattenCmds(cmds, t, FLATTEN_N);
  const outline = outlineOf(o.id, owner, polys);
  const points: SnapPoint[] = [];
  const pt = (p: Vec2, kind: SnapKind, what: string) => points.push({ p, kind, what, owner, id: o.id });
  switch (g.kind) {
    case "ellipse": {
      // Centre and the four axis ends, through the object transform (so a rotated ellipse's quadrants rotate too).
      pt(apply(t, [0, 0]), "center", "Center");
      for (const q of [[g.rx_mm, 0], [0, g.ry_mm], [-g.rx_mm, 0], [0, -g.ry_mm]] as Vec2[]) pt(apply(t, q), "quadrant", "Quadrant");
      break;
    }
    case "rect":
    case "image": {
      pt(apply(t, [0, 0]), "center", "Center");
      points.push(...pathPoints(cmds, t, owner, o.id));
      break;
    }
    case "path": {
      const closed = polys.filter((p) => p.closed);
      let c: Vec2 | null = null;
      if (polys.length === 1 && closed.length === 1) c = polygonCentroid(closed[0].pts);
      if (!c) {
        const b = cmdsBounds(cmds, t);
        if (b) c = boxCenter(b);
      }
      if (c) {
        pt(c, "center", "Center");
        // A circle drawn as a path (DXF/SVG imports): its quadrants too.
        const r = polys.length === 1 && closed.length === 1 ? circleFit(closed[0].pts, c) : null;
        if (r) for (const q of [[r, 0], [0, r], [-r, 0], [0, -r]] as Vec2[]) pt([c[0] + q[0], c[1] + q[1]], "quadrant", "Quadrant");
      }
      points.push(...pathPoints(cmds, t, owner, o.id));
      break;
    }
  }
  return { points, outline };
}

/** Snap candidates of the stock: centre, corners and edge midpoints (or quadrants of a round stock). */
export function stockSnaps(s: Stock): SnapPart {
  const { cmds, transform } = stockCmds(s);
  const points: SnapPoint[] = [];
  const at = (p: Vec2) => apply(transform, p);
  points.push({ p: at([0, 0]), kind: "center", what: "Center", owner: "stock", id: "stock" });
  if (s.shape.kind === "circle") {
    const r = s.shape.diameter_mm / 2;
    for (const q of [[r, 0], [0, r], [-r, 0], [0, -r]] as Vec2[]) points.push({ p: at(q), kind: "quadrant", what: "Quadrant", owner: "stock", id: "stock" });
  } else {
    for (const p of pathPoints(cmds, transform, "stock", "stock")) points.push(p.kind === "midpoint" ? { ...p, owner: "stock edge" } : p);
  }
  return { points, outline: outlineOf("stock", "stock edge", flattenCmds(cmds, transform, FLATTEN_N)) };
}

/** Snap candidates of the field (or workspace): centre, corners and edge midpoints. No outline. */
export function fieldSnaps(b: Box): SnapPart {
  const [cx, cy] = boxCenter(b);
  const P = (x: number, y: number, kind: SnapKind, what: string, owner: string): SnapPoint => ({ p: [x, y], kind, what, owner, id: "field" });
  return {
    points: [
      P(cx, cy, "center", "Center", "field"),
      P(b.x0, b.y0, "endpoint", "Vertex", "field corner"),
      P(b.x1, b.y0, "endpoint", "Vertex", "field corner"),
      P(b.x1, b.y1, "endpoint", "Vertex", "field corner"),
      P(b.x0, b.y1, "endpoint", "Vertex", "field corner"),
      P(cx, b.y0, "midpoint", "Midpoint", "field edge"),
      P(b.x1, cy, "midpoint", "Midpoint", "field edge"),
      P(cx, b.y1, "midpoint", "Midpoint", "field edge"),
      P(b.x0, cy, "midpoint", "Midpoint", "field edge"),
    ],
    outline: null,
  };
}

// --- index -------------------------------------------------------------------------------------------------------------

export interface SnapIndex {
  points: SnapPoint[];
  outlines: Outline[];
  cell: number;
  cells: Map<number, number[]>;
  /** Point indices sorted by x and by y, for alignment guides. */
  byX: Int32Array;
  byY: Int32Array;
}

const cellKey = (ix: number, iy: number) => (ix + 0x8000) * 0x10000 + (iy + 0x8000);

/** Spatial index over snap parts. Build once per gesture (or per document); queries are then cheap. */
export function buildSnapIndex(parts: readonly SnapPart[]): SnapIndex {
  const points: SnapPoint[] = [];
  const outlines: Outline[] = [];
  for (const p of parts) {
    for (const q of p.points) if (Number.isFinite(q.p[0]) && Number.isFinite(q.p[1])) points.push(q);
    if (p.outline) outlines.push(p.outline);
  }
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const q of points) {
    x0 = Math.min(x0, q.p[0]);
    y0 = Math.min(y0, q.p[1]);
    x1 = Math.max(x1, q.p[0]);
    y1 = Math.max(y1, q.p[1]);
  }
  const extent = points.length ? Math.max(x1 - x0, y1 - y0) : 1;
  // About 256 cells across the extent, at least 0.25 mm, and never more than 30000 cells per axis.
  const cell = Math.max(0.25, extent / 256, extent / 30000);
  const cells = new Map<number, number[]>();
  points.forEach((q, i) => {
    const k = cellKey(Math.floor(q.p[0] / cell), Math.floor(q.p[1] / cell));
    const list = cells.get(k);
    if (list) list.push(i);
    else cells.set(k, [i]);
  });
  const ids = Array.from(points.keys());
  const byX = Int32Array.from([...ids].sort((a, b) => points[a].p[0] - points[b].p[0]));
  const byY = Int32Array.from([...ids].sort((a, b) => points[a].p[1] - points[b].p[1]));
  return { points, outlines, cell, cells, byX, byY };
}

/** Indices of points within `r` of `p` (grid hash, else a linear scan when the query covers too many cells). */
export function pointsNear(ix: SnapIndex, p: Vec2, r: number): number[] {
  const out: number[] = [];
  const c = ix.cell;
  const gx0 = Math.floor((p[0] - r) / c);
  const gx1 = Math.floor((p[0] + r) / c);
  const gy0 = Math.floor((p[1] - r) / c);
  const gy1 = Math.floor((p[1] + r) / c);
  const within = (i: number) => Math.hypot(ix.points[i].p[0] - p[0], ix.points[i].p[1] - p[1]) <= r;
  if ((gx1 - gx0 + 1) * (gy1 - gy0 + 1) > ix.points.length + 64) {
    for (let i = 0; i < ix.points.length; i++) if (within(i)) out.push(i);
    return out;
  }
  for (let gx = gx0; gx <= gx1; gx++)
    for (let gy = gy0; gy <= gy1; gy++) {
      const list = ix.cells.get(cellKey(gx, gy));
      if (list) for (const i of list) if (within(i)) out.push(i);
    }
  return out;
}

/** Visit the segments of outlines that lie in `q` (by chunk box, then segment box). Stops when `visit` returns false. */
function segmentsIn(o: Outline, q: Box, visit: (ax: number, ay: number, bx: number, by: number) => boolean | void): void {
  if (!boxesIntersect(o.box, q)) return;
  const s = o.segs;
  for (const ch of o.chunks) {
    if (!boxesIntersect(ch.box, q)) continue;
    for (let i = ch.first; i < ch.end; i++) {
      const k = i * 4;
      const ax = s[k];
      const ay = s[k + 1];
      const bx = s[k + 2];
      const by = s[k + 3];
      if (Math.max(ax, bx) < q.x0 || Math.min(ax, bx) > q.x1 || Math.max(ay, by) < q.y0 || Math.min(ay, by) > q.y1) continue;
      if (visit(ax, ay, bx, by) === false) return;
    }
  }
}

/** Caps for the per-query work, so a dense drawing never stalls a drag. */
export const LIMITS = { segmentsPerOutline: 256, segments: 2048, pairs: 40000, nearestSegments: 20000, guideScan: 4000 };

/** Intersections between outlines of different things within `r` of `p`. */
export function intersectionsNear(ix: SnapIndex, p: Vec2, r: number): SnapPoint[] {
  const q: Box = { x0: p[0] - r, y0: p[1] - r, x1: p[0] + r, y1: p[1] + r };
  const groups: { o: Outline; segs: number[] }[] = [];
  let total = 0;
  for (const o of ix.outlines) {
    if (total >= LIMITS.segments) break;
    const segs: number[] = [];
    segmentsIn(o, q, (ax, ay, bx, by) => {
      segs.push(ax, ay, bx, by);
      total++;
      return segs.length / 4 < LIMITS.segmentsPerOutline && total < LIMITS.segments;
    });
    if (segs.length) groups.push({ o, segs });
  }
  const out: SnapPoint[] = [];
  let pairs = 0;
  for (let i = 0; i < groups.length; i++)
    for (let j = i + 1; j < groups.length; j++) {
      const A = groups[i];
      const B = groups[j];
      if (A.o.id === B.o.id) continue;
      for (let a = 0; a < A.segs.length; a += 4)
        for (let b = 0; b < B.segs.length; b += 4) {
          if (++pairs > LIMITS.pairs) return out;
          const x = segIntersection(A.segs[a], A.segs[a + 1], A.segs[a + 2], A.segs[a + 3], B.segs[b], B.segs[b + 1], B.segs[b + 2], B.segs[b + 3]);
          if (x && Math.hypot(x[0] - p[0], x[1] - p[1]) <= r) out.push({ p: x, kind: "intersection", what: "Intersection", owner: `${A.o.owner} / ${B.o.owner}`, id: A.o.id });
        }
    }
  return out;
}

/** Proper or touching intersection point of segments ab and cd (null for parallel or disjoint segments). */
export function segIntersection(ax: number, ay: number, bx: number, by: number, cx: number, cy: number, dx: number, dy: number): Vec2 | null {
  const rx = bx - ax;
  const ry = by - ay;
  const sx = dx - cx;
  const sy = dy - cy;
  const den = rx * sy - ry * sx;
  if (Math.abs(den) < 1e-15) return null;
  const t = ((cx - ax) * sy - (cy - ay) * sx) / den;
  const u = ((cx - ax) * ry - (cy - ay) * rx) / den;
  const e = 1e-9;
  if (t < -e || t > 1 + e || u < -e || u > 1 + e) return null;
  return [ax + t * rx, ay + t * ry];
}

/** The closest point on any outline within `r` of `p`, or null. */
export function nearestOnOutline(ix: SnapIndex, p: Vec2, r: number): SnapPoint | null {
  const q: Box = { x0: p[0] - r, y0: p[1] - r, x1: p[0] + r, y1: p[1] + r };
  let best: { d: number; x: number; y: number; o: Outline } | null = null;
  let seen = 0;
  for (const o of ix.outlines) {
    if (seen > LIMITS.nearestSegments) break;
    segmentsIn(o, q, (ax, ay, bx, by) => {
      seen++;
      const vx = bx - ax;
      const vy = by - ay;
      const l2 = vx * vx + vy * vy;
      const t = l2 > 0 ? Math.max(0, Math.min(1, ((p[0] - ax) * vx + (p[1] - ay) * vy) / l2)) : 0;
      const x = ax + t * vx;
      const y = ay + t * vy;
      const d = Math.hypot(x - p[0], y - p[1]);
      if (d <= r && (!best || d < best.d)) best = { d, x, y, o };
      return seen <= LIMITS.nearestSegments;
    });
  }
  if (!best) return null;
  const b = best as { d: number; x: number; y: number; o: Outline };
  return { p: [b.x, b.y], kind: "nearest", what: "Nearest", owner: b.o.owner, id: b.o.id };
}

// --- queries ------------------------------------------------------------------------------------------------------------

export interface SnapOptions {
  /** Tolerance in mm (the pixel tolerance divided by the zoom). */
  tol: number;
  /** Point snaps (centre, quadrant, vertex, midpoint, intersection). */
  points: boolean;
  /** Nearest point on an outline. */
  nearest: boolean;
  /** Alignment guides. */
  guides: boolean;
  /** Grid spacing in mm, 0 for none. */
  grid: number;
  /** Only points inside this box infer guides (the visible canvas), so a guide never runs to something off screen. */
  view?: Box | null;
  /** Extra guide sources (e.g. the centre of the circle being drawn): they infer guides but are not snapped to. */
  extra?: readonly SnapPoint[];
  /** Point snap kinds switched off in Preferences (`kinds.midpoint === false`); absent kinds are on. */
  kinds?: Partial<Record<SnapKind, boolean>>;
}

/** A guide line from the point it aligns with to the snapped point. */
export interface Guide {
  from: Vec2;
  to: Vec2;
  axis: "x" | "y";
}

export interface SnapHit {
  /** The snapped position. */
  p: Vec2;
  kind: SnapKind;
  /** Label at the point, e.g. "Quadrant · Circle 1"; empty for the grid. */
  label: string;
  guides: Guide[];
}

/** Closest guide source on one axis: the point whose coordinate is within `tol` of `v`, closest first, then nearest along the line. */
function guideOn(ix: SnapIndex, axis: 0 | 1, p: Vec2, tol: number, view: Box | null | undefined, extra: readonly SnapPoint[]): SnapPoint | null {
  const order = axis === 0 ? ix.byX : ix.byY;
  const pts = ix.points;
  const v = p[axis];
  // Binary search the first point at v - tol.
  let lo = 0;
  let hi = order.length;
  while (lo < hi) {
    const m = (lo + hi) >> 1;
    if (pts[order[m]].p[axis] < v - tol) lo = m + 1;
    else hi = m;
  }
  let best: { k: number; s: SnapPoint } | null = null;
  const consider = (s: SnapPoint) => {
    const d = Math.abs(s.p[axis] - v);
    if (d > tol) return;
    if (view && (s.p[0] < view.x0 || s.p[0] > view.x1 || s.p[1] < view.y0 || s.p[1] > view.y1)) return;
    const along = Math.abs(s.p[1 - axis] - p[1 - axis]);
    if (along < tol) return; // that point is right here: a point snap, not a guide
    // Closest in the aligned coordinate (to 1 % of the tolerance), then centres (a centre line reads best), then the
    // nearest along the guide.
    const k = Math.round((d / tol) * 100) * 1e12 + (s.kind === "center" ? 0 : 1e9) + along;
    if (!best || k < best.k) best = { k, s };
  };
  for (let i = lo, n = 0; i < order.length && n < LIMITS.guideScan; i++, n++) {
    const s = pts[order[i]];
    if (s.p[axis] > v + tol) break;
    consider(s);
  }
  for (const s of extra) consider(s);
  return best ? (best as { k: number; s: SnapPoint }).s : null;
}

/** Score of a point snap: distance, plus a tenth of the tolerance per priority step (so priority breaks near-ties). */
export function snapScore(kind: SnapKind, d: number, tol: number): number {
  return d + PRIORITY[kind] * 0.1 * tol;
}

/**
 * Snap field point `p`: point snaps (closest, priority breaking near-ties), else the nearest point on an outline, else
 * alignment guides (both axes give their crossing; one axis keeps the other coordinate, or its grid value), else the
 * grid. Null when nothing applies.
 */
export function findSnap(ix: SnapIndex, p: Vec2, o: SnapOptions): SnapHit | null {
  const tol = o.tol;
  const extra = o.extra ?? [];
  const on = (k: SnapKind) => o.kinds?.[k] !== false;
  if (o.points) {
    let best: { s: number; pt: SnapPoint } | null = null;
    const consider = (pt: SnapPoint) => {
      if (!on(pt.kind)) return;
      const d = Math.hypot(pt.p[0] - p[0], pt.p[1] - p[1]);
      if (d > tol) return;
      const s = snapScore(pt.kind, d, tol);
      if (!best || s < best.s) best = { s, pt };
    };
    for (const i of pointsNear(ix, p, tol)) consider(ix.points[i]);
    if (on("intersection")) for (const x of intersectionsNear(ix, p, tol)) consider(x);
    if (best) {
      const pt = (best as { s: number; pt: SnapPoint }).pt;
      return { p: [pt.p[0], pt.p[1]], kind: pt.kind, label: snapLabel(pt), guides: [] };
    }
  }
  if (o.nearest && on("nearest")) {
    const n = nearestOnOutline(ix, p, tol);
    if (n) return { p: n.p, kind: "nearest", label: snapLabel(n), guides: [] };
  }
  const g = o.grid > 0 ? o.grid : 0;
  const onGrid = (v: number) => (g ? Math.round(v / g) * g : v);
  if (o.guides && on("guide")) {
    const gx = guideOn(ix, 0, p, tol, o.view, extra); // vertical guide: same x
    const gy = guideOn(ix, 1, p, tol, o.view, extra); // horizontal guide: same y
    if (gx || gy) {
      const q: Vec2 = [gx ? gx.p[0] : onGrid(p[0]), gy ? gy.p[1] : onGrid(p[1])];
      const guides: Guide[] = [];
      if (gx) guides.push({ from: gx.p, to: q, axis: "x" });
      if (gy) guides.push({ from: gy.p, to: q, axis: "y" });
      const label =
        gx && gy
          ? `Aligned · ${snapLabel(gx)} / ${snapLabel(gy)}`
          : gx
            ? `Vertical · ${snapLabel(gx)}`
            : `Horizontal · ${snapLabel(gy as SnapPoint)}`;
      return { p: q, kind: "guide", label, guides };
    }
  }
  if (g) return { p: [onGrid(p[0]), onGrid(p[1])], kind: "grid", label: "", guides: [] };
  return null;
}

/** The selection's own snap point nearest to `grab` (the reference point that snaps while dragging), or null. */
export function referencePoint(points: readonly SnapPoint[], grab: Vec2): SnapPoint | null {
  let best: { d: number; s: SnapPoint } | null = null;
  for (const s of points) {
    const d = Math.hypot(s.p[0] - grab[0], s.p[1] - grab[1]);
    // Ties go to a centre, then to the higher priority point (a circle's centre over a coincident vertex).
    const rank = (k: SnapKind) => (k === "center" ? -1 : PRIORITY[k]);
    if (!best || d < best.d - 1e-9 || (Math.abs(d - best.d) <= 1e-9 && rank(s.kind) < rank(best.s.kind))) best = { d, s };
  }
  return best?.s ?? null;
}
