// HTTP calls for job control, framing, calibration and device settings (ARCHITECTURE.md, "Job control, calibration
// and device settings"). Same auth and error handling as client.ts (bearer token, ApiError with the engine's `error`
// text, `code` and HTTP status); kept apart so the machine side of the UI has its own module. `requestRun` only creates
// a pending run, which still needs approval in the Runs card; `start` is the no-PIN engine's one-click Start (LightBurn's
// Start), which the engine accepts only from this page in a browser.

import { ApiError, authToken, UI_ORIGIN } from "./client.ts";
import { contentDispositionName } from "./project.ts";

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
    res = await fetch(path, {
      method,
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
    res = await fetch(path, { headers: headers("*/*") });
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

export const jobApi = {
  // ---- job control ----
  job: () => call("GET", "/api/job"),
  /** Stop At: approvals are refused once the job counter reaches it; null clears it. */
  setStopAt: (stopAt: number | null) => call("PUT", "/api/job/settings", { stop_at: stopAt }),
  resetCount: () => call("POST", "/api/job/count/reset", {}),
  endAfterCurrent: () => call("POST", "/api/job/end_after_current", {}),
  /** Never gated: it only holds motion. */
  pause: () => call("POST", "/api/machine/pause", {}),
  /** UI only. */
  resume: () => call("POST", "/api/machine/resume", { origin: UI_ORIGIN }),
  /** Creates a pending run with these options (`api/job.ts` runRequestBody); nothing fires until it is approved. */
  requestRun: (body: Record<string, unknown>) => call("POST", "/api/runs", body),
  /** One-click Start (engine `--approval click` only): plans the document with these options and burns it at once. */
  start: (body: Record<string, unknown>) => call("POST", "/api/job/start", { origin: UI_ORIGIN, ...body }),

  // ---- framing and pointer (light only) ----
  frame: (body: Record<string, unknown>) => call("POST", "/api/machine/frame", body),
  frameStop: () => call("POST", "/api/machine/frame/stop", {}),
  /** Hold the red dot at a field point (red-dot jog). */
  pointer: (x: number, y: number) => call("POST", "/api/machine/pointer", { x, y }),
  /** Move the extension axis (Z lift or rotary); UI only. */
  axis: (p: { z_mm: number } | { steps: number }) => call("POST", "/api/machine/axis", { ...p, origin: UI_ORIGIN }),
  machineInfo: () => call("GET", "/api/machine/info"),

  // ---- preview ----
  previewPng: (width: number, jumps: boolean) => download(`/api/preview.png?width=${Math.round(width)}&jumps=${jumps}`),

  // ---- lens calibration ----
  calibrationPattern: (size_mm: number, layer?: string | null) => call("POST", "/api/calibration/pattern", layer ? { size_mm, layer } : { size_mm }),
  calibrationSolve: (size_mm: number, measurements: Record<string, number>, apply: boolean) =>
    call("POST", "/api/calibration/solve", apply ? { size_mm, measurements, apply, origin: UI_ORIGIN } : { size_mm, measurements }),

  // ---- devices ----
  deviceEdit: (id: string, body: { profile?: unknown; display_name?: string }) => call("PUT", `/api/devices/${enc(id)}`, { ...body, origin: UI_ORIGIN }),
  deviceDuplicate: (id: string) => call("POST", `/api/devices/${enc(id)}/duplicate`, { origin: UI_ORIGIN }),
  deviceLbdev: (id: string) => download(`/api/devices/${enc(id)}/lbdev`),
  /** The `.lbdev` JSON text, for reading the profile's LightBurn-keyed values. */
  deviceLbdevText: async (id: string): Promise<string> => (await download(`/api/devices/${enc(id)}/lbdev`)).blob.text(),
  corUpload: (id: string, file: Blob, name: string) =>
    call("PUT", `/api/devices/${enc(id)}/cor?name=${enc(name)}&origin=${UI_ORIGIN}`, undefined, { body: file, type: "application/octet-stream" }),
  corClear: (id: string) => call("DELETE", `/api/devices/${enc(id)}/cor?origin=${UI_ORIGIN}`),
  importMarkcfg: (file: Blob, name: string) =>
    call("POST", `/api/devices/import/markcfg?name=${enc(name)}&origin=${UI_ORIGIN}`, undefined, { body: file, type: "application/octet-stream" }),
};
