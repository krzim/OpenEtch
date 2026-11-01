// HTTP client for the engine API. Every request carries the bearer token in the Authorization header (the engine
// accepts `?token=` only on the /api/events WebSocket). The UI never edits the document locally: edits go through
// `execute`/`batch`, and callers refetch the document afterwards.

import type {
  CommandSpec,
  ExecuteResponse,
  GDocument,
  MachineStatus,
  Preview,
  ProjectInfo,
} from "./types.ts";
import { contentDispositionName, type FileFormat, type ImportMode, type UploadFormat } from "./project.ts";
import { okError } from "./runs.ts";

const TOKEN_KEY = "openetch.token";

/**
 * Read `?token=` from the page URL, persist it, and strip it from the address bar so it is not shared by accident.
 * Returns the stored token (which may come from an earlier visit), or null.
 */
export function initAuthToken(): string | null {
  try {
    const url = new URL(window.location.href);
    const fromUrl = url.searchParams.get("token");
    if (fromUrl) {
      localStorage.setItem(TOKEN_KEY, fromUrl);
      url.searchParams.delete("token");
      window.history.replaceState(null, "", url.toString());
    }
  } catch {
    // Storage may be unavailable (private mode); fall through to whatever is stored.
  }
  return authToken();
}

/** The stored bearer token, or null. */
export function authToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

/** An HTTP or engine-level failure, with a message suitable for a toast. */
export class ApiError extends Error {
  /** Machine-readable `code` from the error body (e.g. "pin_invalid"), when the engine sent one. */
  readonly code?: string;
  /** The engine's own `error` text, without the method/path/status prefix. */
  readonly detail?: string;
  /** HTTP status, when the failure was an HTTP error. */
  readonly status?: number;
  // No parameter properties: Node's type stripping (npm test) cannot run them.
  constructor(message: string, status?: number, extra?: { code?: string; detail?: string }) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = extra?.code;
    this.detail = extra?.detail;
  }
}

function authHeaders(accept: string): Record<string, string> {
  const headers: Record<string, string> = { Accept: accept };
  const token = authToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

async function send(method: string, path: string, init: { headers: Record<string, string>; body?: BodyInit }): Promise<Response> {
  try {
    return await fetch(path, { method, ...init });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
}

/** Turn a non-2xx answer into an ApiError carrying the engine's `error` text and `code`. */
function httpError(method: string, path: string, res: Response, text: string): ApiError {
  const data = parseBody(text);
  const obj = data && typeof data === "object" ? (data as { error?: unknown; code?: unknown }) : null;
  const msg = obj && "error" in obj ? String(obj.error) : text || res.statusText;
  const code = obj && typeof obj.code === "string" ? obj.code : undefined;
  return new ApiError(`${method} ${path}: ${res.status} ${msg}`, res.status, { code, detail: msg });
}

function parseBody(text: string): unknown {
  if (!text) return undefined;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers = authHeaders("application/json");
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const res = await send(method, path, { headers, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await res.text();
  if (!res.ok) throw httpError(method, path, res, text);
  return parseBody(text) as T;
}

/** The origin every UI edit is tagged with (ARCHITECTURE.md: `"origin": "ui"`). */
export const UI_ORIGIN = "ui";

/** One command invocation for `/api/execute` or `/api/batch`. */
export interface Invocation {
  id: string;
  params: Record<string, unknown>;
}

/** Run a document command. Throws ApiError when the engine answers `ok: false`. */
export async function execute(inv: Invocation): Promise<ExecuteResponse & { ok: true }> {
  const res = await request<ExecuteResponse>("POST", "/api/execute", { ...inv, origin: UI_ORIGIN });
  if (!res || res.ok !== true) throw new ApiError(res && "error" in res ? res.error : "command failed");
  return res;
}

/** Run several commands as one undo step (all-or-nothing). */
export async function batch(commands: Invocation[]): Promise<unknown> {
  const res = await request<{ ok?: boolean; error?: string }>("POST", "/api/batch", { commands, origin: UI_ORIGIN });
  if (res && res.ok === false) throw new ApiError(res.error ?? "batch failed");
  return res;
}

/** POST that throws ApiError on a 200 `{ok:false, error}` answer (e.g. undo with nothing to undo). */
async function checked(path: string, body: unknown, what: string): Promise<unknown> {
  const res = await request<unknown>("POST", path, body);
  const err = okError(res, what);
  if (err !== null) throw new ApiError(err);
  return res;
}

export const api = {
  commands: () => request<CommandSpec[]>("GET", "/api/commands"),
  document: () => request<GDocument>("GET", "/api/document"),
  /** The timeline (`{marker, steps}`), or an older engine's log array; normalise with `normalizeHistory`. */
  history: () => request<unknown>("GET", "/api/history"),
  /** Roll the timeline marker to `marker` (the index of the last step to keep applied). */
  historyGoto: (marker: number) => checked("/api/history/goto", { marker }, "moving the timeline marker failed"),
  /** Remove one timeline step; the later steps are replayed. */
  historyDelete: (index: number) => checked("/api/history/delete", { index }, "deleting the step failed"),
  preview: () => request<Preview>("GET", "/api/preview"),
  machine: () => request<MachineStatus>("GET", "/api/machine"),
  /** POST /api/undo. Throws ApiError when there is nothing to undo (200 `{ok:false}`). */
  undo: () => checked("/api/undo", { origin: UI_ORIGIN }, "nothing to undo"),
  /** POST /api/redo. Throws ApiError when there is nothing to redo (200 `{ok:false}`). */
  redo: () => checked("/api/redo", { origin: UI_ORIGIN }, "nothing to redo"),
  /** Red-pointer framing of the current job bounds. Never fires the laser. */
  frame: () => checked("/api/machine/frame", {}, "framing failed"),
  /** Ends red-pointer framing only; pending runs are kept. */
  frameStop: () => checked("/api/machine/frame/stop", {}, "stopping the frame failed"),
  /** Emergency stop: abort whatever runs and cancel pending runs. Always allowed, never gated. */
  stop: () => request<unknown>("POST", "/api/machine/stop", {}),
  /** Creates a pending run; nothing fires until it is approved. */
  requestRun: () => request<unknown>("POST", "/api/runs", { origin: UI_ORIGIN }),
  /**
   * Approve a pending run with the operator PIN. The PIN goes only into this request body; it is never stored.
   * Errors are HTTP 403 with `code` "pin_not_set" | "pin_invalid" | "pin_required" | "pin_locked".
   */
  approveRun: (id: string, pin: string) => request<unknown>("POST", `/api/runs/${encodeURIComponent(id)}/approve`, { pin }),
  /** The current project name and whether the document changed since it was saved. */
  project: () => request<ProjectInfo>("GET", "/api/project"),
  /** Saved projects on the engine (`[{name, modified_unix_s, size}]`); normalize with `projectList`. */
  projects: () => request<unknown>("GET", "/api/projects"),
  /**
   * Save the document as `<projects-dir>/<name>.openetch`. Without `overwrite` the engine's default applies (replace);
   * with `overwrite: false` saving over another existing project is refused with a 409 ApiError (see `isSaveConflict`).
   */
  saveProject: (name: string, overwrite?: boolean) =>
    checked("/api/projects/save", overwrite === undefined ? { name } : { name, overwrite }, "saving the project failed"),
  /** Replace the document with a saved project (one undo step). */
  openProject: (name: string) => checked("/api/projects/open", { name }, "opening the project failed"),
  /**
   * Upload a file as the raw body: `openetch` runs `doc.load`, `lbrn2` runs `doc.import_lbrn2` in `mode`, laid out
   * for `fieldMm` (default the document's field size); `svg`, `dxf` and `image` import large artwork into the
   * document (always merged, so no mode is sent) on `art.layer` (default the engine's first layer) named `art.name`.
   * Resolves to the engine's answer; unwrap it with `commandResult`.
   */
  upload: async (
    file: Blob,
    format: UploadFormat,
    mode: ImportMode = "replace",
    fieldMm?: number,
    art: { layer?: string | null; name?: string | null } = {},
  ): Promise<unknown> => {
    // A bad field size is refused here, never silently dropped (the import would then use the document's size).
    if (format === "lbrn2" && fieldMm !== undefined && !(Number.isFinite(fieldMm) && fieldMm > 0)) {
      throw new ApiError(`field_mm must be a positive number (got ${fieldMm})`, 400, { detail: `field_mm must be a positive number (got ${fieldMm})` });
    }
    const project = format === "openetch" || format === "lbrn2";
    const field = format === "lbrn2" && fieldMm !== undefined ? `&field_mm=${fieldMm}` : "";
    // Layer only goes with artwork imports; the engine refuses options that do not fit the format. `name` goes with
    // every format: for a project it is the opened file's name, which the engine keeps as the document's display name
    // (download names after a replacing Open) and gives a merged LightBurn file's group.
    const nameParam = art.name ? `&name=${encodeURIComponent(art.name)}` : "";
    const extra = project ? nameParam : [art.layer ? `&layer=${encodeURIComponent(art.layer)}` : "", nameParam].join("");
    const path = `/api/document/upload?format=${format}${project ? `&mode=${mode}` : ""}${field}${extra}`;
    const headers = authHeaders("application/json");
    headers["Content-Type"] = "application/octet-stream";
    const res = await send("POST", path, { headers, body: file });
    const text = await res.text();
    if (!res.ok) throw httpError("POST", path, res, text);
    return parseBody(text);
  },
  /** LightBurn devices the engine knows (`{available, source, default_device, devices}`); normalize with `lightburnInfo`. */
  lightburn: () => request<unknown>("GET", "/api/lightburn"),
  /**
   * Give the engine LightBurn's device list, for engines on a computer without LightBurn. `body` is prefs.ini-shaped
   * JSON built by `devicePrefsBody` (the device list only, never the whole file). The engine stores only the parsed
   * devices.
   */
  lightburnUpload: async (body: string): Promise<unknown> => {
    const path = "/api/lightburn/prefs";
    const headers = authHeaders("application/json");
    headers["Content-Type"] = "application/json";
    const res = await send("POST", path, { headers, body });
    const text = await res.text();
    if (!res.ok) throw httpError("POST", path, res, text);
    const data = parseBody(text);
    const err = okError(data, "the engine refused LightBurn's settings");
    if (err !== null) throw new ApiError(err);
    return data;
  },
  /** Forget an uploaded LightBurn device list (the engine's own LightBurn settings, if any, apply again). */
  lightburnForget: () => request<unknown>("DELETE", "/api/lightburn/prefs"),
  /** Download the document as a file, with the name from Content-Disposition when the engine sends one. */
  download: async (format: FileFormat): Promise<{ blob: Blob; filename: string | null }> => {
    const path = `/api/document/download?format=${format}`;
    const res = await send("GET", path, { headers: authHeaders("*/*") });
    if (!res.ok) throw httpError("GET", path, res, await res.text());
    return { blob: await res.blob(), filename: contentDispositionName(res.headers.get("Content-Disposition")) };
  },
};

/** WebSocket URL for /api/events, carrying the token as `?token=` (browsers cannot set WS headers). */
export function eventsUrl(): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  const url = new URL(`${proto}//${window.location.host}/api/events`);
  const token = authToken();
  if (token) url.searchParams.set("token", token);
  return url.toString();
}
