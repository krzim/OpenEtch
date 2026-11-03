// Unit tests for api/packedPreview.ts: the browser preview's flat arrays read the same as the engine's JSON segments
// in every consumer (toolpath layer, timeline, layer counts).

import assert from "node:assert/strict";
import { test } from "node:test";
import { segmentTimes, startHereSegment } from "./job.ts";
import { previewLayers } from "./laser.ts";
import { NO_PASS, estimateOf, isMark, layerOf, packSegments, previewFromWorker, segmentAt, segmentCount } from "./packedPreview.ts";
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
