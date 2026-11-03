// Unit tests for how the preview worker gets the document (wasm/previewClient.ts `docMessage`): the engine's change
// sets when they lead from the revision the worker holds, the whole document otherwise.

import assert from "node:assert/strict";
import { test } from "node:test";
import { applyChanges, deltaBaseOf, deltaOf, syncStep, type DocChanges } from "../api/docSync.ts";
import type { GDocument } from "../api/types.ts";
import { docMessage, Stale, WorkerCore } from "./previewClient.ts";

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

// ---- WorkerCore with a stand-in Worker: what reaches the worker, by document rather than by revision -------------

class FakeWorker {
  static all: FakeWorker[] = [];
  sent: { type: string; [k: string]: unknown }[] = [];
  terminated = false;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onerror: ((e: unknown) => void) | null = null;
  constructor() {
    FakeWorker.all.push(this);
  }
  postMessage(m: { type: string }): void {
    this.sent.push(m);
  }
  terminate(): void {
    this.terminated = true;
  }
  /** The worker says it is ready. */
  ready(): void {
    this.onmessage?.({ data: { type: "ready" } });
  }
  /** Finish the last job sent, as the worker would. */
  finish(): void {
    const job = [...this.sent].reverse().find((m) => m.type === "job") as { id: number; kind: string; rev: number };
    this.sent = this.sent.filter((m) => m.type !== "job");
    this.onmessage?.({ data: { type: "done", id: job.id, kind: job.kind, rev: job.rev, json: "{}", ms: { doc: 0, job: 0 } } });
  }
  /** The document messages sent so far (`doc` whole or `changes`), taken off the log. */
  take(): string[] {
    const out = this.sent.filter((m) => m.type === "doc" || m.type === "changes").map((m) => m.type);
    this.sent = this.sent.filter((m) => m.type === "job");
    return out;
  }
}

function withFakeWorker(fn: () => Promise<void>): () => Promise<void> {
  return async () => {
    const g = globalThis as { Worker?: unknown };
    const before = g.Worker;
    g.Worker = FakeWorker;
    FakeWorker.all = [];
    try {
      await fn();
    } finally {
      g.Worker = before;
    }
  };
}

const sync = (held: GDocument, epoch: string, ch: DocChanges): GDocument => {
  const s = syncStep(held, epoch, { epoch, document_id: "default", revision: ch.to, changes: ch, history: null, project: null });
  assert.equal(s.kind, "apply");
  return (s as { doc: GDocument }).doc;
};

test(
  "a whole document of the revision the worker holds (a restarted engine) is sent again, never patched over",
  withFakeWorker(async () => {
    const core = new WorkerCore("test");
    const d3 = doc(3, [0, 0]);
    core.setDocument(3, d3);
    const first = core.run("full");
    const w = FakeWorker.all[0];
    w.ready();
    assert.deepEqual(w.take(), ["doc"]);
    w.finish();
    await first;

    // An edit: the change set travels.
    const d4 = sync(d3, "run-1", move(3, d3, 0, 5));
    core.setDocument(4, d4, deltaOf(d4), deltaBaseOf(d4));
    const second = core.run("full");
    assert.deepEqual(w.take(), ["changes"]);
    w.finish();
    await second;

    // The engine restarted and is at revision 4 too, with another document: it goes whole.
    const other = doc(4, [9, 9, 9]);
    core.setDocument(4, other);
    const third = core.run("full");
    const sent = w.sent.find((m) => m.type === "doc") as { doc: unknown } | undefined;
    assert.deepEqual(w.take(), ["doc"]);
    assert.equal(sent?.doc, other);
    w.finish();
    await third;

    // A change set from revision 4 of the old run's document does not patch the new one.
    const stale = sync(d4, "run-1", move(4, d4, 1, 2));
    core.setDocument(5, stale, deltaOf(stale), deltaBaseOf(stale));
    const fourth = core.run("full");
    assert.deepEqual(w.take(), ["doc"]);
    w.finish();
    await fourth;
  }),
);

test(
  "a job for a replaced document of the same revision is cancelled",
  withFakeWorker(async () => {
    const core = new WorkerCore("test");
    core.setDocument(7, doc(7, [0]));
    const old = core.run("full");
    FakeWorker.all[0].ready();
    core.setDocument(7, doc(7, [1, 2]));
    const next = core.run("full");
    await assert.rejects(old, Stale);
    assert.equal(FakeWorker.all[0].terminated, true, "the busy worker was replaced");
    const w = FakeWorker.all[1];
    w.ready();
    assert.deepEqual(w.take(), ["doc"]);
    w.finish();
    await next;
  }),
);
