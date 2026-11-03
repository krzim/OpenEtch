// HTTP calls for job control, framing, calibration and device settings (ARCHITECTURE.md, "Job control, calibration
// and device settings"). Same auth and error handling as client.ts (bearer token, ApiError with the engine's `error`
// text, `code` and HTTP status); kept apart so the machine side of the UI has its own module. `requestRun` only creates
// a pending run, which still needs approval in the Laser window (Waiting for approval); `start` is the no-PIN engine's one-click Start (LightBurn's
// Start), which the engine accepts only from this page in a browser.

import { ApiError, authToken, UI_ORIGIN } from "./client.ts";
import { contentDispositionName } from "./project.ts";
import { noMachine, transport } from "./transport.ts";
import { DESIGN_ONLY } from "../mode.ts";

function headers(accept: string, contentType?: string): Record<string, string> {
  const h: Record<string, string> = { Accept: accept };
  if (contentType) h["Content-Type"] = contentType;
  const token = authToken();
  if (token) h.Authorization = `Bearer ${token}`;
  return h;
}

function parse(text: string): unknown {
  if (!text) return undefined;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

async function call<T = unknown>(method: string, path: string, body?: unknown, raw?: { body: BodyInit; type: string }): Promise<T> {
  let res: Response;
  try {
    res = await transport(method, path, {
      headers: headers("application/json", raw ? raw.type : body === undefined ? undefined : "application/json"),
      body: raw ? raw.body : body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  const text = await res.text();
  const data = parse(text);
  const obj = data && typeof data === "object" ? (data as { ok?: unknown; error?: unknown; code?: unknown }) : null;
  if (!res.ok || obj?.ok === false) {
    const msg = obj && "error" in obj ? String(obj.error) : text || res.statusText;
    const code = obj && typeof obj.code === "string" ? obj.code : undefined;
    throw new ApiError(`${method} ${path}: ${res.status} ${msg}`, res.status, { code, detail: msg });
  }
  return data as T;
}

/** A file from the engine, with its Content-Disposition name. */
async function download(path: string): Promise<{ blob: Blob; filename: string | null }> {
  let res: Response;
  try {
    res = await transport("GET", path, { headers: headers("*/*") });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  if (!res.ok) {
    const text = await res.text();
    const data = parse(text);
    const msg = data && typeof data === "object" && "error" in data ? String((data as { error: unknown }).error) : text || res.statusText;
    throw new ApiError(`GET ${path}: ${res.status} ${msg}`, res.status, { detail: msg });
  }
  return { blob: await res.blob(), filename: contentDispositionName(res.headers.get("Content-Disposition")) };
}

/** Save a blob through a temporary link. */
export function saveBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

const enc = encodeURIComponent;

/**
 * POST /api/machine/console: the board's answer `{line, lines, ok, error}` also when the board refused the line
 * (`ok: false`, HTTP 200); an engine refusal (a line that could fire: 422, MCP headers: 403, busy: 409) throws.
 */
async function consoleLine(line: string): Promise<unknown> {
  let res: Response;
  try {
    res = await transport("POST", "/api/machine/console", {
      headers: headers("application/json", "application/json"),
      body: JSON.stringify({ line, origin: UI_ORIGIN }),
    });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  const text = await res.text();
  const data = parse(text);
  if (!res.ok) {
    const obj = data && typeof data === "object" ? (data as { error?: unknown; code?: unknown }) : null;
    const msg = obj && "error" in obj ? String(obj.error) : text || res.statusText;
    throw new ApiError(`POST /api/machine/console: ${res.status} ${msg}`, res.status, { detail: msg, code: typeof obj?.code === "string" ? obj.code : undefined });
  }
  return data;
}

export const jobApi = {
  // ---- job control ----
  job: () => call("GET", "/api/job"),
  /** Stop At: approvals are refused once the job counter reaches it; null clears it. */
  setStopAt: DESIGN_ONLY ? noMachine : (stopAt: number | null) => call("PUT", "/api/job/settings", { stop_at: stopAt }),
  resetCount: DESIGN_ONLY ? noMachine : () => call("POST", "/api/job/count/reset", {}),
  endAfterCurrent: DESIGN_ONLY ? noMachine : () => call("POST", "/api/job/end_after_current", {}),
  /** Never gated: it only holds motion. */
  pause: DESIGN_ONLY ? noMachine : () => call("POST", "/api/machine/pause", {}),
  /** UI only. */
  resume: DESIGN_ONLY ? noMachine : () => call("POST", "/api/machine/resume", { origin: UI_ORIGIN }),
  /** Creates a pending run with these options (`api/job.ts` runRequestBody); nothing fires until it is approved. */
  requestRun: DESIGN_ONLY ? noMachine : (body: Record<string, unknown>) => call("POST", "/api/runs", body),
  /** One-click Start (engine `--approval click` only): plans the document with these options and burns it at once. */
  start: DESIGN_ONLY ? noMachine : (body: Record<string, unknown>) => call("POST", "/api/job/start", { origin: UI_ORIGIN, ...body }),

  // ---- framing and pointer (light only) ----
  frame: DESIGN_ONLY ? noMachine : (body: Record<string, unknown>) => call("POST", "/api/machine/frame", body),
  frameStop: DESIGN_ONLY ? noMachine : () => call("POST", "/api/machine/frame/stop", {}),
  /** Hold the red dot at a field point (red-dot jog). */
  pointer: DESIGN_ONLY ? noMachine : (x: number, y: number) => call("POST", "/api/machine/pointer", { x, y }),
  /** Move the extension axis (Z lift or rotary); UI only. */
  axis: DESIGN_ONLY ? noMachine : (p: { z_mm: number } | { steps: number }) => call("POST", "/api/machine/axis", { ...p, origin: UI_ORIGIN }),
  machineInfo: DESIGN_ONLY ? noMachine : () => call("GET", "/api/machine/info"),

  // ---- a GRBL gantry: Move and Console windows, Save G-code (none of these can fire the laser) ----
  /** GET /api/machine/grbl; `refresh` asks an idle board for a status report first (its position). */
  grblStatus: DESIGN_ONLY ? noMachine : (refresh: boolean) => call("GET", `/api/machine/grbl${refresh ? "?refresh=true" : ""}`),
  /** A jog by `{dx, dy}` (canvas directions) or to `{x, y}` (canvas frame), mm, at `feed` mm/min, laser off. UI only. */
  jog: DESIGN_ONLY ? noMachine : (body: { dx: number; dy: number } | { x: number; y: number }, feed?: number) =>
    call("POST", "/api/machine/jog", { ...body, ...(feed ? { feed } : {}), origin: UI_ORIGIN }),
  /** 0x85: stops a jog; never gated. */
  jogCancel: DESIGN_ONLY ? noMachine : () => call("POST", "/api/machine/jog/cancel", {}),
  home: DESIGN_ONLY ? noMachine : () => call("POST", "/api/machine/home", { origin: UI_ORIGIN }),
  unlock: DESIGN_ONLY ? noMachine : () => call("POST", "/api/machine/unlock", { origin: UI_ORIGIN }),
  console: DESIGN_ONLY ? noMachine : consoleLine,
  /** GET /api/export/gcode: the job as G-code for the active GRBL gantry (sends nothing to the machine). */
  exportGcode: () => download("/api/export/gcode"),

  // ---- a Ruida gantry: File List window, Send, Save RD file ----
  /** GET /api/machine/files: the files stored on the controller (reads only). */
  ruidaFiles: DESIGN_ONLY ? noMachine : () => call("GET", "/api/machine/files"),
  /** Send: the job (or the objects `ids`) stored on the controller as `name`; never started. UI only. */
  ruidaSend: DESIGN_ONLY ? noMachine : (name: string, ids?: readonly string[]) => call("POST", "/api/machine/files/send", { name, ...(ids ? { ids } : {}), origin: UI_ORIGIN }),
  /** A pending run of stored file `index` (named `name`): it starts only when approved in the Laser window. UI only. */
  ruidaRun: DESIGN_ONLY ? noMachine : (index: number, name: string) => call("POST", `/api/machine/files/${index}/run`, { name, origin: UI_ORIGIN }),
  /** Delete stored file `index` if it is still named `name`. UI only. */
  ruidaDelete: DESIGN_ONLY ? noMachine : (index: number, name: string) => call("POST", `/api/machine/files/${index}/delete`, { name, origin: UI_ORIGIN }),
  /** GET /api/export/rd: the job as an RDWorks .rd file for the active Ruida gantry (sends nothing to the machine). */
  exportRd: DESIGN_ONLY ? noMachine : () => download("/api/export/rd"),

  // ---- preview ----
  previewPng: (width: number, jumps: boolean) => download(`/api/preview.png?width=${Math.round(width)}&jumps=${jumps}`),

  // ---- lens calibration ----
  calibrationPattern: DESIGN_ONLY ? noMachine : (size_mm: number, layer?: string | null) => call("POST", "/api/calibration/pattern", layer ? { size_mm, layer } : { size_mm }),
  calibrationSolve: DESIGN_ONLY ? noMachine : (size_mm: number, measurements: Record<string, number>, apply: boolean) =>
    call("POST", "/api/calibration/solve", apply ? { size_mm, measurements, apply, origin: UI_ORIGIN } : { size_mm, measurements }),

  // ---- devices ----
  deviceEdit: (id: string, body: { profile?: unknown; display_name?: string }) => call("PUT", `/api/devices/${enc(id)}`, { ...body, origin: UI_ORIGIN }),
  deviceDuplicate: (id: string) => call("POST", `/api/devices/${enc(id)}/duplicate`, { origin: UI_ORIGIN }),
  deviceLbdev: (id: string) => download(`/api/devices/${enc(id)}/lbdev`),
  /** The `.lbdev` JSON text, for reading the profile's LightBurn-keyed values. */
  deviceLbdevText: async (id: string): Promise<string> => (await download(`/api/devices/${enc(id)}/lbdev`)).blob.text(),
  corUpload: DESIGN_ONLY ? noMachine : (id: string, file: Blob, name: string) =>
    call("PUT", `/api/devices/${enc(id)}/cor?name=${enc(name)}&origin=${UI_ORIGIN}`, undefined, { body: file, type: "application/octet-stream" }),
  corClear: DESIGN_ONLY ? noMachine : (id: string) => call("DELETE", `/api/devices/${enc(id)}/cor?origin=${UI_ORIGIN}`),
  /** POST /api/devices/grbl: a GRBL gantry from a profile (UI only). */
  addGrblDevice: DESIGN_ONLY ? noMachine : (body: { display_name: string; profile: Record<string, unknown> }) =>
    call("POST", "/api/devices/grbl", { ...body, origin: UI_ORIGIN }),
  /** POST /api/devices/ruida: a Ruida gantry from a profile (UI only). */
  addRuidaDevice: DESIGN_ONLY ? noMachine : (body: { display_name: string; profile: Record<string, unknown> }) =>
    call("POST", "/api/devices/ruida", { ...body, origin: UI_ORIGIN }),
  importMarkcfg: DESIGN_ONLY ? noMachine : (file: Blob, name: string) =>
    call("POST", `/api/devices/import/markcfg?name=${enc(name)}&origin=${UI_ORIGIN}`, undefined, { body: file, type: "application/octet-stream" }),
};
