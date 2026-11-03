// The design-only build's backend worker (a dedicated Web Worker, one per tab): crates/webcore's wasm behind the
// worker shell (backend.ts), answering the page's transport (rpc.ts). The message loop is host.ts; this file only
// wires it to the worker global, IndexedDB, the wasm and the BroadcastChannel.

import { CHANNEL, type ChannelLike } from "./backend.ts";
import type { FromWorker, ToWorker } from "./contract.ts";
import { loadCore } from "./core.ts";
import { workerHost } from "./host.ts";
import { openStore } from "./idb.ts";

/** The worker scope (the web tsconfig has the DOM lib, not the webworker one). */
const scope = self as unknown as {
  postMessage(m: FromWorker, transfer?: Transferable[]): void;
  onmessage: ((e: MessageEvent<ToWorker>) => void) | null;
};

const handle = workerHost({
  post: (m, transfer = []) => scope.postMessage(m, transfer),
  openStore: () => openStore(),
  loadCore,
  channel: () => (typeof BroadcastChannel === "function" ? (new BroadcastChannel(CHANNEL) as unknown as ChannelLike) : null),
  every: (fn, ms) => void setInterval(fn, ms),
});

scope.onmessage = (e) => handle(e.data);
