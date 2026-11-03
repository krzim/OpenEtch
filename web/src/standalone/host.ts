// The backend worker's message loop (contract.ts `ToWorker` / `FromWorker`), apart from the worker global so the
// page-side tests (rpc.test.ts) run the very same loop in Node. worker.ts wires it to `self`.
//
// Start: the page sends `init` (its session id and whether this is a fresh tab, a reload or a restart after a
// crash); the host opens the store (memory when IndexedDB is refused), loads webcore, loads the stores and fonts
// into it, restores the tab's document when asked, and posts `ready`. Requests that arrive earlier wait.

import { ShellBackend, type ChannelLike } from "./backend.ts";
import type { FromWorker, ToWorker, WebcoreBackend } from "./contract.ts";
import type { KeyValueStore } from "./idb.ts";

export interface HostDeps {
  post(m: FromWorker, transfer?: Transferable[]): void;
  openStore(): Promise<KeyValueStore>;
  loadCore(): Promise<WebcoreBackend>;
  channel(): ChannelLike | null;
  /** The autosave check (every 30 s in the worker). */
  every(fn: () => void, ms: number): void;
  now?: () => number;
}

/** Autosave check interval (the engine's too). */
export const TICK_MS = 30_000;

/** The bytes in an ArrayBuffer of their own, so it can be transferred without detaching anything shared. */
export function ownBytes(b: Uint8Array): Uint8Array {
  return b.byteOffset === 0 && b.byteLength === b.buffer.byteLength && b.buffer instanceof ArrayBuffer ? b : b.slice();
}

/** A message handler for one worker. */
export function workerHost(deps: HostDeps): (m: ToWorker) => void {
  let backend: ShellBackend | null = null;
  let started = false;
  const waiting: ToWorker[] = [];

  async function start(init: Extract<ToWorker, { kind: "init" }>): Promise<void> {
    let b: ShellBackend | null = null;
    try {
      const [store, core] = await Promise.all([deps.openStore(), deps.loadCore()]);
      const channel = deps.channel();
      b = new ShellBackend({
        core,
        store,
        sessionId: init.sessionId,
        channel,
        now: deps.now,
        onEvent: (data) => deps.post({ kind: "event", data }),
        // After the crashing request's own answer (a macrotask later), so the page shows that answer first.
        onCrash: (error) => {
          channel?.close();
          setTimeout(() => deps.post({ kind: "crashed", error }), 0);
        },
      });
      const recovered = await b.boot(init.boot, init.tzOffsetMin);
      if (b.isCrashed) return;
      backend = b;
      const live = b;
      deps.every(() => void live.tick(), TICK_MS);
      deps.post({ kind: "ready", recovered, storage: store.kind });
      for (const m of waiting.splice(0)) handle(m);
    } catch (e) {
      // webcore panicked while loading the stores: `crashed` is on its way and the page restarts this worker.
      if (b?.isCrashed) return;
      deps.post({ kind: "failed", error: e instanceof Error ? e.message : String(e) });
    }
  }

  function handle(m: ToWorker): void {
    if (m.kind === "init") {
      if (!started) {
        started = true;
        void start(m);
      }
      return;
    }
    if (!backend) {
      waiting.push(m);
      return;
    }
    if (m.kind === "flush") {
      void backend.flush();
      return;
    }
    if (m.kind === "preview") {
      const t0 = performance.now();
      const id = m.id;
      void backend.preview(m.job, m.params).then(
        (result) => {
          const transfer = result.arrays ? Object.values(result.arrays).map((a) => a.buffer as ArrayBuffer) : [];
          deps.post({ kind: "preview", id, result, ms: performance.now() - t0 }, transfer);
        },
        (e: unknown) => deps.post({ kind: "preview", id, error: e instanceof Error ? e.message : String(e), ms: performance.now() - t0 }),
      );
      return;
    }
    const { id, method, path, query, body } = m;
    const since = m.headers.find(([k]) => k.toLowerCase() === "x-openetch-since")?.[1];
    void backend.request({ method, path, query, body, since }).then((r) => {
      const b = ownBytes(r.body);
      deps.post({ kind: "reply", id, status: r.status, headers: r.headers, body: b }, [b.buffer as ArrayBuffer]);
    });
  }

  return handle;
}
