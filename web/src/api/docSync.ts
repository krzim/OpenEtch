// Keeping the browser's copy of the document current from the engine's change sets (crates/engine/src/docsync.rs),
// instead of fetching the whole document (megabytes for a large LightBurn import) after every edit.
//
// - `GET /api/document/sync?since=<revision>&epoch=<epoch>` answers a SyncPayload: `changes` from the revision this
//   browser holds, or the whole `document` when the engine cannot diff from it (first load, another engine run, a
//   revision it no longer keeps, another open document), plus the timeline and the project status of that revision.
// - Every POST to /api/ carries `X-OpenEtch-Since: <epoch>:<revision>` once a document is held (client.ts), and the
//   engine adds the same payload as `sync` to the answer, so an edit, undo, timeline move or import needs no second
//   request. client.ts hands every such payload to the listener useEngine registers here.
//
// The copy is for display and editing UI only: the engine plans every burn from its own document. Pure apart from
// the small module state below, so `npm test` covers it.

import type { GDocument } from "./types.ts";

/** The objects part of a change set. `order`, when present, is the complete new order of object ids. */
export interface ObjectChanges {
  upsert: GDocument["objects"];
  remove: string[];
  order?: string[];
}

/** The difference between revision `from` and revision `to` of one document. */
export interface DocChanges {
  from: number;
  to: number;
  /** Top-level members that changed (as GET /api/document has them). */
  set: Record<string, unknown>;
  /** Top-level members that are no longer sent. */
  unset: string[];
  objects: ObjectChanges;
}

/** GET /api/document/sync's answer, and the `sync` member of a mutating answer. */
export interface SyncPayload {
  epoch: string;
  document_id: string;
  revision: number;
  changes?: DocChanges;
  document?: GDocument;
  history: unknown;
  project: unknown;
}

/** Header a mutating request sends to get a `sync` member back. */
export const SYNC_HEADER = "X-OpenEtch-Since";

/** Whether `v` looks like a SyncPayload (an engine without change sets never sends one). */
export function isSyncPayload(v: unknown): v is SyncPayload {
  if (!v || typeof v !== "object") return false;
  const p = v as Partial<SyncPayload>;
  return typeof p.epoch === "string" && typeof p.revision === "number" && (isObject(p.changes) || isObject(p.document));
}

function isObject(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === "object" && !Array.isArray(v);
}

/**
 * `doc` with `ch` applied, or null when they do not fit together (the caller then fetches the whole document): `ch`
 * is not from `doc`'s revision, or names an object order with ids neither side has. Objects that did not change keep
 * their identity, so memoised drawing of them is reused.
 */
export function applyChanges(doc: GDocument, ch: DocChanges): GDocument | null {
  if (!ch || typeof ch.from !== "number" || ch.from !== doc.revision || !isObject(ch.set) || !Array.isArray(ch.unset) || !isObject(ch.objects)) {
    return null;
  }
  const next: Record<string, unknown> = { ...(doc as unknown as Record<string, unknown>) };
  for (const k of ch.unset) {
    if (k !== "objects") delete next[k];
  }
  for (const [k, v] of Object.entries(ch.set)) {
    if (k !== "objects") next[k] = v;
  }
  const upsert = new Map<string, GDocument["objects"][number]>();
  for (const o of Array.isArray(ch.objects.upsert) ? ch.objects.upsert : []) {
    if (o && typeof o.id === "string") upsert.set(o.id, o);
  }
  const removed = new Set(Array.isArray(ch.objects.remove) ? ch.objects.remove : []);
  let objects: GDocument["objects"];
  if (Array.isArray(ch.objects.order)) {
    const byId = new Map(doc.objects.map((o) => [o.id, o] as const));
    objects = [];
    for (const id of ch.objects.order) {
      const o = upsert.get(id) ?? (removed.has(id) ? undefined : byId.get(id));
      if (!o) return null;
      objects.push(o);
    }
  } else {
    // The id sequence is unchanged: every upsert replaces an object in place, and nothing was added or removed.
    const ids = new Set(doc.objects.map((o) => o.id));
    for (const id of upsert.keys()) if (!ids.has(id)) return null;
    objects = doc.objects.map((o) => upsert.get(o.id) ?? o);
  }
  next.objects = objects;
  next.revision = ch.to;
  return next as unknown as GDocument;
}

/** What a payload means for a browser holding `held` (its document, or null). */
export type SyncStep =
  | { kind: "replace"; doc: GDocument }
  | { kind: "apply"; doc: GDocument }
  /** Nothing new: the payload is of the revision held (or older). */
  | { kind: "same" }
  /** The changes do not start at the revision held: fetch again (with `since` = the held revision). */
  | { kind: "gap" };

/** Decide what to do with `p` given the held document and the epoch it came with. */
export function syncStep(held: GDocument | null, heldEpoch: string | null, p: SyncPayload): SyncStep {
  if (p.document && isObject(p.document)) {
    if (held && heldEpoch === p.epoch && p.revision < held.revision) return { kind: "same" };
    return { kind: "replace", doc: p.document };
  }
  if (!held || heldEpoch !== p.epoch || !p.changes) return { kind: "gap" };
  if (p.changes.to <= held.revision && p.changes.from !== held.revision) return { kind: "same" };
  if (p.changes.from === held.revision && p.changes.to === held.revision) return { kind: "same" };
  const doc = applyChanges(held, p.changes);
  return doc ? { kind: "apply", doc } : { kind: "gap" };
}

// --- module state: the sync point mutating requests announce, and the payload listener --------------------------

let point: { epoch: string; revision: number } | null = null;
let listener: ((p: SyncPayload) => void) | null = null;
let mutations = 0;

/** The revision (and engine run) this browser's document is at; null forgets it (no header is sent). */
export function setSyncPoint(epoch: string | null, revision: number | null): void {
  point = epoch !== null && revision !== null ? { epoch, revision } : null;
}

/** The `X-OpenEtch-Since` value, or null while no document is held. */
export function syncHeaderValue(): string | null {
  return point ? `${point.epoch}:${point.revision}` : null;
}

/** Register the one listener (useEngine); returns the unregister function. */
export function onSyncPayload(fn: (p: SyncPayload) => void): () => void {
  listener = fn;
  return () => {
    if (listener === fn) listener = null;
  };
}

/** A mutating request is about to be sent (client.ts). */
export function beginMutation(): void {
  mutations++;
}

/** A mutating request ended; `body` is its parsed answer, whose `sync` member (if any) goes to the listener. */
export function endMutation(body: unknown): void {
  mutations = Math.max(0, mutations - 1);
  const sync = isObject(body) ? body.sync : undefined;
  if (isSyncPayload(sync) && listener) {
    try {
      listener(sync);
    } catch {
      // A listener bug must not fail the request that carried the payload.
    }
  }
}

/** Mutating requests in flight (their answers will bring the changes, so an event need not fetch them). */
export function mutationsInFlight(): number {
  return mutations;
}
