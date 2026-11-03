// The one way the UI reaches its backend (docs/design-only/plan.md "How the UI talks to it"). Every fetch wrapper
// (client.ts `send`, jobApi.ts, design/designApi.ts, files/filesApi.ts, libs/api.ts, workspace/wsApi.ts and the
// smaller ones) calls `transport(method, path, init)` instead of `fetch`, and useEngine.ts opens its event stream with
// `openEventStream`. In the normal build these are `fetch` and the engine's /api/events WebSocket, unchanged. In
// OpenEtch Design (mode.ts DESIGN_ONLY) they go to the installed `DesignBackend`: the in-browser worker that hosts
// crates/webcore (`workerBackend`), or another one set with `setDesignBackend` (designmode/backend.ts picks it).
// Requests made before a backend is installed wait for it. Paths, JSON shapes, ApiError handling, Content-Disposition
// downloads and the normalisers stay exactly as they are: a backend answers with real `Response`s.

import { DESIGN_ONLY } from "../mode.ts";

/** What a wrapper passes: fetch's init without `method` (that is the first argument). */
export type TransportInit = Omit<RequestInit, "method">;

/** A backend for the design build: answers /api requests and announces engine events. */
export interface DesignBackend {
  /** Answer one request, as the engine would (status, headers, body). Rejects only when the backend is gone. */
  request(method: string, path: string, init: TransportInit): Promise<Response>;
  /**
   * Call `listener` with every event the engine would send on /api/events (`{type: "document_changed", ...}`), as
   * the parsed object or its JSON text. Returns the unsubscribe function.
   */
  subscribe(listener: (event: unknown) => void): () => void;
  /**
   * Optional: the backend's own /api/events stream (a socket that closes when the backend restarts, so useEngine
   * reconnects and refetches everything). When given it is used instead of a FakeSocket over `subscribe`.
   */
  openEvents?(): SocketLike;
  /**
   * Optional: things the backend tells the person (the editor restarted, storage is memory only, ...). `key` is a
   * message in workspace/locales; the page shows them as toasts. Returns the unsubscribe function.
   */
  notices?(listener: (n: DesignNotice) => void): () => void;
}

/** A notice from the design backend (standalone/contract.ts `Notice`). */
export interface DesignNotice {
  key: string;
  vars: Record<string, string>;
  level: "info" | "warning" | "error";
}

let backend: DesignBackend | null = null;
let backendReady: Promise<DesignBackend> | null = null;
let resolveBackend: ((b: DesignBackend) => void) | null = null;

function waitForBackend(): Promise<DesignBackend> {
  if (backend) return Promise.resolve(backend);
  if (!backendReady) backendReady = new Promise((resolve) => (resolveBackend = resolve));
  return backendReady;
}

/** Install the design build's backend (once, at startup); requests already waiting go to it. */
export function setDesignBackend(b: DesignBackend): void {
  backend = b;
  resolveBackend?.(b);
  resolveBackend = null;
  backendReady = Promise.resolve(b);
}

/** The design backend's notices (none in the normal build). Returns the unsubscribe function. */
export function onDesignNotice(listener: (n: DesignNotice) => void): () => void {
  if (!DESIGN_ONLY) return () => {};
  let off: (() => void) | null = null;
  let gone = false;
  void waitForBackend().then((b) => {
    if (!gone && b.notices) off = b.notices(listener);
  });
  return () => {
    gone = true;
    off?.();
  };
}

/** Send one request: `fetch` in the normal build, the design backend in OpenEtch Design. */
export function transport(method: string, path: string, init: TransportInit = {}): Promise<Response> {
  if (!DESIGN_ONLY) return fetch(path, { ...init, method });
  return waitForBackend().then((b) => b.request(method, path, init));
}

/** The parts of a WebSocket useEngine.ts uses for /api/events. */
export interface SocketLike {
  onopen: ((ev: Event) => void) | null;
  onmessage: ((ev: MessageEvent) => void) | null;
  onclose: ((ev: CloseEvent) => void) | null;
  onerror: ((ev: Event) => void) | null;
  close(): void;
}

/**
 * The /api/events stream: a WebSocket to `url()` in the normal build; in OpenEtch Design a FakeSocket fed by the
 * backend's events (it opens as soon as a backend is installed and never drops, so useEngine's reconnect logic stays
 * idle). Each message's `data` is the event's JSON text, as the engine's socket sends it.
 */
export function openEventStream(url: () => string): SocketLike {
  if (!DESIGN_ONLY) return new WebSocket(url());
  return backend?.openEvents ? backend.openEvents() : new FakeSocket();
}

/** A WebSocket look-alike over `DesignBackend.subscribe` (or, made before the backend was installed, a stand-in for
 *  the backend's own `openEvents` socket). */
export class FakeSocket implements SocketLike {
  onopen: ((ev: Event) => void) | null = null;
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onclose: ((ev: CloseEvent) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;
  private unsubscribe: (() => void) | null = null;
  private inner: SocketLike | null = null;
  private closed = false;

  constructor() {
    void waitForBackend().then((b) => {
      if (this.closed) return;
      if (b.openEvents) {
        const inner = b.openEvents();
        this.inner = inner;
        inner.onopen = (e) => this.onopen?.(e);
        inner.onmessage = (e) => !this.closed && this.onmessage?.(e);
        inner.onerror = (e) => this.onerror?.(e);
        inner.onclose = (e) => {
          if (this.closed) return;
          this.closed = true;
          this.onclose?.(e);
        };
        return;
      }
      this.unsubscribe = b.subscribe((event) => {
        if (this.closed) return;
        const data = typeof event === "string" ? event : JSON.stringify(event);
        this.onmessage?.(new MessageEvent("message", { data }));
      });
      this.onopen?.(new Event("open"));
    });
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.unsubscribe?.();
    this.unsubscribe = null;
    if (this.inner) {
      this.inner.onclose = null;
      this.inner.close();
      this.inner = null;
    }
    // CloseEvent is missing outside browsers (the Node test runner): a plain Event then.
    const ev = typeof CloseEvent === "function" ? new CloseEvent("close", { wasClean: true, code: 1000 }) : (new Event("close") as CloseEvent);
    this.onclose?.(ev);
  }
}

// --- The worker backend ------------------------------------------------------------------------------------------

/** UI to worker: one request. `query` is the part after "?" (no "?"), `body` the raw bytes (empty when none). */
export interface WorkerRequest {
  id: number;
  method: string;
  path: string;
  query: string;
  headers: Record<string, string>;
  body: Uint8Array;
}

/** Worker to UI: the answer to request `id`. */
export interface WorkerReply {
  kind?: "reply";
  id: number;
  status: number;
  headers: [string, string][] | Record<string, string>;
  body: Uint8Array | ArrayBuffer | null;
}

/** Worker to UI: an engine event (the parsed object, or its JSON text). */
export interface WorkerEvent {
  kind: "event";
  data: unknown;
}

/** Worker to UI: the worker cannot go on (a wasm panic); pending requests fail with `message`. */
export interface WorkerFailure {
  kind: "fatal";
  message: string;
}

export type WorkerMessage = WorkerReply | WorkerEvent | WorkerFailure;

/** The minimum of a Worker this needs (a real Worker, or a stand-in in tests). */
export interface WorkerPort {
  postMessage(msg: unknown, transfer: Transferable[]): void;
  addEventListener(type: "message", fn: (ev: MessageEvent) => void): void;
  addEventListener(type: "error", fn: (ev: Event) => void): void;
}

/** Statuses whose Response must have no body. */
const NULL_BODY = new Set([101, 103, 204, 205, 304]);

/** Split "/api/x?a=1" into its path and query (without "?"). */
export function splitPath(path: string): { path: string; query: string } {
  const i = path.indexOf("?");
  return i < 0 ? { path, query: "" } : { path: path.slice(0, i), query: path.slice(i + 1) };
}

/** A request body as bytes (strings, Blobs, buffers and views alike); empty when there is none. */
export async function bodyBytes(body: BodyInit | null | undefined): Promise<Uint8Array> {
  if (body === null || body === undefined) return new Uint8Array(0);
  if (typeof body === "string") return new TextEncoder().encode(body);
  if (body instanceof Uint8Array) return body.slice();
  if (body instanceof ArrayBuffer) return new Uint8Array(body.slice(0));
  return new Uint8Array(await new Response(body).arrayBuffer());
}

/** Plain lower-case header names, from any HeadersInit. */
export function headerRecord(h: HeadersInit | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  if (!h) return out;
  new Headers(h).forEach((v, k) => (out[k] = v));
  return out;
}

/** The Response for a worker's reply. */
export function replyResponse(r: WorkerReply): Response {
  const headers = new Headers(Array.isArray(r.headers) ? r.headers : Object.entries(r.headers ?? {}));
  const body = NULL_BODY.has(r.status) || !r.body ? null : r.body instanceof ArrayBuffer ? new Uint8Array(r.body) : r.body;
  return new Response(body as BodyInit | null, { status: r.status, headers });
}

/**
 * A DesignBackend over a Dedicated Web Worker that speaks the protocol above: requests are matched to replies by id
 * and bodies are transferred, not copied. When the worker fails, every waiting request rejects (the wrappers report
 * "engine unreachable") and `onFatal` is told, so the shell can restart it.
 */
export function workerBackend(worker: WorkerPort, onFatal?: (message: string) => void): DesignBackend {
  let next = 1;
  const pending = new Map<number, { resolve: (r: Response) => void; reject: (e: Error) => void }>();
  const listeners = new Set<(event: unknown) => void>();
  const failAll = (message: string) => {
    for (const p of pending.values()) p.reject(new TypeError(message));
    pending.clear();
    onFatal?.(message);
  };
  worker.addEventListener("message", (ev: MessageEvent) => {
    const m = ev.data as WorkerMessage | null;
    if (!m || typeof m !== "object") return;
    if (m.kind === "event") {
      for (const fn of [...listeners]) fn(m.data);
      return;
    }
    if (m.kind === "fatal") {
      failAll(m.message || "the design backend stopped");
      return;
    }
    const p = pending.get(m.id);
    if (!p) return;
    pending.delete(m.id);
    try {
      p.resolve(replyResponse(m));
    } catch (e) {
      p.reject(e instanceof Error ? e : new Error(String(e)));
    }
  });
  worker.addEventListener("error", (ev: Event) => failAll((ev as ErrorEvent).message || "the design backend stopped"));
  return {
    async request(method, fullPath, init) {
      const { path, query } = splitPath(fullPath);
      const body = await bodyBytes(init.body);
      const id = next++;
      const msg: WorkerRequest = { id, method: method.toUpperCase(), path, query, headers: headerRecord(init.headers), body };
      return new Promise<Response>((resolve, reject) => {
        pending.set(id, { resolve, reject });
        worker.postMessage(msg, body.byteLength ? [body.buffer] : []);
      });
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => void listeners.delete(listener);
    },
  };
}

/**
 * What a machine call answers in OpenEtch Design: its UI is hidden there, so this is only the safety net. The API
 * objects use it as `name: DESIGN_ONLY ? noMachine : (...) => ...`, a constant test, so the design bundle carries
 * neither the call nor its route. Typed `never` so the entry keeps the full app's signature.
 */
export const noMachine: never = ((..._args: unknown[]): Promise<never> =>
  Promise.reject(new Error("Not available in OpenEtch Design: it has no laser to control."))) as never;
