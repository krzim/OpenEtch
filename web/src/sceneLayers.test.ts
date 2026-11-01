// Unit tests for sceneLayers.ts: the view transform, visible region, grid spacing, image smoothing, per-frame view
// coalescing, the tiled toolpath batches and their look.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { CutSettings, GDocument, Layer, Preview, PreviewSegment } from "./api/types.ts";
import {
  batchLook,
  buildToolpath,
  devicePxPerImagePx,
  filledTiles,
  frameCoalescer,
  gridSpacing,
  imageSmoothing,
  parseHexColor,
  viewMatrix,
  visibleBox,
  visibleTiles,
  type ToolpathOptions,
  type ToolpathStyle,
} from "./sceneLayers.ts";

const cut = (o: Partial<CutSettings> = {}): CutSettings => ({
  mode: "fill",
  power_pct: 40,
  speed_mm_s: 1000,
  frequency_khz: 30,
  pulse_width_ns: 200,
  passes: 1,
  hatch: { interval_mm: 0.05, angle_deg: 0, angle_increment_deg: 0, cross_hatch: false, bidirectional: true },
  timing: null,
  ...o,
});
const layer = (id: string, o: Partial<Layer> = {}): Layer => ({ id, name: id, color: "#d22020", visible: true, output: true, settings: cut(), sub_layers: [], ...o });
const docOf = (...layers: Layer[]): GDocument => ({ revision: 1, name: "t", stock: null, layers, objects: [], next_id: 1 });
const seg = (kind: "mark" | "jump", layer: string, from: [number, number], to: [number, number], pass = 0): PreviewSegment => ({ kind, layer, pass, from, to });
const previewOf = (segments: PreviewSegment[]): Preview => ({ segments }) as unknown as Preview;
const all: ToolpathOptions = { hidden: new Set(), hiddenPasses: new Set(), upTo: null, jumps: true };

const apply = (m: number[], p: [number, number]): [number, number] => [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]];

test("viewMatrix: the view centre lands mid-canvas, +Y up, scaled by the device pixel ratio", () => {
  const v = { cx: 10, cy: -5, scale: 4 };
  assert.deepEqual(apply(viewMatrix(v, [800, 600]), [10, -5]), [400, 300]);
  assert.deepEqual(apply(viewMatrix(v, [800, 600]), [11, -4]), [404, 296]);
  assert.deepEqual(apply(viewMatrix(v, [800, 600], 2), [11, -4]), [808, 592]);
  // Same numbers as the SVG view group's matrix at dpr 1.
  assert.deepEqual(viewMatrix(v, [800, 600]), [4, 0, 0, -4, 400 - 40, 300 - 20]);
});

test("visibleBox: the field region on screen, widened by a margin", () => {
  assert.deepEqual(visibleBox({ cx: 0, cy: 0, scale: 2 }, [200, 100]), { x0: -50, y0: -25, x1: 50, y1: 25 });
  assert.deepEqual(visibleBox({ cx: 1, cy: 1, scale: 2 }, [200, 100], 10), { x0: -54, y0: -29, x1: 56, y1: 31 });
});

test("gridSpacing: steps of 0.5..50 mm at least 10 px apart; a preference spacing while it is 4 px or more", () => {
  assert.deepEqual(gridSpacing(3, 0, 77), { minor: 5, major: 10 });
  assert.deepEqual(gridSpacing(3.2, 0, 77), { minor: 5, major: 10 });
  assert.deepEqual(gridSpacing(25, 0, 77), { minor: 0.5, major: 1 });
  assert.deepEqual(gridSpacing(0.2, 0, 77), { minor: 50, major: 100 });
  assert.deepEqual(gridSpacing(1, 0, 77), { minor: 10, major: 100 });
  assert.deepEqual(gridSpacing(1, 5, 77), { minor: 5, major: 50 });
  // 1 mm at 2 px/mm is too dense: back to the automatic steps.
  assert.deepEqual(gridSpacing(2, 1, 77), { minor: 5, major: 10 });
  // The spacing only changes at those steps, so the grid path is not rebuilt on every zoom frame.
  const seen = new Set<string>();
  for (let s = 0.2; s < 400; s *= 1.05) seen.add(JSON.stringify(gridSpacing(s, 0, 77)));
  assert.ok(seen.size <= 7);
});

test("image smoothing: off once a bitmap is magnified (crisp pixels), on while it is shrunk", () => {
  // A 100 mm wide, 1000 px image at 20 device px/mm: 2 px per image pixel.
  assert.equal(devicePxPerImagePx([20, 0, 0, -20, 0, 0], 100, 1000), 2);
  // Rotated and mirrored transforms measure the same.
  const c = Math.cos(0.5) * 20;
  const s = Math.sin(0.5) * 20;
  assert.ok(Math.abs(devicePxPerImagePx([c, s, s, -c, 5, 5], 100, 1000) - 2) < 1e-9);
  assert.equal(imageSmoothing(2).enabled, false);
  assert.equal(imageSmoothing(0.5).enabled, true);
  assert.equal(imageSmoothing(1).enabled, true);
});

test("frameCoalescer: many updates in a frame flush once, with the newest value", () => {
  const frames: (() => void)[] = [];
  const flushed: number[] = [];
  const c = frameCoalescer<number>(
    (cb) => frames.push(cb),
    (v) => flushed.push(v),
  );
  c.push(1);
  c.push(2);
  c.push(3);
  assert.equal(frames.length, 1);
  assert.equal(c.pending(), true);
  frames.shift()?.();
  assert.deepEqual(flushed, [3]);
  assert.equal(c.pending(), false);
  c.push(4);
  frames.shift()?.();
  assert.deepEqual(flushed, [3, 4]);
});

test("buildToolpath: batches by layer and pass, jumps first, hidden layers and jumps left out", () => {
  const doc = docOf(layer("A"), layer("B"));
  const p = previewOf([seg("jump", "A", [0, 0], [1, 0]), seg("mark", "A", [1, 0], [2, 0]), seg("jump", "B", [2, 0], [3, 3]), seg("mark", "B", [3, 3], [4, 4]), seg("mark", "A", [5, 5], [6, 6])]);
  const tp = buildToolpath(p, doc, all);
  assert.deepEqual(
    tp.batches.map((b) => [b.key, b.coords.length / 4]),
    [
      ["jump", 2],
      ["A#0", 2],
      ["B#0", 1],
    ],
  );
  assert.equal(tp.segments, 5);
  assert.equal(tp.head, null);
  const noB = buildToolpath(p, doc, { ...all, hidden: new Set(["B"]), jumps: false });
  assert.deepEqual(
    noB.batches.map((b) => b.key),
    ["A#0"],
  );
  assert.equal(noB.segments, 2);
  const noPass = buildToolpath(p, doc, { ...all, hiddenPasses: new Set(["A#0"]) });
  assert.deepEqual(
    noPass.batches.map((b) => b.key),
    ["jump", "B#0"],
  );
});

test("buildToolpath: the draw-order animation stops at upTo and reports the head", () => {
  const p = previewOf([seg("jump", "A", [0, 0], [1, 0]), seg("mark", "A", [1, 0], [2, 0]), seg("mark", "A", [2, 0], [3, 0])]);
  const tp = buildToolpath(p, docOf(layer("A")), { ...all, upTo: 2 });
  assert.equal(tp.segments, 2);
  assert.deepEqual(tp.head, [2, 0]);
  assert.equal(buildToolpath(p, docOf(layer("A")), { ...all, upTo: 3 }).head, null);
});

test("buildToolpath: tiles keep every segment, bound them, and cull what is off screen", () => {
  // A 100 x 100 mm raster of 4000 short runs, enough for several tiles.
  const segs: PreviewSegment[] = [];
  for (let i = 0; i < 4000; i++) {
    const x = (i % 100) + 0.1;
    const y = Math.floor(i / 100) * 2.5;
    segs.push(seg("mark", "A", [x, y], [x + 0.5, y]));
  }
  const tp = buildToolpath(previewOf(segs), docOf(layer("A")), all);
  const b = tp.batches[0];
  assert.ok(tp.grid.cols > 1);
  assert.equal(b.coords.length, 4000 * 4);
  assert.equal(b.start[b.start.length - 1], 4000);
  // Same segments, reordered by tile.
  const key = (a: ArrayLike<number>, i: number) => `${a[4 * i].toFixed(3)},${a[4 * i + 1].toFixed(3)}`;
  const before = new Set(segs.map((s) => `${s.from[0].toFixed(3)},${s.from[1].toFixed(3)}`));
  const after = new Set(Array.from({ length: 4000 }, (_, i) => key(b.coords, i)));
  assert.deepEqual(after, before);
  // Each tile's box holds its segments.
  for (let t = 0; t + 1 < b.start.length; t++) {
    for (let i = b.start[t]; i < b.start[t + 1]; i++) {
      assert.ok(b.coords[4 * i] >= b.tileBox[4 * t] && b.coords[4 * i + 2] <= b.tileBox[4 * t + 2] + 1e-6);
      assert.ok(b.coords[4 * i + 1] >= b.tileBox[4 * t + 1] && b.coords[4 * i + 3] <= b.tileBox[4 * t + 3] + 1e-6);
    }
  }
  const everything = visibleTiles(b, { x0: -1, y0: -1, x1: 200, y1: 200 });
  assert.equal(everything.length, filledTiles(b));
  const corner = visibleTiles(b, { x0: 0, y0: 0, x1: 5, y1: 5 });
  assert.ok(corner.length >= 1 && corner.length < everything.length);
  assert.deepEqual(visibleTiles(b, { x0: 500, y0: 500, x1: 600, y1: 600 }), []);
});

test("buildToolpath: an empty or all-hidden preview has no batches", () => {
  const tp = buildToolpath(previewOf([]), null, all);
  assert.equal(tp.batches.length, 0);
  assert.equal(tp.segments, 0);
});

test("batchLook: dashed grey travel, marks in the pass colour, or shaded by energy", () => {
  const A = layer("A", { color: "#000000" });
  const st: ToolpathStyle = { layers: new Map([["A", A]]), theme: "light", shade: false, jumpColor: "#9aa0a8", shadeColor: "#111111" };
  assert.deepEqual(batchLook({ kind: "jump", layer: "", pass: null }, 1, st), { color: "#9aa0a8", alpha: 1, widthPx: 0.8, dash: [4, 3], caps: false });
  assert.deepEqual(batchLook({ kind: "mark", layer: "A", pass: 0 }, 1, st), { color: "#000000", alpha: 1, widthPx: 1.2, dash: null, caps: true });
  // Black on the dark field is lifted for contrast (canvasColor).
  assert.notEqual(batchLook({ kind: "mark", layer: "A", pass: 0 }, 1, { ...st, theme: "dark" }).color, "#000000");
  const shaded = batchLook({ kind: "mark", layer: "A", pass: 0 }, 1000, { ...st, shade: true });
  assert.equal(shaded.color, "#111111");
  assert.ok(shaded.alpha >= 0.25 && shaded.alpha <= 1);
  // A mark of an unknown layer stays visible.
  assert.equal(batchLook({ kind: "mark", layer: "?", pass: null }, 1, st).color, "#dd2222");
});

test("parseHexColor", () => {
  assert.deepEqual(parseHexColor("#ff0000"), [1, 0, 0, 1]);
  assert.deepEqual(parseHexColor("#0f0"), [0, 1, 0, 1]);
  assert.deepEqual(parseHexColor(" #00000080 ")?.map((v) => Math.round(v * 255)), [0, 0, 0, 128]);
  assert.equal(parseHexColor("red"), null);
  assert.equal(parseHexColor("#12345"), null);
});
