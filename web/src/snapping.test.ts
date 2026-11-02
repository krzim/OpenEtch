// Unit tests for snapping.ts: snap candidates per shape (centre, quadrants through the transform, vertices and
// endpoints, midpoints, path centroids, path circles), stock and field points, intersections, nearest-on-outline,
// priority and tolerance, alignment guides, the grid, the drag reference point, circle detection and caps.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { Affine, DocObject, Stock, Vec2 } from "./api/types.ts";
import {
  LIMITS,
  MAX_POINTS_PER_OBJECT,
  buildSnapIndex,
  circleOf,
  displayName,
  fieldSnaps,
  findSnap,
  intersectionsNear,
  isRound,
  nearestOnOutline,
  objectIconName,
  objectSnaps,
  pathPoints,
  polygonCentroid,
  referencePoint,
  snapLabel,
  snapScore,
  stockSnaps,
  type SnapOptions,
  type SnapPoint,
} from "./snapping.ts";
import { parseSvgPath } from "./geometry.ts";

const T = (x: number, y: number): Affine => [1, 0, 0, 1, x, y];
const near = (a: Vec2, b: Vec2, eps = 1e-6) => Math.abs(a[0] - b[0]) <= eps && Math.abs(a[1] - b[1]) <= eps;
const has = (pts: SnapPoint[], kind: string, p: Vec2) => pts.some((s) => s.kind === kind && near(s.p, p));

function obj(id: string, geometry: DocObject["geometry"], transform: Affine = T(0, 0), name = ""): DocObject {
  return { id, name, layer: "layer-1", geometry, transform };
}
const circle = (id: string, r: number, t: Affine = T(0, 0), name = "Circle 1") => obj(id, { kind: "ellipse", rx_mm: r, ry_mm: r }, t, name);
const rect = (id: string, w: number, h: number, t: Affine = T(0, 0), name = "Rectangle 1", cr = 0) =>
  obj(id, { kind: "rect", width_mm: w, height_mm: h, corner_radius_mm: cr }, t, name);
const path = (id: string, d: string, name = "Path 1") => obj(id, { kind: "path", path: d }, T(0, 0), name);

const opts = (o: Partial<SnapOptions> = {}): SnapOptions => ({ tol: 1, points: true, nearest: true, guides: true, grid: 0, ...o });

test("ellipse: centre and four quadrants, through a rotation", () => {
  const rot90: Affine = [0, 1, -1, 0, 10, 5];
  const s = objectSnaps(obj("e", { kind: "ellipse", rx_mm: 4, ry_mm: 2 }, rot90, "Ellipse 2")).points;
  assert.equal(s.length, 5);
  assert.ok(has(s, "center", [10, 5]));
  // Local (4, 0) turns to (0, 4) about the centre.
  assert.ok(has(s, "quadrant", [10, 9]));
  assert.ok(has(s, "quadrant", [8, 5]));
  assert.equal(snapLabel(s.find((p) => p.kind === "quadrant")!), "Quadrant · Ellipse 2");
});

test("rectangle: centre, corners on the shape and edge midpoints", () => {
  const s = objectSnaps(rect("r", 10, 4, T(5, 0))).points;
  assert.ok(has(s, "center", [5, 0]));
  for (const c of [[0, -2], [10, -2], [10, 2], [0, 2]] as Vec2[]) assert.ok(has(s, "endpoint", c), `corner ${c}`);
  for (const m of [[5, -2], [10, 0], [5, 2], [0, 0]] as Vec2[]) assert.ok(has(s, "midpoint", m), `midpoint ${m}`);
  assert.equal(s.filter((p) => p.kind === "endpoint").length, 4);
  assert.equal(s.find((p) => p.kind === "endpoint")!.what, "Vertex");
  // A rounded rectangle: vertices at the tangent points, midpoints only on the straight edges.
  const r = objectSnaps(rect("r", 10, 4, T(0, 0), "R", 1)).points;
  assert.equal(r.filter((p) => p.kind === "endpoint").length, 8);
  assert.equal(r.filter((p) => p.kind === "midpoint").length, 4);
  assert.ok(has(r, "midpoint", [0, 2]));
});

test("paths: open ends are endpoints, inner nodes vertices, straight segments give midpoints, curves none", () => {
  const s = pathPoints(parseSvgPath("M0,0 L10,0 C12,0 12,4 10,4"), [1, 0, 0, 1, 0, 0], "P", "p");
  assert.deepEqual(
    s.filter((p) => p.kind === "endpoint").map((p) => p.what),
    ["Endpoint", "Vertex", "Endpoint"],
  );
  assert.deepEqual(s.filter((p) => p.kind === "midpoint").map((p) => p.p), [[5, 0]]);
  // A closed triangle: three vertices, the closing line's midpoint too, and the area centroid as the centre.
  const tri = objectSnaps(path("t", "M0,0 L6,0 L0,6 Z")).points;
  assert.equal(tri.filter((p) => p.kind === "endpoint").length, 3);
  assert.ok(has(tri, "midpoint", [3, 3]));
  assert.ok(has(tri, "center", [2, 2]), "centroid, not the box centre");
  // An open path's centre is its box centre.
  assert.ok(has(objectSnaps(path("o", "M0,0 L10,0 L10,4")).points, "center", [5, 2]));
  assert.deepEqual(polygonCentroid([[0, 0], [1, 0], [2, 0]]), null, "no area");
});

test("a circle drawn as a path gets its centre and quadrants", () => {
  const k = 0.5522847498 * 5;
  const d = `M5,0 C5,${k} ${k},5 0,5 C${-k},5 -5,${k} -5,0 C-5,${-k} ${-k},-5 0,-5 C${k},-5 5,${-k} 5,0 Z`;
  const s = objectSnaps(path("c", d)).points;
  assert.ok(s.some((p) => p.kind === "center" && near(p.p, [0, 0], 1e-3)));
  assert.equal(s.filter((p) => p.kind === "quadrant").length, 4);
});

test("stock and field points", () => {
  const st: Stock = { name: "s", shape: { kind: "rect", width_mm: 20, height_mm: 10, corner_radius_mm: 0 }, thickness_mm: 1, material: null, center: [0, 0], rotation_deg: 90 };
  const s = stockSnaps(st);
  assert.ok(has(s.points, "center", [0, 0]));
  assert.ok(has(s.points, "endpoint", [5, -10]), "corner rotated by 90 degrees");
  assert.equal(snapLabel(s.points.find((p) => p.kind === "midpoint")!), "Midpoint · stock edge");
  assert.ok(s.outline);
  const round: Stock = { ...st, shape: { kind: "circle", diameter_mm: 30 }, rotation_deg: 0 };
  assert.ok(has(stockSnaps(round).points, "quadrant", [15, 0]));
  const f = fieldSnaps({ x0: -50, y0: -50, x1: 50, y1: 50 });
  assert.equal(f.points.length, 9);
  assert.equal(snapLabel(f.points[0]), "Center · field");
});

test("intersections between outlines of different objects near the cursor", () => {
  const ix = buildSnapIndex([objectSnaps(rect("a", 10, 10)), objectSnaps(rect("b", 10, 10, T(5, 5)))]);
  // a's right edge (x 5) crosses b's bottom edge (y 0) at (5, 0).
  const xs = intersectionsNear(ix, [5.2, 0.3], 1);
  assert.equal(xs.length, 1);
  assert.ok(near(xs[0].p, [5, 0]));
  const hit = findSnap(ix, [5.2, 0.3], opts());
  assert.equal(hit?.kind, "intersection");
  assert.equal(hit?.label, "Intersection · Rectangle 1 / Rectangle 1");
  // An object's own corners are not intersections.
  assert.equal(intersectionsNear(buildSnapIndex([objectSnaps(rect("a", 10, 10))]), [5, 5], 1).length, 0);
  // Circle and line: the crossing lies on the circle (flattening error well under a micrometre... of a 10 mm circle).
  const ix2 = buildSnapIndex([objectSnaps(circle("c", 10)), objectSnaps(path("l", "M-20,3 L20,3"))]);
  const c = intersectionsNear(ix2, [9.5, 3], 1);
  assert.equal(c.length, 1);
  assert.ok(Math.abs(Math.hypot(c[0].p[0], c[0].p[1]) - 10) < 0.005);
});

test("nearest on an outline is the lowest priority", () => {
  const ix = buildSnapIndex([objectSnaps(rect("a", 20, 20))]);
  const n = nearestOnOutline(ix, [3, 10.4], 1);
  assert.ok(n && near(n.p, [3, 10]));
  const hit = findSnap(ix, [3, 10.4], opts());
  assert.equal(hit?.kind, "nearest");
  assert.equal(hit?.label, "Nearest · Rectangle 1");
  // Near the edge midpoint (0, 10) the midpoint wins over the outline, even though the outline is closer.
  assert.equal(findSnap(ix, [0.6, 10.1], opts())?.kind, "midpoint");
  assert.equal(findSnap(ix, [3, 10.4], opts({ nearest: false, guides: false })), null);
});

test("priority breaks near-ties; a clearly closer point still wins", () => {
  assert.ok(snapScore("endpoint", 0.5, 1) < snapScore("midpoint", 0.5, 1));
  assert.ok(snapScore("center", 0.5, 1) < snapScore("quadrant", 0.5, 1));
  const pts = (list: [SnapPoint["kind"], Vec2][]) => ({
    points: list.map(([kind, p]) => ({ p, kind, what: kind, owner: "x", id: "x" })),
    outline: null,
  });
  // Midpoint and endpoint at the same distance: the endpoint.
  let ix = buildSnapIndex([pts([["midpoint", [1, 0]], ["endpoint", [-1, 0]]])]);
  assert.equal(findSnap(ix, [0, 0], opts({ tol: 2 }))?.kind, "endpoint");
  // A midpoint right under the cursor beats an endpoint at the edge of the tolerance.
  ix = buildSnapIndex([pts([["midpoint", [0.05, 0]], ["endpoint", [1.9, 0]]])]);
  assert.equal(findSnap(ix, [0, 0], opts({ tol: 2 }))?.kind, "midpoint");
  // Outside the tolerance nothing snaps.
  assert.equal(findSnap(ix, [10, 10], opts({ tol: 2, guides: false })), null);
});

test("alignment guides: one axis keeps the other coordinate, two axes give the crossing", () => {
  const ix = buildSnapIndex([objectSnaps(circle("a", 2, T(0, 0))), objectSnaps(circle("b", 2, T(40, 30), "Circle 2"))]);
  // Level with a's centre (y 0), far from everything else.
  let hit = findSnap(ix, [20, 0.4], opts());
  assert.equal(hit?.kind, "guide");
  assert.deepEqual(hit?.p, [20, 0]);
  assert.equal(hit?.guides.length, 1);
  assert.equal(hit?.guides[0].axis, "y");
  assert.ok(hit?.label.startsWith("Horizontal · Center · Circle 1"));
  // Plumb with b's centre (x 40) and level with a's centre: the crossing of both guides.
  hit = findSnap(ix, [40.3, -0.5], opts());
  assert.deepEqual(hit?.p, [40, 0]);
  assert.equal(hit?.guides.length, 2);
  // Guides only (Alt): no point snap even right on a centre's quadrant.
  hit = findSnap(ix, [2.1, 0.05], opts({ points: false, nearest: false }));
  assert.notEqual(hit?.kind, "quadrant");
  // Points outside the visible box infer no guides.
  assert.equal(findSnap(ix, [20, 0.4], opts({ view: { x0: 10, y0: 10, x1: 30, y1: 30 } })), null);
});

test("a single guide combines with the grid; the grid alone otherwise", () => {
  const ix = buildSnapIndex([objectSnaps(circle("a", 2, T(0, 0.25)))]);
  const hit = findSnap(ix, [20.3, 0.4], opts({ grid: 1 }));
  assert.deepEqual(hit?.p, [20, 0.25]);
  const g = findSnap(buildSnapIndex([]), [1.3, 2.6], opts({ grid: 1 }));
  assert.equal(g?.kind, "grid");
  assert.deepEqual(g?.p, [1, 3]);
  assert.equal(g?.label, "");
});

test("extra guide sources infer guides but are never snapped to", () => {
  const start: SnapPoint = { p: [0, 0], kind: "center", what: "Center", owner: "new circle", id: "draw" };
  const ix = buildSnapIndex([]);
  assert.equal(findSnap(ix, [0.1, 0.1], opts({ extra: [start], guides: false })), null);
  const hit = findSnap(ix, [10, 0.3], opts({ extra: [start] }));
  assert.deepEqual(hit?.p, [10, 0]);
});

test("the drag reference is the selection's own snap point nearest the grab", () => {
  const pts = objectSnaps(rect("r", 10, 10)).points;
  assert.ok(near(referencePoint(pts, [4.6, 4.2])!.p, [5, 5]), "the top-right corner");
  assert.ok(near(referencePoint(pts, [0.3, -1])!.p, [0, 0]), "the centre");
  assert.equal(referencePoint([], [0, 0]), null);
  // A coincident centre and vertex: the centre (priority).
  const same: SnapPoint[] = [
    { p: [0, 0], kind: "endpoint", what: "Vertex", owner: "", id: "" },
    { p: [0, 0], kind: "center", what: "Center", owner: "", id: "" },
  ];
  assert.equal(referencePoint(same, [1, 1])?.kind, "center");
});

test("circles: round only with equal radii and no stretch; names and diameter", () => {
  assert.ok(isRound(circle("c", 5)));
  assert.ok(isRound(circle("c", 5, [0, 2, -2, 0, 1, 1])), "rotated and uniformly scaled");
  assert.ok(!isRound(circle("c", 5, [2, 0, 0, 1, 0, 0])), "stretched");
  assert.ok(!isRound(obj("e", { kind: "ellipse", rx_mm: 5, ry_mm: 4 })));
  assert.deepEqual(circleOf(circle("c", 5, [0, 2, -2, 0, 1, 1])), { center: [1, 1], diameter: 20 });
  assert.equal(displayName(obj("e", { kind: "ellipse", rx_mm: 3, ry_mm: 3 }, T(0, 0), "Ellipse 3")), "Circle 3");
  assert.equal(displayName(obj("e", { kind: "ellipse", rx_mm: 3, ry_mm: 2 }, T(0, 0), "Ellipse 3")), "Ellipse 3");
  assert.equal(displayName(obj("e", { kind: "ellipse", rx_mm: 3, ry_mm: 3 }, T(0, 0), "Ellipses")), "Ellipses");
  assert.equal(displayName(obj("obj-7", { kind: "ellipse", rx_mm: 3, ry_mm: 3 })), "Circle obj-7");
});

test("Browser icons: paths (lines, polygons, converted shapes) get the path icon, never Import", () => {
  assert.equal(objectIconName(path("p", "M0,0 L10,0")), "path");
  assert.equal(objectIconName(circle("c", 5)), "circle");
  assert.equal(objectIconName({ geometry: { kind: "text" } as never, transform: [1, 0, 0, 1, 0, 0] as never }), "text");
  assert.equal(objectIconName(obj("e", { kind: "ellipse", rx_mm: 5, ry_mm: 4 })), "ellipse");
  assert.equal(objectIconName(obj("r", { kind: "rect", width_mm: 5, height_mm: 4, corner_radius_mm: 0 })), "rect");
  assert.equal(objectIconName(obj("i", { kind: "image", width_mm: 5, height_mm: 4, mime: "image/png", data_base64: "" })), "image");
});

test("caps: a huge path keeps at most MAX_POINTS_PER_OBJECT candidates, corners first", () => {
  let d = "M0,0";
  for (let i = 1; i <= 6000; i++) d += ` L${i},${i % 2}`;
  const s = objectSnaps(path("big", d)).points;
  assert.ok(s.length <= MAX_POINTS_PER_OBJECT + 1, "centre plus the capped list");
  assert.ok(s.filter((p) => p.kind === "endpoint").length >= MAX_POINTS_PER_OBJECT - 1);
});

test("performance: 500 objects index and snap well under a frame", () => {
  const parts = [];
  for (let i = 0; i < 500; i++) {
    const x = (i % 25) * 6 - 75;
    const y = Math.floor(i / 25) * 6 - 60;
    parts.push(objectSnaps(i % 2 ? circle(`c${i}`, 2.5, T(x, y)) : rect(`r${i}`, 5, 4, T(x, y))));
  }
  const t0 = performance.now();
  const ix = buildSnapIndex(parts);
  const t1 = performance.now();
  let hits = 0;
  for (let k = 0; k < 200; k++) {
    const p: Vec2 = [Math.sin(k) * 70, Math.cos(k * 1.3) * 60];
    if (findSnap(ix, p, opts({ tol: 2 }))) hits++;
  }
  const t2 = performance.now();
  assert.ok(hits > 0);
  assert.ok(t1 - t0 < 200, `index built in ${(t1 - t0).toFixed(1)} ms`);
  assert.ok((t2 - t1) / 200 < 8, `${((t2 - t1) / 200).toFixed(2)} ms per snap`);
  assert.ok(LIMITS.pairs > 0);
});
