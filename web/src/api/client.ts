// HTTP client for the engine API. Every request carries the bearer token in the Authorization header (the engine
// accepts `?token=` only on the /api/events WebSocket). The UI never edits the document locally: edits go through
// `execute`/`batch`, and callers refetch the document afterwards.
//
// Access token (docs/ux/ui-plan-2026-11.md 1.3): kept in localStorage ("Remember on this device"), or for this tab
// only. An answer of 401 sets `needsToken`, and the app shows the Connect card (app/ConnectScreen.tsx) instead of a
// broken UI; pasting the token there (`tryToken`) reconnects without a reload. The token never goes into a URL
// except the WebSocket's, which browsers cannot give headers. App windows that cannot see the tab's localStorage
// (Safari Add to Dock) are logged in by the engine's HttpOnly cookie, which `fetch` sends on its own.

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

/** localStorage key of the remembered access token. */
export const TOKEN_KEY = "openetch.token";

/** A token pasted with "Remember on this device" off: kept for this tab only (in memory). */
let tabToken: string | null = null;

/** The engine refused the token (401): the Connect card replaces the app until a token is accepted. */
let needsToken = false;
const tokenListeners = new Set<() => void>();

function setNeedsToken(v: boolean): void {
  if (needsToken === v) return;
  needsToken = v;
  for (const fn of [...tokenListeners]) fn();
}

/** Whether the engine refused the token (an answer of 401 since the last accepted token). */
export function needsTokenNow(): boolean {
  return needsToken;
}

/** Call `fn` whenever `needsTokenNow()` changes (for useSyncExternalStore); returns the unsubscribe function. */
export function subscribeNeedsToken(fn: () => void): () => void {
  tokenListeners.add(fn);
  return () => void tokenListeners.delete(fn);
}

/** The engine answered 401: show the Connect card. Any module that talks to /api may report it. */
export function markUnauthorized(): void {
  setNeedsToken(true);
}

/** A token was accepted: leave the Connect card. */
export function markAuthorized(): void {
  setNeedsToken(false);
}

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

/** The bearer token (this tab's, else the remembered one), or null. */
export function authToken(): string | null {
  if (tabToken) return tabToken;
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

/**
 * Use `token` from now on: remembered on this device (localStorage), or with `remember` false for this tab only (a
 * remembered one is then forgotten). An empty token forgets both.
 */
export function setToken(token: string, remember = true): void {
  const t = token.trim();
  tabToken = null;
  try {
    if (t && remember) localStorage.setItem(TOKEN_KEY, t);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    // Storage unavailable (private mode): keep it for this tab.
    if (t) tabToken = t;
  }
  if (t && !remember) tabToken = t;
}

/** Forget the token on this device and in this tab. */
export function clearToken(): void {
  setToken("");
}

/**
 * The token in what was pasted into the Connect card: the token itself, or a pairing URL (`…/?token=<t>`) or a line
 * of `openetch token` output that carries one. Null when there is nothing token-like.
 */
export function tokenFromInput(text: string): string | null {
  const s = text.trim();
  const m = /[?&]token=([A-Za-z0-9_-]+)/.exec(s);
  const t = m ? m[1] : s;
  return /^[A-Za-z0-9_-]{16,}$/.test(t) ? t : null;
}

/** What `tryToken` found. */
export type TokenCheck = { ok: true } | { ok: false; reason: "refused" | "unreachable"; detail?: string };

/**
 * Check a token against the engine (GET /api/machine): accepted, it stays in use and the Connect card goes away;
 * refused, it is forgotten again. `token` null retries with what is stored (and the engine's cookie).
 */
export async function tryToken(token: string | null, remember = true): Promise<TokenCheck> {
  if (token !== null) setToken(token, remember);
  try {
    await request<unknown>("GET", "/api/machine");
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      if (token !== null) clearToken();
      return { ok: false, reason: "refused", detail: e.detail };
    }
    return { ok: false, reason: "unreachable", detail: e instanceof Error ? e.message : String(e) };
  }
  markAuthorized();
  return { ok: true };
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

/** Turn a non-2xx answer into an ApiError carrying the engine's `error` text and `code`. A 401 also raises the
 * Connect card (`needsToken`). */
function httpError(method: string, path: string, res: Response, text: string): ApiError {
  if (res.status === 401) markUnauthorized();
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
  /**
   * Red-pointer framing. Never fires the laser. Default: the bounding box of the whole job; `ids` frames only those
   * objects, `mode: "outline"` traces their shapes instead of the box (engine `FrameRequest`).
   */
  frame: (req: { ids?: string[]; mode?: "bounds" | "outline" } = {}) => checked("/api/machine/frame", req, "framing failed"),
  /** Ends red-pointer framing only; pending runs are kept. */
  frameStop: () => checked("/api/machine/frame/stop", {}, "stopping the frame failed"),
  /** Emergency stop: abort whatever runs and cancel pending runs. Always allowed, never gated. */
  stop: () => request<unknown>("POST", "/api/machine/stop", {}),
  /** Creates a pending run; nothing fires until it is approved. */
  requestRun: () => request<unknown>("POST", "/api/runs", { origin: UI_ORIGIN }),
  /**
   * Approve a pending run with the operator PIN, or with `pin` null when the engine runs without a PIN (Start; the
   * browser adds the Sec-Fetch-Site and Origin headers the engine checks). The PIN goes only into this request body;
   * it is never stored. Errors are HTTP 403 with `code` "pin_not_set" | "pin_invalid" | "pin_required" |
   * "pin_locked" | "browser_required".
   */
  approveRun: (id: string, pin: string | null) =>
    request<unknown>("POST", `/api/runs/${encodeURIComponent(id)}/approve`, pin === null ? {} : { pin }),
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
  /** The device registry (`{active, devices}`); normalize with `deviceList`. */
  devices: () => request<unknown>("GET", "/api/devices"),
  /** One device with its settings and profile diffs; normalize with `deviceDetails`. */
  device: (id: string) => request<unknown>("GET", `/api/devices/${encodeURIComponent(id)}`),
  /**
   * Import devices. `body` is the prefs.ini/.lbdev JSON built by `deviceImportBody` (typed device fields only); without
   * a body the engine reads LightBurn's prefs.ini on its own computer (read-only). `add` is "all" or one device's name.
   * Resolves to `{added, updated, skipped}`; normalize with `importResult`.
   */
  deviceImport: async (body: string | null, add: string): Promise<unknown> => {
    const path = `/api/devices/import?${body === null ? "from=lightburn&" : ""}add=${encodeURIComponent(add)}`;
    const headers = authHeaders("application/json");
    if (body !== null) headers["Content-Type"] = "application/json";
    const res = await send("POST", path, { headers, body: body ?? undefined });
    const text = await res.text();
    if (!res.ok) throw httpError("POST", path, res, text);
    const data = parseBody(text);
    const err = okError(data, "the engine refused the devices");
    if (err !== null) throw new ApiError(err);
    return data;
  },
  /** Make a device active: the engine swaps the profile and re-initialises the laser; 409 while a run or frame is active. */
  deviceActivate: (id: string) => checked(`/api/devices/${encodeURIComponent(id)}/activate`, {}, "switching the device failed"),
  /** Remove a device (never the active or the built-in one). */
  deviceRemove: (id: string) => request<unknown>("DELETE", `/api/devices/${encodeURIComponent(id)}`),
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
