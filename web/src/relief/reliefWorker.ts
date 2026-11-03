// The 3D relief preview's worker (a dedicated Web Worker, like the preview worker in src/wasm/): it keeps the job's
// arrays and builds the height field (reliefModel.ts buildField) for each scrubber position, off the page's thread.
// Display only: it never talks to the engine.

import { buildField, type ReliefInput } from "./reliefModel.ts";
import type { FromRelief, ToRelief } from "./reliefProtocol.ts";

const scope = self as unknown as { postMessage(m: unknown, transfer: Transferable[]): void; onmessage: ((e: MessageEvent<ToRelief>) => void) | null };

let input: ReliefInput | null = null;
let include: boolean[] = [];
let maxSide = 320;
/** The newest build asked for; older ones still queued are skipped. */
let wanted = -1;

function post(m: FromRelief, transfer: Transferable[] = []): void {
  scope.postMessage(m, transfer);
}

scope.onmessage = (e) => {
  const m = e.data;
  if (m.type === "load") {
    input = m.input;
    include = m.include;
    maxSide = m.maxSide;
    return;
  }
  wanted = m.id;
  // Let a burst of scrubber moves arrive first; only the newest is built.
  setTimeout(() => {
    if (m.id !== wanted) return;
    if (!input) {
      post({ type: "error", id: m.id, error: "no job loaded" });
      return;
    }
    try {
      const t0 = performance.now();
      const f = buildField(input, include, m.upTo, maxSide);
      if (!f) {
        post({ type: "field", id: m.id, field: null, ms: performance.now() - t0 });
        return;
      }
      post({ type: "field", id: m.id, field: f, ms: performance.now() - t0 }, [f.depth.buffer]);
    } catch (err) {
      post({ type: "error", id: m.id, error: err instanceof Error ? err.message : String(err) });
    }
  }, 0);
};
