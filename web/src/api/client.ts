// HTTP client for the engine API. Every request carries the bearer token in the Authorization header (the engine
// accepts `?token=` only on the /api/events WebSocket). The UI never edits the document locally: edits go through
// `execute`/`batch`, and callers refetch the document afterwards.

import type {
  CommandSpec,
  ExecuteResponse,
  GDocument,
  HistoryEntry,
  MachineStatus,
  Preview,
} from "./types.ts";
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

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  const token = authToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  if (body !== undefined) headers["Content-Type"] = "application/json";
  let res: Response;
  try {
    res = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  const text = await res.text();
  let data: unknown = undefined;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = text;
    }
  }
  if (!res.ok) {
    const obj = data && typeof data === "object" ? (data as { error?: unknown; code?: unknown }) : null;
    const msg = obj && "error" in obj ? String(obj.error) : text || res.statusText;
    const code = obj && typeof obj.code === "string" ? obj.code : undefined;
    throw new ApiError(`${method} ${path}: ${res.status} ${msg}`, res.status, { code, detail: msg });
  }
  return data as T;
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
  history: () => request<HistoryEntry[]>("GET", "/api/history"),
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
};

/** WebSocket URL for /api/events, carrying the token as `?token=` (browsers cannot set WS headers). */
export function eventsUrl(): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  const url = new URL(`${proto}//${window.location.host}/api/events`);
  const token = authToken();
  if (token) url.searchParams.set("token", token);
  return url.toString();
}
