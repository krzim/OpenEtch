// HTTP calls of the Library window, layer and optimization defaults, preferences, the job log, diagnostics, the
// update check, Find My Laser / Add Galvo, the camera, Print and Cut and the run options it adds (batch and the
// external axis table). Routes: crates/engine/src/api_libs.rs (module docs) and ARCHITECTURE.md "Libraries, defaults,
// logs, diagnostics, camera". Same auth and error handling as api/client.ts (bearer token, ApiError with the engine's
// `error` text, `code` and HTTP status). Changes the engine allows only from a person (preferences, the device wizard,
// camera calibration, approvals) carry `origin: "ui"`. Nothing here fires the laser: a run request is a pending run.

import { ApiError, authToken, UI_ORIGIN } from "../api/client.ts";
import { contentDispositionName } from "../api/project.ts";

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

/** A file from the engine (with its Content-Disposition name), fetched with the Authorization header. */
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

const enc = encodeURIComponent;
const lib = (name: string) => `/api/library/${enc(name)}`;
const entry = (name: string, id: string) => `${lib(name)}/entries/${enc(id)}`;

export interface EntryMetaBody {
  material: string;
  /** null: No Thickness (then `title` names the folder). */
  thickness_mm: number | null;
  title: string;
  description: string;
}

export const libsApi = {
  // ---- material library (LightBurn's Library window) ----
  library: () => call("GET", "/api/library"),
  libraryTree: (name: string) => call("GET", lib(name)),
  libraryNew: (name: string) => call("POST", "/api/library/new", { name }),
  libraryLoad: (name: string, file: Blob) => call("POST", `/api/library/load?name=${enc(name)}`, undefined, { body: file, type: "application/xml" }),
  librarySelect: (name: string) => call("POST", "/api/library/select", { name }),
  libraryUnload: (name: string, discard: boolean) => call("POST", "/api/library/unload", discard ? { name, discard } : { name }),
  librarySave: (name: string, as?: string) => call("POST", "/api/library/save", as ? { name, as } : { name }),
  libraryRename: (name: string, newName: string) => call("POST", "/api/library/rename", { name, new_name: newName }),
  libraryMerge: (name: string, from: string) => call("POST", "/api/library/merge", { name, from }),
  libraryDownload: (name: string) => download(`${lib(name)}/download`),
  entryCreate: (name: string, layer: string, meta: EntryMetaBody) =>
    call("POST", `${lib(name)}/entries`, { layer, material: meta.material, thickness_mm: meta.thickness_mm, title: meta.title, description: meta.description }),
  entryUpdateFromLayer: (name: string, id: string, layer: string) => call("PUT", entry(name, id), { from_layer: layer }),
  entryEditCut: (name: string, id: string, settings: Record<string, unknown>) => call("PUT", entry(name, id), { settings }),
  entryEditMeta: (name: string, id: string, meta: EntryMetaBody) => call("PUT", entry(name, id), { meta }),
  entryDuplicate: (name: string, id: string) => call("POST", `${entry(name, id)}/duplicate`, {}),
  entryDelete: (name: string, id: string) => call("DELETE", entry(name, id)),
  /** Assign (copy) or Link the preset to a layer: one `layer.apply_preset` undo step. */
  entryApply: (name: string, id: string, layer: string, link: boolean) => call("POST", `${entry(name, id)}/apply`, { layer, link, origin: UI_ORIGIN }),

  // ---- preferences and defaults ----
  prefs: () => call("GET", "/api/prefs"),
  /** UI only: Require Framing, Play Sound, Job Checklist, Load defaults on New, debug log, update check. */
  prefsEdit: (patch: Record<string, unknown>) => call("PUT", "/api/prefs", { ...patch, origin: UI_ORIGIN }),
  layerMakeDefault: (layer: string, all: boolean) => call("POST", `/api/layers/${enc(layer)}/default`, all ? { all } : {}),
  layerReset: (layer: string, builtin: boolean) => call("POST", `/api/layers/${enc(layer)}/reset`, builtin ? { builtin, origin: UI_ORIGIN } : { origin: UI_ORIGIN }),
  optimizationMakeDefault: () => call("POST", "/api/optimization/default", {}),
  optimizationForgetDefault: () => call("DELETE", "/api/optimization/default"),
  optimizationReset: (builtin: boolean) => call("POST", "/api/optimization/reset", builtin ? { builtin, origin: UI_ORIGIN } : { origin: UI_ORIGIN }),

  // ---- job log, diagnostics, updates ----
  jobLog: (limit = 200) => call("GET", `/api/job-log?limit=${Math.max(1, Math.round(limit))}`),
  jobLogCsv: () => download("/api/job-log?format=csv"),
  bugReport: () => download("/api/diagnostics/report"),
  logs: () => call("GET", "/api/diagnostics/logs"),
  updates: () => call("GET", "/api/updates"),
  updatesCheck: () => call("POST", "/api/updates/check", {}),

  // ---- Find My Laser / Add Galvo ----
  scan: () => call("GET", "/api/devices/scan"),
  wizard: (body: { name?: string; markcfg_base64: string; cor_name?: string; cor_base64?: string; activate?: boolean }) =>
    call("POST", "/api/devices/wizard", { ...body, origin: UI_ORIGIN }),

  // ---- camera ----
  camera: () => call("GET", "/api/camera"),
  cameraFrame: (file: Blob) => call("POST", "/api/camera/frame", undefined, { body: file, type: file.type || "application/octet-stream" }),
  cameraCapture: (index: number) => call("POST", "/api/camera/capture", { index }),
  cameraFramePng: () => download("/api/camera/frame.png"),
  cameraOverlayPng: (pxPerMm: number) => download(`/api/camera/overlay.png?px_per_mm=${pxPerMm}`),
  cameraDetect: (cols: number, rows: number) => call("POST", "/api/camera/detect", { cols, rows }),
  cameraLens: (width: number, height: number, views: { cols: number; rows: number; corners: [number, number][] }[]) =>
    call("POST", "/api/camera/lens", { width, height, views, origin: UI_ORIGIN }),
  cameraAlign: (image: [number, number][], field: [number, number][]) => call("POST", "/api/camera/align", { image, field, origin: UI_ORIGIN }),
  cameraForget: () => call("DELETE", `/api/camera/calibration?origin=${UI_ORIGIN}`),

  // ---- machine bits used by the wizards (light only) ----
  pointer: (x: number, y: number) => call("POST", "/api/machine/pointer", { x, y }),
  frameStop: () => call("POST", "/api/machine/frame/stop", {}),
  activeDevice: async (): Promise<{ id: string; profile: Record<string, unknown> | null } | null> => {
    const list = (await call("GET", "/api/devices")) as { active?: unknown };
    if (typeof list?.active !== "string") return null;
    const d = (await call("GET", `/api/devices/${enc(list.active)}`)) as { profile?: unknown };
    const profile = d?.profile && typeof d.profile === "object" ? { ...(d.profile as Record<string, unknown>) } : null;
    // The device summary leaves the extension axis out; the active device's controller info has it.
    if (profile && !("axis" in profile)) {
      const info = (await call("GET", "/api/machine/info").catch(() => null)) as { axis?: unknown } | null;
      if (info && "axis" in info) profile.axis = info.axis;
    }
    return { id: list.active, profile };
  },
  deviceEdit: (id: string, profile: Record<string, unknown>) => call("PUT", `/api/devices/${enc(id)}`, { profile, origin: UI_ORIGIN }),
  projects: () => call("GET", "/api/projects"),
  /** A pending run (nothing fires until it is approved with the PIN). */
  requestRun: (body: Record<string, unknown>) => call("POST", "/api/runs", body),
  /** Approve with the operator PIN and the ticked Job Checklist (UI only; the PIN goes only into this body). */
  approve: (id: string, pin: string, checklist: boolean[] | null) =>
    call("POST", `/api/runs/${enc(id)}/approve`, checklist ? { pin, checklist } : { pin }),
};

export type LibsApi = typeof libsApi;
