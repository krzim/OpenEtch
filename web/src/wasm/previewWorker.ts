// The preview worker (a dedicated Web Worker): hosts crates/webcore as WebAssembly and plans the display preview, its
// stats and Fill Region hovers from the engine's document and planning context, off the page's main thread.
// Display only: nothing here talks to the engine or the laser (runs are planned by the engine from its own document).
//
// Messages arrive in order (protocol.ts). The document arrives whole or as the engine's change sets against the copy
// held (docSync.ts), and is parsed into the wasm core lazily, when a job needs a newer revision than it holds; the page
// cancels a stale job by terminating this worker (previewClient.ts).

import init, { Core } from "webcore-pkg";
import { applyChanges } from "../api/docSync.ts";
import type { GDocument } from "../api/types.ts";
import type { FromWorker, ToWorker } from "./protocol.ts";

/** The worker scope (the web tsconfig has the DOM lib, not the webworker one). */
const scope = self as unknown as { postMessage(m: unknown, transfer: Transferable[]): void; onmessage: ((e: MessageEvent<ToWorker>) => void) | null };

let core: Core | null = null;
/** The revision parsed into `core`. */
let docRev = -1;
/** Whether `held` changed since it was parsed (a whole document of the same revision counts: another engine run's). */
let fresh = false;
/** The page's document as of the last message (patched by change sets), parsed into `core` when a job needs it. */
let held: { rev: number; doc: GDocument } | null = null;
const queue: ToWorker[] = [];

function post(m: FromWorker, transfer: Transferable[] = []): void {
  scope.postMessage(m, transfer);
}

function handle(m: ToWorker): void {
  if (!core) {
    queue.push(m);
    return;
  }
  switch (m.type) {
    case "context":
      core.set_context(m.json);
      break;
    case "doc":
      // Kept until a job needs it: a burst of revisions parses only the last one.
      held = { rev: m.rev, doc: m.doc as GDocument };
      fresh = true;
      break;
    case "changes": {
      // Patch the copy held; a change set that does not fit drops it, and the next job reports the revision missing
      // (the page then restarts this worker and sends the whole document).
      let d: GDocument | null = held ? held.doc : null;
      for (const ch of m.changes) d = d ? applyChanges(d, ch) : null;
      held = d && d.revision === m.rev ? { rev: m.rev, doc: d } : null;
      fresh = true;
      break;
    }
    case "job": {
      try {
        const t0 = performance.now();
        if (held && fresh) {
          core.set_document(JSON.stringify(held.doc));
          docRev = held.rev;
          fresh = false;
        } else if (!held && fresh) {
          // A change set that did not fit: what core holds is not the page's document.
          docRev = -1;
          fresh = false;
        }
        const t1 = performance.now();
        if (docRev !== m.rev) {
          post({ type: "error", id: m.id, error: `the worker has revision ${docRev}, not ${m.rev}` });
          return;
        }
        if (m.kind === "full") {
          const p = core.preview();
          const json = p.meta();
          const arrays = { xy: p.take_xy(), t: p.take_t(), info: p.take_info(), pass: p.take_pass(), power: p.take_power() };
          p.free();
          const ms = { doc: t1 - t0, job: performance.now() - t1 };
          post({ type: "done", id: m.id, kind: m.kind, rev: m.rev, json, arrays, ms }, [arrays.xy.buffer, arrays.t.buffer, arrays.info.buffer, arrays.pass.buffer, arrays.power.buffer]);
        } else {
          const json = m.kind === "stats" ? core.stats() : core.fill_region_preview(JSON.stringify(m.params ?? {}));
          post({ type: "done", id: m.id, kind: m.kind, rev: m.rev, json, ms: { doc: t1 - t0, job: performance.now() - t1 } });
        }
      } catch (e) {
        post({ type: "error", id: m.id, error: e instanceof Error ? e.message : String(e) });
      }
      break;
    }
  }
}

scope.onmessage = (e) => handle(e.data);

init()
  .then(() => {
    core = new Core();
    post({ type: "ready" });
    for (const m of queue.splice(0)) handle(m);
  })
  .catch((e: unknown) => post({ type: "failed", error: e instanceof Error ? e.message : String(e) }));
