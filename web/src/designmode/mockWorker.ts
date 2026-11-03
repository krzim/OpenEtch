// A stand-in for OpenEtch Design's backend worker (docs/design-only/plan.md 1.2) until the real one lands: the shell
// track's src/standalone/worker.ts, which hosts crates/webcore's wasm. It speaks the same protocol (api/transport.ts
// WorkerRequest / WorkerReply / WorkerEvent) and holds one empty document in memory: enough for the design build to
// start, show its canvas and its device picker, and for the UI's transport and FakeSocket to be exercised end to end.
// Edits answer 501. Replace the `new Worker(...)` in designmode/backend.ts with the real worker and delete this file.

import type { WorkerReply, WorkerRequest } from "../api/transport.ts";
import { designDeviceList, DEFAULT_DESIGN_DEVICE } from "./designDevices.ts";
import { machineStub } from "./routes.ts";

const scope = self as unknown as { postMessage(msg: unknown, transfer: Transferable[]): void; onmessage: ((ev: MessageEvent) => void) | null };

const EPOCH = Math.floor(Math.random() * 2 ** 48).toString(16);
let active = DEFAULT_DESIGN_DEVICE;

function document() {
  const list = designDeviceList(active);
  const dev = list.devices.find((d) => d.id === active);
  return {
    revision: 0,
    name: "Untitled",
    field_mm: dev?.workspace_mm[0] ?? 153.75,
    stock: null,
    layers: [{ id: "layer-1", name: "Layer 1", color: "#1f77b4", visible: true, output: true, settings: { mode: "line", power_pct: 20, speed_mm_s: 1000, frequency_khz: 30, pulse_width_ns: null, passes: 1 }, sub_layers: [] }],
    objects: [],
    groups: [],
    next_id: 2,
  };
}

const HISTORY = { marker: 0, marker_moved_by: "edit", undone_steps: 0, steps: [] };
const PROJECT = { dirty: false, display_name: null, name: null, revision: 0, saved_revision: null };
/** The commands the UI checks for at startup (api/commands.ts CMD), so the stand-in raises no "missing" chip. */
const COMMANDS = ["stock.define", "shape.rect", "shape.ellipse", "art.import_svg", "art.place", "art.array", "object.delete", "object.transform", "object.set_layer", "object.rename", "layer.add", "layer.update", "doc.new", "doc.import_lbrn2", "doc.export_lbrn2"].map((id) => ({
  id,
  title: id,
  description: "",
  risk: "edit",
  input_schema: { type: "object" },
}));

function route(method: string, path: string): { status: number; body: unknown } {
  const stub = machineStub(method, path);
  if (stub) return stub;
  if (method === "GET") {
    switch (path) {
      case "/api/commands":
        return { status: 200, body: COMMANDS };
      case "/api/document":
        return { status: 200, body: document() };
      case "/api/document/sync":
        return { status: 200, body: { epoch: EPOCH, document_id: "default", revision: 0, document: document(), history: HISTORY, project: PROJECT } };
      case "/api/history":
        return { status: 200, body: HISTORY };
      case "/api/project":
        return { status: 200, body: PROJECT };
      case "/api/projects":
        return { status: 200, body: [] };
      case "/api/documents":
        return { status: 200, body: { active: "default", documents: [{ id: "default", active: true, dirty: false, name: "Untitled", document_name: "Untitled", display_name: null, project: null, revision: 0, layers: 1, objects: 0 }] } };
      case "/api/devices":
        return { status: 200, body: designDeviceList(active) };
    }
  }
  const act = /^\/api\/devices\/([^/]+)\/activate$/.exec(path);
  if (method === "POST" && act) {
    const id = decodeURIComponent(act[1]);
    if (!designDeviceList(active).devices.some((d) => d.id === id)) return { status: 404, body: { error: `no device ${id}` } };
    active = id;
    scope.postMessage({ kind: "event", data: { type: "device_changed", id } }, []);
    return { status: 200, body: { ok: true, active } };
  }
  return { status: method === "GET" ? 404 : 501, body: { error: "not available in this preview of OpenEtch Design yet", code: "design_only" } };
}

scope.onmessage = (ev: MessageEvent) => {
  const req = ev.data as WorkerRequest;
  if (!req || typeof req.id !== "number") return;
  const { status, body } = route(req.method, req.path);
  const bytes = new TextEncoder().encode(JSON.stringify(body));
  const reply: WorkerReply = { id: req.id, status, headers: [["content-type", "application/json"]], body: bytes };
  scope.postMessage(reply, [bytes.buffer]);
};
