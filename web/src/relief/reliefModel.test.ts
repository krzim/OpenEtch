// The 3D relief model: which passes are relief, the height field from image rasters and vector marks, the scrubber's
// partial field, normals and the starting exaggeration.

import assert from "node:assert/strict";
import { test } from "node:test";
import { NO_PASS, packSegments, type PreviewRaster } from "../api/packedPreview.ts";
import {
  autoExaggeration,
  buildField,
  depthText,
  fieldNormals,
  includedPasses,
  rasterRowsDone,
  reliefPasses,
  settingsKind,
  type ReliefInput,
} from "./reliefModel.ts";

const layers = [
  { id: "coin", settings: { image: { dither: "slice3d" }, passes: 3 }, sub_layers: [{ cleanup: { every_n_passes: 1 } }] },
  { id: "photo", settings: { image: { dither: "grayscale" } }, sub_layers: [] },
  { id: "deep", settings: { mode: "fill", options: { z_step_mm: -0.02 } }, sub_layers: [] },
  { id: "outline", settings: { mode: "line" }, sub_layers: [] },
];

test("relief passes: 3D Sliced (with its cleanup), grayscale, Z step; the rest only when nothing else", () => {
  assert.deepEqual(settingsKind({ image: { dither: "3dslice" } }), { kind: "slice3d", zStepMm: 0 });
  assert.deepEqual(settingsKind({}), { kind: "other", zStepMm: 0 });
  const passes = reliefPasses(
    [
      { layer_id: "coin", sub_layer: 0 },
      { layer_id: "coin", sub_layer: 1 },
      { layer_id: "photo", sub_layer: 0 },
      { layer_id: "deep", sub_layer: 0 },
      { layer_id: "outline", sub_layer: 0 },
      { layer_id: "gone", sub_layer: 0 },
    ],
    layers,
    0.01,
  );
  assert.deepEqual(
    passes.map((p) => [p.kind, p.depthMm]),
    [
      ["slice3d", 0.01],
      ["slice3d", 0.01],
      ["grayscale", 0.01],
      ["zstep", 0.02],
      ["other", 0.01],
      ["other", 0.01],
    ],
  );
  assert.deepEqual(includedPasses(passes, true), { include: [true, true, true, true, false, false], fallback: false });
  assert.deepEqual(includedPasses(passes, false).include, [true, true, true, true, true, true]);
  const plain = reliefPasses([{ layer_id: "outline", sub_layer: 0 }], layers, 0.01);
  assert.deepEqual(includedPasses(plain, true), { include: [true], fallback: true });
});

/** A raster of `cols` x `rows` dots of 0.1 mm at the origin, every dot at level `lv[k]` (row-major). */
function raster(pass: number, cols: number, rows: number, lv: number[], first: number): PreviewRaster {
  const rowSegments = new Uint32Array(rows * 2);
  for (let r = 0; r < rows; r++) {
    rowSegments[2 * r] = first + r;
    rowSegments[2 * r + 1] = first + r + 1;
  }
  return {
    pass,
    first,
    end: first + rows,
    grid: [0.1, 0, 0, 0.1, 0, 0],
    cols,
    rows,
    levels: Uint8Array.from(lv),
    powers: [50, 25],
    passPowerPct: 50,
    rowSegments,
    rowSpan: new Float32Array(rows * 2),
  };
}

function input(rasters: PreviewRaster[], segs: Parameters<typeof packSegments>[0], depths: number[]): ReliefInput {
  const pk = packSegments(segs);
  return { xy: pk.xy, info: pk.info, pass: pk.pass, power: pk.power, rasters, passes: depths.map((d) => ({ layerId: "l", kind: "slice3d", depthMm: d })) };
}

test("raster dots add depth by pass and power; the scrubber shows whole rows done", () => {
  // Two passes over a 4 x 2 raster: pass 0 marks the left half, pass 1 every dot; level 2 is half power.
  const segs = Array.from({ length: 4 }, (_, i) => ({ kind: "mark" as const, from: [NaN, NaN] as [number, number], to: [NaN, NaN] as [number, number], pass: i < 2 ? 0 : 1, emits: true, t_s: i }));
  const r0 = raster(0, 4, 2, [1, 1, 0, 0, 1, 1, 0, 0], 0);
  const r1 = raster(1, 4, 2, [1, 1, 1, 2, 1, 1, 1, 2], 2);
  const inp = input([r0, r1], segs, [0.01, 0.01]);
  const f = buildField(inp, [true, true], null, 400);
  assert.ok(f);
  // Cells are at least a dot wide: one dot per cell, so depths are whole passes.
  assert.ok(Math.abs(f.cell - 0.1) < 1e-9, `${f.cell}`);
  const at = (x: number, y: number) => f.depth[Math.floor((y - (f.y0 - f.cell / 2)) / f.cell) * f.cols + Math.floor((x - (f.x0 - f.cell / 2)) / f.cell)];
  assert.ok(Math.abs(at(0.05, 0.05) - 0.02) < 1e-6, "two passes on the left");
  assert.ok(Math.abs(at(0.25, 0.05) - 0.01) < 1e-6, "one pass in the middle");
  assert.ok(Math.abs(at(0.35, 0.15) - 0.005) < 1e-6, "half power, half depth");
  assert.ok(Math.abs(f.maxDepth - 0.02) < 1e-6);
  assert.equal(at(-0.1, 0.05), 0, "the margin stays flat");
  // Up to segment 3: pass 0 done, pass 1's first row only.
  assert.equal(rasterRowsDone(r1, 3), 1);
  assert.equal(rasterRowsDone(r1, 2), 0);
  assert.equal(rasterRowsDone(r1, null), 2);
  const part = buildField(inp, [true, true], 3, 400);
  assert.ok(part);
  assert.deepEqual([part.cols, part.rows, part.x0, part.y0], [f.cols, f.rows, f.x0, f.y0], "the frame does not move while scrubbing");
  const pat = (x: number, y: number) => part.depth[Math.floor((y - (part.y0 - part.cell / 2)) / part.cell) * part.cols + Math.floor((x - (part.x0 - part.cell / 2)) / part.cell)];
  assert.ok(Math.abs(pat(0.05, 0.05) - 0.02) < 1e-6);
  assert.ok(Math.abs(pat(0.05, 0.15) - 0.01) < 1e-6, "the second row of pass 1 is not done yet");
  // A grid coarser than the dots and not aligned to them: an evenly marked image stays even (no spikes where a cell
  // happens to hold two dots instead of one).
  const wide = raster(0, 40, 40, new Array(1600).fill(1), 0);
  const even = buildField(input([wide], [{ kind: "mark", from: [NaN, NaN], to: [NaN, NaN], pass: 0, emits: true, t_s: 0 }], [0.01]), [true], null, 31);
  assert.ok(even && even.cell > 0.1);
  const inside = [...even.depth].filter((v) => v > 0);
  assert.ok(Math.max(...inside) <= 0.01 + 1e-6, `max ${Math.max(...inside)}`);
  // Leaving pass 1 out.
  const only0 = buildField(inp, [true, false], null, 400);
  assert.ok(only0 && Math.abs(only0.maxDepth - 0.01) < 1e-6);
});

test("vector marks deepen every cell they cross once per pass, however close the hatch", () => {
  // Pass 0: 11 hatch lines 0.01 mm apart across a 1 mm square (all in one cell row band); pass 1: the same again.
  const segs = [];
  for (const pass of [0, 1])
    for (let k = 0; k <= 10; k++) segs.push({ kind: "mark" as const, from: [0, k * 0.01] as [number, number], to: [1, k * 0.01] as [number, number], pass, emits: true, t_s: segs.length });
  segs.push({ kind: "jump" as const, from: [1, 0] as [number, number], to: [0, 0] as [number, number], pass: 1, emits: false, t_s: 99 });
  const inp = input([], segs, [0.02, 0.02]);
  const f = buildField(inp, [true, true], null, 40);
  assert.ok(f);
  assert.ok(Math.abs(f.maxDepth - 0.04) < 1e-6, `${f.maxDepth}`);
  const half = buildField(inp, [true, true], 11, 40);
  assert.ok(half && Math.abs(half.maxDepth - 0.02) < 1e-6, "only the first pass by segment 11");
  // Segments without a pass, and jumps, never mark.
  const none = buildField(input([], [{ kind: "mark", from: [0, 0], to: [1, 0], emits: true, t_s: 0 }], [0.01]), [true], null, 40);
  assert.equal(none, null);
  assert.equal(packSegments([{ kind: "mark", from: [0, 0], to: [1, 0], t_s: 0 }]).pass[0], NO_PASS);
});

test("normals, exaggeration and depth texts", () => {
  const f = { cols: 3, rows: 3, x0: 0, y0: 0, cell: 1, depth: new Float32Array(9), maxDepth: 0 };
  const n = fieldNormals(f, 10);
  assert.deepEqual([n[12], n[13], n[14]], [0, 0, 1], "flat is straight up");
  f.depth[5] = 0.1; // (2, 1) deeper: the surface falls towards +X at the centre
  const m = fieldNormals({ ...f, maxDepth: 0.1 }, 10);
  assert.ok(m[12] > 0, "the normal leans towards the deeper side");
  assert.equal(autoExaggeration({ ...f, maxDepth: 0 }), 1);
  assert.equal(autoExaggeration({ ...f, cols: 300, rows: 300, cell: 0.1, maxDepth: 0.1 }), 50);
  assert.equal(depthText(0), "0 mm");
  assert.equal(depthText(0.035), "35 µm");
  assert.equal(depthText(0.25), "0.25 mm");
});
