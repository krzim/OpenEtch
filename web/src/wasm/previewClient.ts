// The page's side of the preview worker (previewWorker.ts, crates/webcore as WebAssembly): one job at a time per
// worker, the newest request of each kind waiting behind it, and a job for an older document revision cancelled by
// terminating the worker (wasm cannot be interrupted) and starting a fresh one. Two workers: one plans previews and
// stats, the other answers Fill Region hovers, so a hover never waits behind a long plan.
//
// Results are display only. When WebAssembly or module workers are missing, or the worker cannot start, `ok()` turns
// false and the app uses the engine (GET /api/preview, `path.fill_region_preview`) as before.

import type { FromWorker, JobKind, ToWorker } from "./protocol.ts";

/** A finished job. */
export interface JobResult {
  rev: number;
  json: string;
  arrays?: { xy: Float32Array; t: Float32Array; info: Uint32Array; pass: Uint32Array; power: Float32Array };
  ms: { doc: number; job: number };
}

/** A job was overtaken by a newer revision (or its worker restarted): not an error to show. */
export class Stale extends Error {
  constructor() {
    super("stale");
    this.name = "Stale";
  }
}

interface Pending {
  id: number;
  kind: JobKind;
  rev: number;
  params?: unknown;
  resolve: (r: JobResult) => void;
  reject: (e: unknown) => void;
}

type State = "starting" | "ready" | "failed";

/** Whether this browser can run the worker at all (localStorage "openetch.previewWorker" = "off" turns it off). */
export function workerSupported(): boolean {
  return typeof Worker !== "undefined" && typeof WebAssembly === "object" && typeof WebAssembly.instantiate === "function" && !switchedOff();
}

function switchedOff(): boolean {
  try {
    return globalThis.localStorage?.getItem("openetch.previewWorker") === "off";
  } catch {
    return false;
  }
}

/** Activity of the workers, for the e2e performance profile (web/e2e/perf.mjs reads `window.__oeWorker`). */
interface Activity {
  busy: number;
  last: number;
  jobs: { kind: JobKind; rev: number; doc: number; job: number; wall: number; at: number }[];
}

function activity(): Activity | null {
  const w = globalThis as { __oeWorker?: Activity; window?: unknown };
  if (!w.window) return null;
  if (!w.__oeWorker) w.__oeWorker = { busy: 0, last: 0, jobs: [] };
  return w.__oeWorker;
}

export class WorkerCore {
  private worker: Worker | null = null;
  private state: State = "starting";
  private readyWaiters: ((ok: boolean) => void)[] = [];
  private ctx: string | null = null;
  private doc: { rev: number; doc: unknown } | null = null;
  /** The revision the current worker was sent. */
  private sentRev = -1;
  private inflight: (Pending & { started: number }) | null = null;
  private waiting = new Map<JobKind, Pending>();
  private nextId = 1;
  private failures = 0;
  private readonly name: string;

  constructor(name: string) {
    this.name = name;
  }

  /** False once the worker could not start (the app then asks the engine). */
  ok(): boolean {
    return this.state !== "failed" && workerSupported();
  }

  /** Resolves when the worker is ready (true) or could not start (false). */
  ready(): Promise<boolean> {
    if (!workerSupported()) return Promise.resolve(false);
    this.ensure();
    if (this.state === "ready") return Promise.resolve(true);
    if (this.state === "failed") return Promise.resolve(false);
    return new Promise((r) => this.readyWaiters.push(r));
  }

  setContext(json: string): void {
    if (this.ctx === json) return;
    this.ctx = json;
    if (this.worker) this.send({ type: "context", json });
  }

  /** The document the next jobs are for (only sent to the worker when a job needs it). */
  setDocument(rev: number, doc: unknown): void {
    if (this.doc && this.doc.rev === rev && this.doc.doc === doc) return;
    this.doc = { rev, doc };
  }

  /** Run `kind` on the current document. Rejects with `Stale` when a newer revision overtakes it. */
  run(kind: JobKind, params?: unknown): Promise<JobResult> {
    const doc = this.doc;
    if (!doc) return Promise.reject(new Error("no document yet"));
    if (!this.ok()) return Promise.reject(new Error("the preview worker is not available"));
    this.ensure();
    return new Promise<JobResult>((resolve, reject) => {
      const job: Pending = { id: this.nextId++, kind, rev: doc.rev, params, resolve, reject };
      // The newest request of a kind replaces the one waiting.
      this.waiting.get(kind)?.reject(new Stale());
      this.waiting.set(kind, job);
      // A running job for an older revision is cancelled: wasm cannot be interrupted, so the worker goes.
      if (this.inflight && this.inflight.rev < doc.rev) this.restart();
      this.pump();
    });
  }

  dispose(): void {
    this.worker?.terminate();
    this.worker = null;
    this.inflight?.reject(new Stale());
    this.inflight = null;
    for (const j of this.waiting.values()) j.reject(new Stale());
    this.waiting.clear();
  }

  private ensure(): void {
    if (this.worker || this.state === "failed" || !workerSupported()) return;
    let w: Worker;
    try {
      w = new Worker(new URL("./previewWorker.ts", import.meta.url), { type: "module", name: `openetch-${this.name}` });
    } catch {
      this.fail();
      return;
    }
    this.worker = w;
    this.state = "starting";
    this.sentRev = -1;
    w.onmessage = (e: MessageEvent<FromWorker>) => this.onMessage(w, e.data);
    w.onerror = (e) => {
      e.preventDefault();
      if (this.worker === w && this.state === "starting") this.fail();
    };
    if (this.ctx !== null) this.send({ type: "context", json: this.ctx });
  }

  private fail(): void {
    this.state = "failed";
    this.worker?.terminate();
    this.worker = null;
    for (const r of this.readyWaiters.splice(0)) r(false);
    this.inflight?.reject(new Error("the preview worker stopped"));
    this.inflight = null;
    for (const j of this.waiting.values()) j.reject(new Error("the preview worker stopped"));
    this.waiting.clear();
  }

  private restart(): void {
    this.worker?.terminate();
    this.worker = null;
    if (this.inflight) {
      this.inflight.reject(new Stale());
      this.inflight = null;
      const a = activity();
      if (a) a.busy = Math.max(0, a.busy - 1);
    }
    this.ensure();
  }

  private send(m: ToWorker): void {
    this.worker?.postMessage(m);
  }

  private pump(): void {
    if (this.inflight || !this.worker || this.state !== "ready") return;
    // Previews first, then stats, then Fill Region (each worker gets the kinds it is used for).
    const next = this.waiting.get("full") ?? this.waiting.get("stats") ?? this.waiting.get("fill");
    if (!next) return;
    this.waiting.delete(next.kind);
    const doc = this.doc;
    if (!doc || doc.rev !== next.rev) {
      next.reject(new Stale());
      this.pump();
      return;
    }
    if (this.sentRev !== doc.rev) {
      this.send({ type: "doc", rev: doc.rev, doc: doc.doc });
      this.sentRev = doc.rev;
    }
    this.inflight = { ...next, started: performance.now() };
    const a = activity();
    if (a) {
      a.busy++;
      a.last = Date.now();
    }
    this.send({ type: "job", id: next.id, kind: next.kind, rev: next.rev, params: next.params });
  }

  private onMessage(w: Worker, m: FromWorker): void {
    if (w !== this.worker) return;
    switch (m.type) {
      case "ready":
        this.state = "ready";
        for (const r of this.readyWaiters.splice(0)) r(true);
        this.pump();
        return;
      case "failed":
        this.fail();
        return;
      case "done":
      case "error": {
        const job = this.inflight;
        if (!job || job.id !== m.id) return;
        this.inflight = null;
        const a = activity();
        if (a) {
          a.busy = Math.max(0, a.busy - 1);
          a.last = Date.now();
        }
        if (m.type === "done") {
          this.failures = 0;
          if (a) {
            a.jobs.push({ kind: m.kind, rev: m.rev, doc: m.ms.doc, job: m.ms.job, wall: performance.now() - job.started, at: Date.now() });
            if (a.jobs.length > 200) a.jobs.splice(0, a.jobs.length - 200);
          }
          job.resolve({ rev: m.rev, json: m.json, arrays: m.arrays, ms: m.ms });
        } else {
          job.reject(new Error(m.error));
          // A trap (a panic in the wasm) leaves the instance unusable: start a fresh worker, and give up after a few.
          if (++this.failures >= 3) this.fail();
          else this.restart();
        }
        this.pump();
        return;
      }
    }
  }
}

/** Previews and stats. */
export const previewCore = new WorkerCore("preview");
/** Fill Region hovers. */
export const fillCore = new WorkerCore("fill");
