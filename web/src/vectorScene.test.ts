// Unit tests for vectorScene.ts, the raster canvas's view-independent vector data: flattening, the packed scene and its
// flags, the look (the old SVG layer's), the back layer's painting, and that what the canvas draws is exactly what the
// geometric hit test can pick (clicks on lines, inside closed shapes, groups, hidden layers, touch tolerances).

import assert from "node:assert/strict";
import { test } from "node:test";
import type { CutSettings, DocObject, GDocument, Layer } from "./api/types.ts";
import { geometryCmds, parseSvgPath } from "./geometry.ts";
import { TOL } from "./pointer.ts";
import { applySelect, buildShapeIndex, hitTest } from "./selection.ts";
import {
  FLAG_OUTSIDE,
  FLAG_SELECTED,
  META_FILL_ONLY,
  META_NO_FILL,
  RENDER_TOL_MM,
  SEG_STRIDE,
  antsPhase,
  buildVectorScene,
  crisp,
  flattenSegments,
  gridLines,
  groupLook,
  paintBack,
  segmentFlags,
  strokeVisibleRuns,
  type BackCtx,
  type CanvasTokens,
  type VectorScene,
} from "./vectorScene.ts";
import { canvasColor } from "./workspace/theme.ts";

const I: [number, number, number, number, number, number] = [1, 0, 0, 1, 0, 0];
const cut = (mode: CutSettings["mode"]): CutSettings => ({ mode }) as CutSettings;
const layer = (id: string, o: Partial<Layer> = {}): Layer => ({ id, name: id, color: "#d22020", visible: true, output: true, settings: cut("line"), sub_layers: [], ...o });
const path = (id: string, d: string, lay = "L1", extra: Partial<DocObject> = {}): DocObject => ({
  id,
  name: id,
  layer: lay,
  geometry: { kind: "path", path: d } as DocObject["geometry"],
  transform: I,
  ...extra,
});
const rect = (id: string, x: number, y: number, w = 10, h = 10, lay = "L1", extra: Partial<DocObject> = {}): DocObject => ({
  id,
  name: id,
  layer: lay,
  geometry: { kind: "rect", width_mm: w, height_mm: h } as DocObject["geometry"],
  transform: [1, 0, 0, 1, x, y],
  ...extra,
});
const docOf = (layers: Layer[], objects: DocObject[], groups: GDocument["groups"] = []): GDocument =>
  ({ revision: 1, name: "t", stock: null, layers, objects, groups, next_id: 1 }) as GDocument;

/** Segments of a scene as plain records. */
function segsOf(s: Float32Array) {
  const out = [];
  for (let k = 0; k < s.length; k += SEG_STRIDE)
    out.push({ a: [s[k], s[k + 1]], b: [s[k + 2], s[k + 3]], along: s[k + 4], anchor: [s[k + 5], s[k + 6]], meta: s[k + 7] });
  return out;
}

test("a closed polyline: one segment per edge, the closing edge stroked, dash distance along the subpath", () => {
  const s = segsOf(flattenSegments(parseSvgPath("M0,0 L10,0 L10,5 Z"), I));
  assert.equal(s.length, 3);
  assert.deepEqual(s[2].a, [10, 5]);
  assert.deepEqual(s[2].b, [0, 0]);
  assert.deepEqual(
    s.map((x) => x.along),
    [0, 10, 15],
  );
  assert.ok(s.every((x) => x.meta === 0 && x.anchor[0] === 0 && x.anchor[1] === 0));
});

test("an open subpath gets a fill-only closing edge (SVG fills it closed, never strokes it); subpaths restart", () => {
  const s = segsOf(flattenSegments(parseSvgPath("M0,0 L10,0 L10,10 M20,0 L30,0 L30,10 L20,0"), I));
  assert.equal(s.length, 6, "two strokes + closing edge, then three (the second ends where it began)");
  assert.equal(s[2].meta, META_FILL_ONLY);
  assert.deepEqual(s[2].a, [10, 10]);
  assert.deepEqual(s[2].b, [0, 0]);
  assert.deepEqual(s[3].anchor, [20, 0]);
  assert.equal(s[3].along, 0, "dashes restart per subpath");
  assert.ok(s.slice(3).every((x) => x.meta === 0));
});

test("zero-length segments are dropped (an SVG butt-capped dot draws nothing)", () => {
  assert.equal(flattenSegments(parseSvgPath("M5,5 L5,5"), I).length, 0);
  assert.equal(flattenSegments(parseSvgPath("M5,5 Z"), I).length, 0);
});

test("curves flatten within the render tolerance under the object transform, so the zoom never shows facets", () => {
  // A 50 mm circle scaled 2x: radius 100 mm in the field.
  const cmds = geometryCmds({ kind: "ellipse", rx_mm: 50, ry_mm: 50 } as DocObject["geometry"]);
  const s = segsOf(flattenSegments(cmds, [2, 0, 0, 2, 10, 20]));
  let worst = 0;
  for (const x of s) {
    // The chord's midpoint is the furthest point from the arc.
    const mx = (x.a[0] + x.b[0]) / 2 - 10;
    const my = (x.a[1] + x.b[1]) / 2 - 20;
    worst = Math.max(worst, 100 - Math.hypot(mx, my));
  }
  // Four cubics approximate a circle to about 0.03 %; the flattening adds at most the tolerance.
  assert.ok(worst <= 100 * 3e-4 + RENDER_TOL_MM, `chord error ${worst}`);
  assert.ok(s.length < 4 * 512, "bounded");
  // Small curves stay cheap.
  const small = flattenSegments(geometryCmds({ kind: "ellipse", rx_mm: 0.5, ry_mm: 0.5 } as DocObject["geometry"]), I);
  assert.ok(small.length / SEG_STRIDE <= 64);
});

test("scene: visible layers in document order, unknown layers last, hidden layers left out, objects indexed", () => {
  const d = docOf(
    [layer("A"), layer("H", { visible: false }), layer("B")],
    [rect("b1", 0, 0, 10, 10, "B"), rect("a1", 0, 0, 10, 10, "A"), rect("h1", 0, 0, 10, 10, "H"), rect("x1", 0, 0, 10, 10, "gone"), rect("a2", 50, 0, 10, 10, "A")],
  );
  const s = buildVectorScene(d);
  assert.deepEqual(
    s.groups.map((g) => g.key),
    ["A", "B", ""],
  );
  assert.deepEqual(
    s.objects.map((o) => o.id),
    ["a1", "a2", "b1", "x1"],
  );
  assert.equal(s.count, 16);
  assert.deepEqual(
    s.groups.map((g) => [g.first, g.count]),
    [
      [0, 8],
      [8, 4],
      [12, 4],
    ],
  );
  // meta carries the object index.
  const metas = segsOf(s.segs).map((x) => Math.floor(x.meta / 4));
  assert.deepEqual(metas, [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3]);
  assert.equal(s.objIndex.get("b1"), 2);
  assert.equal(buildVectorScene(null).count, 0);
});

test("image frames are outlined but never filled", () => {
  const img = { id: "i", name: "i", layer: "L1", geometry: { kind: "image", width_mm: 10, height_mm: 5, mime: "image/png", data_base64: "" }, transform: I } as unknown as DocObject;
  const s = buildVectorScene(docOf([layer("L1")], [img]));
  assert.equal(s.count, 4);
  assert.ok(segsOf(s.segs).every((x) => (x.meta & META_NO_FILL) !== 0));
});

test("a refetched document reuses the flattened objects; a moved or reshaped one is flattened again", () => {
  const d = docOf([layer("L1")], [rect("r", 0, 0), path("p", "M0,0 C10,0 10,10 0,10 Z")]);
  const s1 = buildVectorScene(d);
  const again = buildVectorScene(structuredClone(d));
  assert.deepEqual([...again.segs], [...s1.segs]);
  const moved = structuredClone(d);
  moved.objects[0].transform = [1, 0, 0, 1, 5, 0];
  const s2 = buildVectorScene(moved);
  assert.equal(s2.segs[0], s1.segs[0] + 5);
  const reshaped = structuredClone(d);
  (reshaped.objects[1].geometry as { path: string }).path = "M0,0 L20,0 L20,20 Z";
  const s3 = buildVectorScene(reshaped);
  assert.equal(s3.count, 4 + 3);
});

test("flags: selected and outside bits on exactly their objects' segments", () => {
  const s = buildVectorScene(docOf([layer("L1")], [rect("a", 0, 0), rect("b", 20, 0), rect("c", 40, 0)]));
  const f = segmentFlags(s, new Set(["b", "nope"]), new Set(["c", "b"]));
  assert.deepEqual([...f], [0, 0, 0, 0, 3, 3, 3, 3, 2, 2, 2, 2]);
  assert.equal(FLAG_SELECTED | FLAG_OUTSIDE, 3);
});

test("look: the old SVG layer's colours and opacities", () => {
  const out = { layer: layer("L", { color: "#000000", settings: cut("fill") }) };
  const noOut = { layer: layer("N", { color: "#2a9d4a", output: false, settings: cut("fill") }) };
  const line = { layer: layer("Ln", { color: "#2a9d4a", settings: cut("line") }) };
  assert.deepEqual(groupLook(out, "dark", true, false), {
    stroke: canvasColor("#000000", "dark"),
    strokeAlpha: 1,
    fill: canvasColor("#000000", "dark"),
    fillAlpha: 0.35,
    movingFillAlpha: 0.35,
  });
  const n = groupLook(noOut, "light", true, false);
  assert.equal(n.strokeAlpha, 0.4, "no-output layers at 40 %");
  assert.equal(n.fillAlpha, 0.15);
  assert.ok(Math.abs(n.movingFillAlpha - 0.14) < 1e-9, "a moving fill sits in the no-output group: 35 % x 40 %");
  assert.equal(groupLook(line, "light", true, false).fill, null, "line layers are not filled");
  assert.equal(groupLook(out, "light", false, false).fill, null, "wireframe view");
  const dim = groupLook(noOut, "light", true, true);
  assert.ok(Math.abs(dim.strokeAlpha - 0.12) < 1e-9 && Math.abs(dim.fillAlpha - 0.045) < 1e-9, "the preview dims to 30 %");
  assert.deepEqual(groupLook({ layer: null }, "light", true, false), { stroke: canvasColor(undefined, "light"), strokeAlpha: 1, fill: null, fillAlpha: 0.15, movingFillAlpha: 0.35 });
});

test("marching ants: 9 px per 0.8 s, in half-pixel steps", () => {
  assert.equal(antsPhase(0), 0);
  assert.equal(antsPhase(400), 4.5);
  assert.equal(antsPhase(800), 0);
  assert.equal(antsPhase(130), 1.0);
  assert.ok(Math.abs(antsPhase(130, 0) - 1.4625) < 1e-9);
});

test("grid lines: only those inside the workspace and on screen, majors apart", () => {
  const g = gridLines(10, 50, 100, -35, 120);
  assert.deepEqual(g.major, [0, 50, 100]);
  assert.deepEqual(g.minor, [-30, -20, -10, 10, 20, 30, 40, 60, 70, 80, 90]);
  assert.deepEqual(gridLines(0, 0, 100, -10, 10), { minor: [], major: [] });
  assert.equal(crisp(10.2), 10.5);
  assert.equal(crisp(10.9), 10.5);
});

const TOKENS: CanvasTokens = {
  field: "F",
  fieldEdge: "E",
  gridMinor: "g",
  gridMajor: "G",
  axisX: "AX",
  axisY: "AY",
  stockFill: "SF",
  stockStroke: "SS",
  accent: "AC",
  accentFill: "AF",
  outside: "O",
};

/** A 2D context that records what is drawn. */
function recorder() {
  const ops: { op: string; style?: unknown; width?: number; dash?: number[]; n?: number }[] = [];
  let moves = 0;
  let dash: number[] = [];
  const ctx = {
    fillStyle: "" as unknown,
    strokeStyle: "" as unknown,
    lineWidth: 1,
    lineDashOffset: 0,
    beginPath: () => (moves = 0),
    moveTo: () => moves++,
    lineTo: () => {},
    bezierCurveTo: () => {},
    quadraticCurveTo: () => {},
    closePath: () => {},
    setTransform: () => {},
    setLineDash: (d: number[]) => (dash = d),
    fillRect: () => ops.push({ op: "fillRect", style: ctx.fillStyle }),
    strokeRect: () => ops.push({ op: "strokeRect", style: ctx.strokeStyle, width: ctx.lineWidth }),
    fill: () => ops.push({ op: "fill", style: ctx.fillStyle }),
    stroke: () => ops.push({ op: "stroke", style: ctx.strokeStyle, width: ctx.lineWidth, dash: [...dash], n: moves }),
  };
  return { ctx: ctx as unknown as BackCtx, ops };
}

test("back layer: field, on-screen grid lines, axes, edge and the dashed stock, widths in device px", () => {
  const { ctx, ops } = recorder();
  // 100 x 100 mm field at 10 px/mm in a 500 x 400 canvas at DPR 2: about 50 x 40 mm on screen.
  paintBack(
    ctx,
    { view: { cx: 0, cy: 0, scale: 10 }, size: [500, 400], dpr: 2 },
    {
      half: [50, 50],
      grid: { visible: true, prefMm: 0 },
      stock: { cmds: geometryCmds({ kind: "rect", width_mm: 20, height_mm: 20 } as DocObject["geometry"]), m: I, selected: true },
      colors: TOKENS,
    },
  );
  assert.deepEqual(
    ops.map((o) => [o.op, o.style]),
    [
      ["fillRect", "F"],
      ["stroke", "g"],
      ["stroke", "G"],
      ["stroke", "AX"],
      ["stroke", "AY"],
      ["strokeRect", "E"],
      ["fill", "AF"],
      ["stroke", "AC"],
    ],
  );
  const minor = ops[1];
  const major = ops[2];
  // 1 mm minor lines at 10 px/mm (10 px apart), every 10th major; only those on screen (x: -25..25, y: -20..20 mm).
  assert.equal((minor.n ?? 0) + (major.n ?? 0), 51 + 41);
  assert.equal(minor.width, 1.2, "0.6 px at DPR 2");
  assert.equal(major.width, 1.8);
  assert.equal(ops[3].width, 2.4);
  assert.equal(ops[5].width, 3);
  assert.equal(ops[7].width, 4, "selected stock: 2 px");
  assert.deepEqual(ops[7].dash, [12, 6], "6 3 dash at DPR 2");

  const off = recorder();
  paintBack(off.ctx, { view: { cx: 0, cy: 0, scale: 10 }, size: [500, 400], dpr: 1 }, { half: [50, 50], grid: { visible: false, prefMm: 0 }, stock: null, colors: TOKENS });
  assert.deepEqual(
    off.ops.map((o) => o.style),
    ["F", "AX", "AY", "E"],
    "no grid, no stock",
  );
});

test("a dashed outline far larger than the screen strokes only its runs on screen, dashes continuing", () => {
  // A 140 mm stock circle at 400 px/mm: about 176,000 px around, a 2560 x 1440 window shows a sliver of it.
  const segs = flattenSegments(geometryCmds({ kind: "ellipse", rx_mm: 70, ry_mm: 70 } as DocObject["geometry"]), I);
  // Centred on the outline at y = 20 mm (away from where the path starts, at y = 0).
  const vm: [number, number, number, number, number, number] = [400, 0, 0, -400, 1280 - 400 * Math.sqrt(70 * 70 - 400), 720 + 400 * 20];
  let lines = 0;
  const offsets: number[] = [];
  const ctx = { lineDashOffset: 0, beginPath: () => {}, moveTo: () => {}, lineTo: () => lines++, stroke: () => offsets.push(ctx.lineDashOffset) };
  const runs = strokeVisibleRuns(ctx as never, segs, vm, 2560, 1440, 4, [6, 3]);
  assert.equal(runs, 1);
  assert.ok(lines > 0 && lines < (segs.length / SEG_STRIDE) / 10, `${lines} of ${segs.length / SEG_STRIDE} segments`);
  assert.ok(offsets[0] >= 0 && offsets[0] < 9);
  // The whole outline on screen: one run from the start, offset 0.
  const all: number[] = [];
  const c2 = { lineDashOffset: 0, beginPath: () => {}, moveTo: () => {}, lineTo: () => {}, stroke: () => all.push(c2.lineDashOffset) };
  assert.equal(strokeVisibleRuns(c2 as never, segs, [5, 0, 0, -5, 1280, 720], 2560, 1440, 4, [6, 3]), 1);
  assert.deepEqual(all, [0]);
});

// --- the canvas draws what the hit test picks ----------------------------------------------------------------------

/** Midpoint of the first stroked segment of object `id` in the scene. */
function onLine(s: VectorScene, id: string): [number, number] {
  const o = s.objects[s.objIndex.get(id) as number];
  for (let k = o.first; k < o.first + o.count; k++) {
    const b = k * SEG_STRIDE;
    if (s.segs[b + 7] % 4 & META_FILL_ONLY) continue;
    return [(s.segs[b] + s.segs[b + 2]) / 2, (s.segs[b + 1] + s.segs[b + 3]) / 2];
  }
  throw new Error("no stroked segment");
}

const hitDoc = docOf(
  [layer("L1", { settings: cut("fill") }), layer("L2"), layer("H", { visible: false })],
  [
    rect("big", 0, 0, 40, 40),
    rect("small", 0, 0, 10, 10, "L2"),
    path("donut", "M90,-10 L110,-10 L110,10 L90,10 Z M95,-5 L105,-5 L105,5 L95,5 Z"),
    path("open", "M200,0 L220,0 L220,20"),
    path("curve", "M300,0 C320,0 320,20 300,20 Z", "L2"),
    rect("g1", 400, 0, 10, 10, "L1", { group: "G" }),
    rect("g2", 420, 0, 10, 10, "L2", { group: "G" }),
    rect("hidden", 500, 0, 10, 10, "H"),
    rect("orphan", 600, 0, 10, 10, "no-such-layer"),
  ],
  [{ id: "G", name: "G" }],
);

test("hit parity: the canvas draws exactly the objects the hit test can pick, hidden layers neither", () => {
  const s = buildVectorScene(hitDoc);
  const idx = buildShapeIndex(hitDoc);
  assert.deepEqual(new Set(s.objects.map((o) => o.id)), new Set(idx.map((e) => e.id)));
  assert.ok(!s.objIndex.has("hidden"));
  assert.ok(s.objIndex.has("orphan"), "objects on unknown layers stay drawn and selectable");
});

test("hit parity: a click on any drawn line picks its object, with the mouse, pen and touch tolerances", () => {
  const s = buildVectorScene(hitDoc);
  const idx = buildShapeIndex(hitDoc);
  const scale = 4; // px per mm
  for (const cls of ["mouse", "pen", "touch"] as const) {
    const tol = TOL[cls].click / scale;
    for (const o of s.objects) {
      const [x, y] = onLine(s, o.id);
      assert.equal(hitTest(idx, [x, y], tol, cls !== "mouse"), o.id, `${cls}: on the line of ${o.id}`);
      // Just inside the tolerance, off the line (every first segment here is horizontal).
      assert.equal(hitTest(idx, [x, y + tol * 0.9], tol, cls !== "mouse"), o.id, `${cls}: near the line of ${o.id}`);
    }
    // Beyond the tolerance and outside every closed shape: nothing.
    assert.equal(hitTest(idx, [210, 5 + tol * 1.5 + 5], tol, cls !== "mouse"), null);
  }
});

test("hit parity: inside closed shapes (smallest wins, holes and open paths do not count), groups select whole", () => {
  const idx = buildShapeIndex(hitDoc);
  const tol = TOL.mouse.click / 4;
  assert.equal(hitTest(idx, [12, 12], tol), "big");
  assert.equal(hitTest(idx, [1, 1], tol), "small");
  assert.equal(hitTest(idx, [92, 0], tol), "donut");
  assert.equal(hitTest(idx, [100, 0], tol), null, "the hole");
  assert.equal(hitTest(idx, [214, 6], tol), null, "inside an open path's hull");
  assert.equal(hitTest(idx, [310, 10], tol), "curve", "inside a curved closed shape");
  assert.equal(hitTest(idx, [505, 0], tol), null, "objects on hidden layers are not drawn and not hit");
  const hit = hitTest(idx, [401, 1], tol);
  assert.equal(hit, "g1");
  const sel = applySelect({ objects: [], layer: null, stock: false }, hitDoc, [hit as string], "replace");
  assert.deepEqual(sel.objects, ["g1", "g2"], "a group member selects the group");
  // Touch: the nearest of two close lines wins.
  const near = docOf([layer("L1")], [path("a", "M0,0 L10,0"), path("b", "M0,2 L10,2")]);
  const ni = buildShapeIndex(near);
  assert.equal(hitTest(ni, [5, 0.6], TOL.touch.click / 4, true), "a");
  assert.equal(hitTest(ni, [5, 0.6], TOL.mouse.click / 2), "b", "the mouse keeps the topmost line");
});

test("hit-test index cache: a refetched document gives equal entries, a moved object a new one", () => {
  const a = buildShapeIndex(hitDoc);
  const b = buildShapeIndex(structuredClone(hitDoc));
  assert.deepEqual(b, a);
  const moved = structuredClone(hitDoc);
  moved.objects[0].transform = [1, 0, 0, 1, 1000, 0];
  const c = buildShapeIndex(moved);
  assert.equal(c[0].box.x0, 980);
  assert.equal(hitTest(c, [1000, 20], 0.5), "big");
  assert.equal(hitTest(c, [0, 20], 0.5), null);
});
