// Unit tests for api/docSync.ts (applying the engine's change sets) and api/previewBinary.ts (the binary preview).

import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument } from "./types.ts";
import {
  applyChanges,
  beginMutation,
  endMutation,
  isSyncPayload,
  mutationsInFlight,
  onSyncPayload,
  setSyncPoint,
  syncHeaderValue,
  syncStep,
  type DocChanges,
  type SyncPayload,
} from "./docSync.ts";
import { EMITS, MARK, segmentAt, segmentCount } from "./packedPreview.ts";
import { decodePreview } from "./previewBinary.ts";

function obj(id: string, x = 0): GDocument["objects"][number] {
  return { id, name: id, layer: "layer-1", geometry: { type: "rect", width_mm: 1, height_mm: 1, corner_radius_mm: 0 }, transform: [1, 0, 0, 1, x, 0] } as unknown as GDocument["objects"][number];
}

function doc(revision: number, ids: string[]): GDocument {
  return { revision, name: "d", stock: null, layers: [], objects: ids.map((id) => obj(id)), next_id: 9, rotary: { enabled: false } } as unknown as GDocument;
}

function changes(from: number, to: number, part: Partial<DocChanges> & { objects?: Partial<DocChanges["objects"]> } = {}): DocChanges {
  return {
    from,
    to,
    set: part.set ?? { revision: to },
    unset: part.unset ?? [],
    objects: { upsert: [], remove: [], ...(part.objects ?? {}) },
  };
}

test("an object moved in place keeps every other object's identity", () => {
  const d = doc(3, ["a", "b", "c"]);
  const moved = obj("b", 5);
  const next = applyChanges(d, changes(3, 4, { objects: { upsert: [moved] } }))!;
  assert.equal(next.revision, 4);
  assert.deepEqual(next.objects.map((o) => o.id), ["a", "b", "c"]);
  assert.equal(next.objects[1], moved);
  assert.equal(next.objects[0], d.objects[0]);
  assert.equal(next.objects[2], d.objects[2]);
  assert.equal(d.revision, 3, "the held document is not changed");
});

test("adds, removes, reorders and top-level members", () => {
  const d = doc(3, ["a", "b", "c"]);
  const next = applyChanges(
    d,
    changes(3, 5, { set: { revision: 5, next_id: 12, layers: [{ id: "layer-2" }] }, unset: ["rotary"], objects: { upsert: [obj("d")], remove: ["b"], order: ["c", "a", "d"] } }),
  )!;
  assert.deepEqual(next.objects.map((o) => o.id), ["c", "a", "d"]);
  assert.equal(next.next_id, 12);
  assert.deepEqual(next.layers, [{ id: "layer-2" }]);
  assert.equal("rotary" in next, false);
});

test("changes that do not fit the held document are refused", () => {
  const d = doc(3, ["a", "b"]);
  assert.equal(applyChanges(d, changes(2, 4)), null, "another starting revision");
  assert.equal(applyChanges(d, changes(3, 4, { objects: { order: ["a", "zz"] } })), null, "an unknown id in the order");
  assert.equal(applyChanges(d, changes(3, 4, { objects: { upsert: [obj("new")] } })), null, "an addition without an order");
});

function payload(p: Partial<SyncPayload>): SyncPayload {
  return { epoch: "e1", document_id: "default", revision: 4, history: { marker: 0, steps: [] }, project: null, ...p };
}

test("what a payload means for the document held", () => {
  const d = doc(3, ["a"]);
  assert.equal(syncStep(null, null, payload({ document: doc(4, []) })).kind, "replace");
  assert.equal(syncStep(d, "e1", payload({ changes: changes(3, 4) })).kind, "apply");
  assert.equal(syncStep(d, "e1", payload({ revision: 3, changes: changes(3, 3) })).kind, "same");
  assert.equal(syncStep(d, "e1", payload({ revision: 2, changes: changes(1, 2) })).kind, "same", "an older answer that arrived late");
  assert.equal(syncStep(d, "e1", payload({ revision: 6, changes: changes(5, 6) })).kind, "gap");
  assert.equal(syncStep(d, "e2", payload({ changes: changes(3, 4) })).kind, "gap", "another engine run");
  assert.equal(syncStep(d, "e1", payload({ revision: 2, document: doc(2, []) })).kind, "same");
  assert.equal(syncStep(d, "e2", payload({ revision: 2, document: doc(2, []) })).kind, "replace");
  assert.equal(isSyncPayload(payload({ changes: changes(3, 4) })), true);
  assert.equal(isSyncPayload({ ok: true }), false);
});

test("mutating answers hand their payload to the listener", () => {
  const got: SyncPayload[] = [];
  const off = onSyncPayload((p) => got.push(p));
  setSyncPoint("e1", 7);
  assert.equal(syncHeaderValue(), "e1:7");
  beginMutation();
  assert.equal(mutationsInFlight(), 1);
  endMutation({ ok: true, sync: payload({ changes: changes(7, 8) }) });
  endMutation({ ok: true });
  assert.equal(mutationsInFlight(), 0);
  assert.equal(got.length, 1);
  off();
  setSyncPoint(null, null);
  assert.equal(syncHeaderValue(), null);
});

/** A binary preview as crates/engine/src/preview_bin.rs writes it (version 1, or 2 with one raster). */
function binaryPreview(version = 1): ArrayBuffer {
  // Segments: jump (0,0)->(1,1); mark (1,1)->(2,1) power 40 (chained); jump (2,1)->(-5.5,7.25) (chained, pass 1);
  // version 2 adds two raster marks with a jump between them (no geometry) and a 2 x 2 raster over them.
  const xy = [0, 0, 1, 1, 2, 1, -5.5, 7.25];
  const t = version === 1 ? [0.5, 1.5, 2.5] : [0.5, 1.5, 2.5, 3, 3.5, 4];
  const power = [40];
  const flags = version === 1 ? [0, 1 | 2 | 4 | 8, 8] : [0, 1 | 2 | 4 | 8, 8, 1 | 2 | 16, 16, 1 | 2 | 16];
  const rowSegments = version === 1 ? [] : [3, 4, 5, 6];
  const rowSpan = version === 1 ? [] : [0, 1, 2, 1];
  const levels = version === 1 ? [] : [1, 1, 0, 1];
  const rasters = version === 1 ? undefined : [{ pass: 1, first: 3, end: 6, grid: [0.5, 0, 0, 0.5, 4, 4], cols: 2, rows: 2, powers: [30], pass_power_pct: 30, levels_at: 0, rows_at: 0 }];
  const runs = [
    { pass: 0, layer: "layer-1", count: 2 },
    { pass: 1, layer: "layer-2", count: t.length - 2 },
  ];
  const build = (sections: Record<string, [number, number]>) =>
    new TextEncoder().encode(JSON.stringify({ format: "openetch-preview", version, revision: 42, count: t.length, runs, rasters, sections, stats: { segment_count: 1 }, passes: [], layers: [], warnings: [] }));
  const words = [...xy, ...t, ...power];
  let header = build({});
  let base = 0;
  for (let i = 0; i < 3; i++) {
    base = Math.ceil((12 + header.length) / 4) * 4;
    const xyAt = base;
    const tAt = xyAt + xy.length * 4;
    const pAt = tAt + t.length * 4;
    const rAt = pAt + power.length * 4;
    const sAt = rAt + rowSegments.length * 4;
    const fAt = sAt + rowSpan.length * 4;
    const extra: Record<string, [number, number]> = version === 1 ? {} : { row_segments: [rAt, rowSegments.length], row_span: [sAt, rowSpan.length], levels: [fAt + flags.length, levels.length] };
    header = build({ xy: [xyAt, xy.length], t: [tAt, t.length], power: [pAt, power.length], flags: [fAt, flags.length], ...extra });
  }
  base = Math.ceil((12 + header.length) / 4) * 4;
  const buf = new ArrayBuffer(base + (words.length + rowSegments.length + rowSpan.length) * 4 + flags.length + levels.length);
  const v = new DataView(buf);
  new Uint8Array(buf).set([79, 69, 80, 86], 0);
  v.setUint32(4, version, true);
  v.setUint32(8, header.length, true);
  new Uint8Array(buf).set(header, 12);
  let at = base;
  for (const f of words) {
    v.setFloat32(at, f, true);
    at += 4;
  }
  for (const r of rowSegments) {
    v.setUint32(at, r, true);
    at += 4;
  }
  for (const s of rowSpan) {
    v.setFloat32(at, s, true);
    at += 4;
  }
  new Uint8Array(buf).set([...flags, ...levels], at);
  return buf;
}

test("the binary preview decodes into the packed form", () => {
  const { preview, revision } = decodePreview(binaryPreview());
  assert.equal(revision, 42);
  assert.equal(preview.segments.length, 0);
  const pk = preview.packed!;
  assert.equal(segmentCount(preview), 3);
  const [a, b, c] = [0, 1, 2].map((i) => segmentAt(pk, i));
  assert.deepEqual([a.kind, a.from, a.to, a.layer, a.pass, a.t_s], ["jump", [0, 0], [1, 1], "layer-1", 0, 0.5]);
  assert.deepEqual([b.kind, b.from, b.to, pk.power[1], pk.info[1] & EMITS], ["mark", [1, 1], [2, 1], 40, EMITS]);
  assert.ok(Number.isNaN(pk.power[0]), "no power of its own");
  assert.deepEqual([c.kind, c.from, c.to, c.layer, c.pass], ["jump", [2, 1], [-5.5, 7.25], "layer-2", 1]);
  assert.deepEqual(preview.stats, { segment_count: 1 });
  assert.deepEqual(preview.rasters, []);
  assert.throws(() => decodePreview(new ArrayBuffer(16)));
});

test("a version 2 binary preview brings its rasters and leaves their segments without geometry", () => {
  const { preview } = decodePreview(binaryPreview(2));
  const pk = preview.packed!;
  assert.equal(segmentCount(preview), 6);
  assert.deepEqual([...pk.t].slice(3), [3, 3.5, 4]);
  assert.ok([...pk.xy.subarray(12)].every(Number.isNaN), "raster segments carry no geometry");
  assert.equal(pk.info[3] & MARK, MARK);
  assert.equal(pk.pass[4], 1);
  const [r] = preview.rasters ?? [];
  assert.deepEqual([r.first, r.end, r.cols, r.rows, [...r.levels], [...r.rowSegments], [...r.rowSpan], r.powers], [3, 6, 2, 2, [1, 1, 0, 1], [3, 4, 5, 6], [0, 1, 2, 1], [30]]);
});
