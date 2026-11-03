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

/** A binary preview as crates/engine/src/preview_bin.rs writes it. */
function binaryPreview(): ArrayBuffer {
  // Segments: jump (0,0)->(1,1); mark (1,1)->(2,1) power 40 (chained); jump (2,1)->(-5.5,7.25) (chained, pass 1).
  const xy = [0, 0, 1, 1, 2, 1, -5.5, 7.25];
  const t = [0.5, 1.5, 2.5];
  const power = [40];
  const flags = [0, 1 | 2 | 4 | 8, 8];
  const runs = [
    { pass: 0, layer: "layer-1", count: 2 },
    { pass: 1, layer: "layer-2", count: 1 },
  ];
  const build = (sections: Record<string, [number, number]>) =>
    new TextEncoder().encode(JSON.stringify({ format: "openetch-preview", version: 1, revision: 42, count: 3, runs, sections, stats: { segment_count: 1 }, passes: [], layers: [], warnings: [] }));
  let header = build({});
  let base = 0;
  for (let i = 0; i < 3; i++) {
    base = Math.ceil((12 + header.length) / 4) * 4;
    const xyAt = base;
    const tAt = xyAt + xy.length * 4;
    const pAt = tAt + t.length * 4;
    const fAt = pAt + power.length * 4;
    header = build({ xy: [xyAt, xy.length], t: [tAt, t.length], power: [pAt, power.length], flags: [fAt, flags.length] });
  }
  base = Math.ceil((12 + header.length) / 4) * 4;
  const buf = new ArrayBuffer(base + (xy.length + t.length + power.length) * 4 + flags.length);
  const v = new DataView(buf);
  new Uint8Array(buf).set([79, 69, 80, 86], 0);
  v.setUint32(4, 1, true);
  v.setUint32(8, header.length, true);
  new Uint8Array(buf).set(header, 12);
  let at = base;
  for (const f of [...xy, ...t, ...power]) {
    v.setFloat32(at, f, true);
    at += 4;
  }
  new Uint8Array(buf).set(flags, at);
  return buf;
}

test("the binary preview decodes into the JSON preview's shape", () => {
  const { preview, revision } = decodePreview(binaryPreview());
  assert.equal(revision, 42);
  assert.equal(preview.segments.length, 3);
  const [a, b, c] = preview.segments as (typeof preview.segments[number] & { emits?: boolean; power_pct?: number })[];
  assert.deepEqual([a.kind, a.from, a.to, a.layer, a.pass, a.t_s], ["jump", [0, 0], [1, 1], "layer-1", 0, 0.5]);
  assert.deepEqual([b.kind, b.from, b.to, b.power_pct, b.emits], ["mark", [1, 1], [2, 1], 40, true]);
  assert.deepEqual([c.kind, c.from, c.to, c.layer, c.pass], ["jump", [2, 1], [-5.5, 7.25], "layer-2", 1]);
  assert.deepEqual(preview.stats, { segment_count: 1 });
  assert.throws(() => decodePreview(new ArrayBuffer(16)));
});
