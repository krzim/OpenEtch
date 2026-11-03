// Unit tests for how the preview worker gets the document (wasm/previewClient.ts `docMessage`): the engine's change
// sets when they lead from the revision the worker holds, the whole document otherwise.

import assert from "node:assert/strict";
import { test } from "node:test";
import { applyChanges, deltaOf, syncStep, type DocChanges } from "../api/docSync.ts";
import type { GDocument } from "../api/types.ts";
import { docMessage } from "./previewClient.ts";

function doc(revision: number, xs: number[]): GDocument {
  return {
    revision,
    name: "d",
    layers: [],
    objects: xs.map((x, i) => ({ id: `o${i}`, name: `o${i}`, layer: "layer-1", geometry: { type: "rect", width_mm: 1, height_mm: 1, corner_radius_mm: 0 }, transform: [1, 0, 0, 1, x, 0] })),
  } as unknown as GDocument;
}

function move(from: number, d: GDocument, i: number, x: number): DocChanges {
  const o = { ...d.objects[i], transform: [1, 0, 0, 1, x, 0] } as GDocument["objects"][number];
  return { from, to: from + 1, set: { revision: from + 1 }, unset: [], objects: { upsert: [o], remove: [] } };
}

test("a chain from the revision the worker holds is sent as change sets, and patches to the same document", () => {
  const d3 = doc(3, [0, 0]);
  const c1 = move(3, d3, 0, 5);
  const d4 = applyChanges(d3, c1)!;
  const c2 = move(4, d4, 1, 7);
  const d5 = applyChanges(d4, c2)!;
  const m = docMessage(3, [c1, c2], { rev: 5, doc: d5 });
  assert.equal(m.type, "changes");
  if (m.type !== "changes") return;
  // What the worker does with it (previewWorker.ts), on its own copy of revision 3.
  let w: GDocument | null = structuredClone(d3);
  for (const ch of m.changes) w = w ? applyChanges(w, ch) : null;
  assert.deepEqual(w, d5);
});

test("the whole document when the worker holds nothing, or the chain does not start or end where it must", () => {
  const d3 = doc(3, [0]);
  const c1 = move(3, d3, 0, 5);
  const d4 = applyChanges(d3, c1)!;
  assert.equal(docMessage(-1, [c1], { rev: 4, doc: d4 }).type, "doc");
  assert.equal(docMessage(2, [c1], { rev: 4, doc: d4 }).type, "doc");
  assert.equal(docMessage(3, [c1], { rev: 5, doc: d4 }).type, "doc");
  assert.equal(docMessage(3, [], { rev: 4, doc: d4 }).type, "doc");
});

test("syncStep remembers the change set an applied document was made with", () => {
  const d3 = doc(3, [0]);
  const c1 = move(3, d3, 0, 5);
  const step = syncStep(d3, "e", { epoch: "e", document_id: "x", revision: 4, changes: c1, history: null, project: null });
  assert.equal(step.kind, "apply");
  if (step.kind !== "apply") return;
  assert.equal(deltaOf(step.doc), c1);
  assert.equal(deltaOf(d3), undefined);
});
