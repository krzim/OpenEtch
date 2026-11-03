// OpenEtch Design picks its backend here (main.tsx calls `installDesignBackend` once, only in the design build): the
// backend worker of src/standalone (crates/webcore's design wasm behind the worker shell: IndexedDB storage,
// autosave, recovery, other tabs), spoken to through its page client (standalone/rpc.ts). Its notices become toasts
// (App.tsx, `onDesignNotice`). The device harness's design-engine build talks to a simulator engine instead
// (engineBackend.ts); Rollup drops that branch from the published build because DESIGN_BACKEND is a constant, and
// the normal build never includes this module (vite.config.ts puts noBackend.ts in its place).

import { DESIGN_BACKEND } from "../mode.ts";
import { setDesignBackend, type SocketLike } from "../api/transport.ts";

export async function installDesignBackend(): Promise<void> {
  if (DESIGN_BACKEND === "engine") {
    const { engineBackend } = await import("./engineBackend.ts");
    setDesignBackend(engineBackend());
    return;
  }
  const { onStandaloneNotice, standaloneEvents, standaloneFetch } = await import("../standalone/rpc.ts");
  setDesignBackend({
    request: (method, path, init) => standaloneFetch(method, path, init),
    // The shell's own event stream (it closes when the worker restarts, so useEngine fetches everything again).
    subscribe: () => () => {},
    openEvents: () => standaloneEvents() as unknown as SocketLike,
    notices: (listener) => onStandaloneNotice(listener),
  });
  // For the browser console and the device harness (e2e/design.mjs): the same requests the UI makes, nothing more.
  (globalThis as { __oeDesign?: unknown }).__oeDesign = {
    request: async (method: string, path: string, body?: unknown) => {
      const init: RequestInit = body === undefined ? {} : body instanceof Uint8Array ? { body } : { body: JSON.stringify(body), headers: { "content-type": "application/json" } };
      const r = await standaloneFetch(method, path, init);
      const type = r.headers.get("content-type") ?? "";
      return { status: r.status, type, body: type.includes("json") ? await r.json() : (await r.arrayBuffer()).byteLength };
    },
  };
}
