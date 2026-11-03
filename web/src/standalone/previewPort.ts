// OpenEtch Design's preview "worker" (vite.config.ts puts this module in place of wasm/spawnPreviewWorker.ts): the
// preview client (wasm/previewClient.ts) talks to it as to its own preview worker, but every job runs in the backend
// worker (worker.ts), on the document the backend holds, with the design wasm it has already loaded. No second
// worker, no second wasm download, and no copy of the document sent across: the page's `context`, `doc` and
// `changes` messages are dropped, since the backend's document is the one the page shows.
//
// A job is answered for the revision it was asked for: the backend runs it in order with the requests, so it is of
// the document every earlier request left (never older than the page's).

import type { FromWorker, ToWorker } from "../wasm/protocol.ts";
import { standaloneClient } from "./rpc.ts";

class PreviewPort {
  onmessage: ((e: MessageEvent<FromWorker>) => void) | null = null;
  onerror: ((e: ErrorEvent) => void) | null = null;
  private alive = true;

  constructor() {
    void standaloneClient()
      .whenReady()
      .then((ok) => this.post(ok ? { type: "ready" } : { type: "failed", error: "the editor could not start" }));
  }

  postMessage(m: ToWorker): void {
    if (m.type !== "job") return;
    standaloneClient()
      .preview(m.kind, m.params)
      .then(
        (r) => this.post({ type: "done", id: m.id, kind: m.kind, rev: m.rev, json: r.json, arrays: r.arrays, ms: { doc: 0, job: r.ms } }),
        (e: unknown) => this.post({ type: "error", id: m.id, error: e instanceof Error ? e.message : String(e) }),
      );
  }

  terminate(): void {
    // The backend worker stays (it holds the document); a job still running there is only ignored.
    this.alive = false;
  }

  private post(data: FromWorker): void {
    if (this.alive) this.onmessage?.({ data } as MessageEvent<FromWorker>);
  }
}

export function spawnPreviewWorker(_name: string): Worker {
  return new PreviewPort() as unknown as Worker;
}
