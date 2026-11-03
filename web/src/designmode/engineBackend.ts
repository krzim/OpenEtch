// The design build talking to a SIMULATOR engine that serves it (`vite build --mode design-engine`, mode.ts
// DESIGN_BACKEND "engine"): for the device harness only, never published. Document routes go to the engine as they
// are, so the design UI can be walked with real documents, previews and exports; the machine routes get the design
// backend's answers (designmode/routes.ts) and the device list the design build's built-in devices, so the screens
// are the ones OpenEtch Design shows. A machine route the UI still calls is logged (`console.warn`): the gating
// missed it. Events come from the engine's /api/events WebSocket.

import type { DesignBackend, TransportInit } from "../api/transport.ts";
import { designDeviceList, DEFAULT_DESIGN_DEVICE } from "./designDevices.ts";
import { jsonResponse, machineStub } from "./routes.ts";

export function engineBackend(): DesignBackend {
  const listeners = new Set<(event: unknown) => void>();
  let active = DEFAULT_DESIGN_DEVICE;
  const emit = (event: unknown) => {
    for (const fn of [...listeners]) fn(event);
  };
  let ws: WebSocket | null = null;
  const connect = () => {
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    ws = new WebSocket(`${proto}//${window.location.host}/api/events`);
    ws.onmessage = (m) => emit(String(m.data));
    ws.onclose = () => window.setTimeout(connect, 1000);
  };
  connect();
  return {
    async request(method: string, path: string, init: TransportInit): Promise<Response> {
      const bare = path.split("?")[0];
      const stub = machineStub(method, bare);
      if (stub) {
        if (stub.status === 404) console.warn(`[design] machine route called: ${method} ${bare}`);
        return jsonResponse(stub.status, stub.body);
      }
      if (method === "GET" && bare === "/api/devices") return jsonResponse(200, designDeviceList(active));
      const act = /^\/api\/devices\/([^/]+)\/activate$/.exec(bare);
      if (method === "POST" && act) {
        active = decodeURIComponent(act[1]);
        window.setTimeout(() => emit({ type: "device_changed", id: active }), 0);
        return jsonResponse(200, { ok: true, active });
      }
      return fetch(path, { ...init, method });
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => void listeners.delete(listener);
    },
  };
}
