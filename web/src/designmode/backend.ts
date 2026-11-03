// OpenEtch Design picks its backend here (main.tsx calls `installDesignBackend` once, only in the design build):
// the in-browser worker, spoken to over api/transport.ts `workerBackend`. Until the shell track's worker
// (src/standalone/worker.ts, crates/webcore in wasm) lands, a small in-memory stand-in (mockWorker.ts) answers. The
// shell's own page client plugs in the same way: setDesignBackend({ request: standaloneFetch, subscribe: () => () =>
// {}, openEvents: standaloneEvents }), its notices shown as toasts. The
// device harness's design-engine build talks to a simulator engine instead (engineBackend.ts); Rollup drops that
// branch from the published build because DESIGN_BACKEND is a constant.

import { DESIGN_BACKEND } from "../mode.ts";
import { setDesignBackend, workerBackend } from "../api/transport.ts";

export async function installDesignBackend(): Promise<void> {
  if (DESIGN_BACKEND === "engine") {
    const { engineBackend } = await import("./engineBackend.ts");
    setDesignBackend(engineBackend());
    return;
  }
  const worker = new Worker(new URL("./mockWorker.ts", import.meta.url), { type: "module", name: "openetch-design" });
  setDesignBackend(workerBackend(worker, (message) => console.error(`[design] the backend stopped: ${message}`)));
}
