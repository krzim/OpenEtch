// Unit tests for api/packedPreview.ts: the browser preview's flat arrays read the same as the engine's JSON segments
// in every consumer (toolpath layer, timeline, layer counts).

import assert from "node:assert/strict";
import { test } from "node:test";
import { segmentTimes, startHereSegment } from "./job.ts";
import { previewLayers } from "./laser.ts";
import {
  NO_PASS,
  estimateOf,
  isMark,
  layerOf,
  levelShades,
  packSegments,
  previewFromWorker,
  rasterHead,
  rasterPoint,
  rasterReveal,
  segmentAt,
  segmentCount,
  segmentLength,
} from "./packedPreview.ts";
import type { Preview, PreviewSegment } from "./types.ts";
import { buildToolpath } from "../sceneLayers.ts";

const SEGS: PreviewSegment[] = [
  { kind: "jump", layer: null, from: [0, 0], to: [1, 0], pass: 0, t_s: 0.1 },
  { kind: "mark", layer: "L1", from: [1, 0], to: [4, 4], pass: 0, t_s: 0.5 },
  { kind: "mark", layer: "L2", from: [4, 4], to: [4, 8], pass: 1, t_s: 0.9 },
  { kind: "jump", layer: null, from: [4, 8], to: [0, 0], t_s: 1.2 },
];
const PASSES = [
  { layer_id: "L1", sub_layer: 0, repeat: 0, segments: 2 },
  { layer_id: "L2", sub_layer: 1, repeat: 0, segments: 1 },
] as Preview["passes"];

function both(): [Preview, Preview] {
  const json: Preview = { segments: SEGS, passes: PASSES, stats: { estimated_time_s: 1.2 } };
  const pk = packSegments(SEGS);
  const meta = JSON.stringify({ passes: PASSES, stats: { estimated_time_s: 1.2 }, layer_ids: pk.layerIds, segment_count: SEGS.length });
  const w = previewFromWorker(meta, pk);
  assert.ok(w.preview);
  return [json, w.preview];
}

test("packed segments read back as the JSON ones", () => {
  const pk = packSegments(SEGS);
  assert.equal(pk.t.length, 4);
  assert.deepEqual(pk.layerIds, ["L1", "L2"]);
  assert.equal(isMark(pk, 0), false);
  assert.equal(isMark(pk, 1), true);
  assert.equal(layerOf(pk, 2), "L2");
  assert.equal(layerOf(pk, 3), null);
  assert.equal(pk.pass[3], NO_PASS);
  const s = segmentAt(pk, 1);
  assert.equal(s.kind, "mark");
  assert.deepEqual(s.to, [4, 4]);
  assert.equal(s.pass, 0);
});

test("worker answers: preview, summary only, and errors", () => {
  const [, p] = both();
  assert.equal(segmentCount(p), 4);
  assert.deepEqual(p.segments, []);
  const stats = previewFromWorker(JSON.stringify({ stats: { estimated_time_s: 42 }, segment_count: 9 }));
  assert.equal(stats.preview, null);
  assert.equal(estimateOf(stats.summary), 42);
  assert.equal(stats.summary?.segment_count, 9);
  const err = previewFromWorker(JSON.stringify({ error: "out of the field" }));
  assert.equal(err.error, "out of the field");
  assert.equal(estimateOf(null), null);
  assert.equal(previewFromWorker("{").error !== null, true);
});

test("timeline, layer counts and the toolpath agree between the two forms", () => {
  const [json, packed] = both();
  assert.deepEqual(Array.from(segmentTimes(packed)), Array.from(segmentTimes(json)).map((t) => Math.fround(t)));
  assert.equal(startHereSegment(packed, 2), startHereSegment(json, 2));
  assert.equal(startHereSegment(packed, 4), null);
  const doc = null;
  const rows = (p: Preview) => previewLayers(p, doc).map((r) => [r.id, r.segments, Math.round(r.markLengthMm * 1000)]);
  assert.deepEqual(rows(packed), rows(json));
  const opts = { upTo: 3, jumps: true, hidden: new Set<string>(), hiddenPasses: new Set<string>() };
  const a = buildToolpath(json, doc, opts);
  const b = buildToolpath(packed, doc, opts);
  assert.equal(b.segments, a.segments);
  assert.deepEqual(b.head, a.head);
  assert.deepEqual(
    b.batches.map((x) => [x.key, x.kind, x.pass]),
    a.batches.map((x) => [x.key, x.kind, x.pass]),
  );
});

// A 3 x 2 raster over segments 1..6 of pass 0 (layer L1): row 0 is mark 1, jump 2, mark 3 (left to right), row 1 is
// mark 5 (right to left) after jump 4; then a vector mark 6 of pass 1 (L2). Bidirectional, as an image pass plans.
function rasterPreview(): Preview {
  const segs: PreviewSegment[] = [
    { kind: "jump", layer: "L1", from: [0, 0], to: [0, 0.5], pass: 0, t_s: 0.1 },
    { kind: "mark", layer: "L1", from: [0, 0.5], to: [1, 0.5], pass: 0, t_s: 0.2 },
    { kind: "jump", layer: "L1", from: [1, 0.5], to: [2, 0.5], pass: 0, t_s: 0.3 },
    { kind: "mark", layer: "L1", from: [2, 0.5], to: [3, 0.5], pass: 0, t_s: 0.4 },
    { kind: "jump", layer: "L1", from: [3, 0.5], to: [3, 1.5], pass: 0, t_s: 0.5 },
    { kind: "mark", layer: "L1", from: [3, 1.5], to: [0, 1.5], pass: 0, t_s: 0.9 },
    { kind: "mark", layer: "L2", from: [5, 5], to: [6, 6], pass: 1, t_s: 1.0 },
  ];
  const pk = packSegments(segs);
  for (let i = 1; i < 6; i++) pk.xy.fill(NaN, 4 * i, 4 * i + 4);
  const headers = [{ pass: 0, first: 1, end: 6, grid: [1, 0, 0, 1, 0, 0], cols: 3, rows: 2, powers: [50], pass_power_pct: 50, levels_at: 0, rows_at: 0 }];
  const meta = JSON.stringify({ passes: PASSES, stats: {}, layer_ids: pk.layerIds, segment_count: segs.length, rasters: headers });
  const w = previewFromWorker(meta, {
    ...pk,
    levels: new Uint8Array([1, 0, 1, 1, 1, 1]),
    rowSegments: new Uint32Array([1, 4, 5, 6]),
    rowSpan: new Float32Array([0, 3, 3, 0]),
  });
  assert.ok(w.preview, String(w.error));
  return w.preview;
}

test("rasters come with the worker's preview and reveal by scan line", () => {
  const p = rasterPreview();
  const [r] = p.rasters ?? [];
  assert.ok(r);
  assert.deepEqual([r.cols, r.rows, [...r.levels], r.passPowerPct], [3, 2, [1, 0, 1, 1, 1, 1], 50]);
  assert.deepEqual(rasterReveal(r, 1), { rows: 0, partial: null }, "nothing before its first run");
  assert.deepEqual(rasterReveal(r, 2), { rows: 0, partial: { from: 0, to: 1 } }, "the first run of row 0");
  assert.deepEqual(rasterReveal(r, 4), { rows: 1, partial: null }, "row 0 done, on the jump up");
  const mid = rasterReveal(r, 5.5);
  assert.deepEqual([mid.rows, mid.partial?.from, mid.partial?.to], [1, 3, 1.5], "half of row 1, backwards");
  assert.deepEqual(rasterReveal(r, 6), { rows: 2, partial: null });
  assert.deepEqual(rasterHead(r, 2), [1, 0.5]);
  assert.deepEqual(rasterHead(r, 4), [3, 0.5], "row 0 done: at its end");
  assert.deepEqual(rasterHead(r, 5), [3, 1.5], "the jump up shown: at row 1's start, where the jump ends");
  assert.deepEqual(rasterHead(r, 6), [0, 1.5], "the end of the last row");
  assert.equal(rasterHead(r, 7), null);
  assert.deepEqual(rasterPoint(r, 1, 2), [1, 2]);
  const lut = levelShades({ ...r, powers: [50, 25] });
  assert.deepEqual([lut[0], lut[1], lut[2]], [0, 255, Math.round(255 * (0.15 + 0.85 * 0.5))]);
  // Consumers count raster segments without geometry: no NaN lengths.
  assert.equal(segmentLength(p.packed!, 3), 0);
  assert.ok(previewLayers(p, null).every((l) => Number.isFinite(l.markLengthMm)));
  // A header the arrays cannot hold is an error, not a crash later.
  const bad = previewFromWorker(JSON.stringify({ rasters: [{ pass: 0, first: 0, end: 1, grid: [1, 0, 0, 1, 0, 0], cols: 9, rows: 9, powers: [], pass_power_pct: 1, levels_at: 0, rows_at: 0 }] }), packSegments([]));
  assert.ok(bad.error && !bad.preview);
});

test("buildToolpath skips a raster's segments and reveals it with the slider", () => {
  const p = rasterPreview();
  const all = { hidden: new Set<string>(), hiddenPasses: new Set<string>(), upTo: null, jumps: true };
  const tp = buildToolpath(p, null, all);
  assert.equal(tp.rasters.length, 1);
  assert.deepEqual([tp.rasters[0].key, tp.rasters[0].rows, tp.rasters[0].partial], ["L1#0", 2, null]);
  // Only the jump in and the vector mark are segments.
  const n = tp.batches.reduce((s, b) => s + b.coords.length / 4, 0);
  assert.equal(n, 2);
  assert.ok(tp.batches.every((b) => [...b.coords].every(Number.isFinite)));
  const half = buildToolpath(p, null, { ...all, upTo: 5 });
  assert.deepEqual([half.rasters[0].rows, half.rasters[0].partial], [1, null], "row 0, on the jump to row 1");
  assert.deepEqual(half.head, [3, 1.5], "the head is where the jump up ends");
  assert.equal(buildToolpath(p, null, { ...all, upTo: 1 }).rasters.length, 0, "not reached yet");
  assert.equal(buildToolpath(p, null, { ...all, hidden: new Set(["L1"]) }).rasters.length, 0, "hidden with its layer");
});
