// The page's side of the design-only backend: `/api/...` requests as messages to the backend worker (worker.ts),
// answered as real `Response`s, so every fetch wrapper, `ApiError` and `Content-Disposition` download works as it
// does against the engine; and a `FakeSocket` in place of the engine's `/api/events` WebSocket.
//
// web/src/api/transport.ts (WP5) is expected to call, in the design-only build:
//
//   transport(method, path, init)  →  standaloneFetch(method, path, init)
//   new WebSocket(eventsUrl())     →  standaloneEvents()
//
// and to show `onStandaloneNotice` notices as toasts (their `key` is a message in workspace/locales).
//
// Request ids match replies. A request that gets no answer in `timeoutMs` (webcore stuck in a loop) is answered 504
// and the worker is replaced; requests waiting behind it go to the new worker. When webcore panics the worker says so (`crashed`): it is terminated and a fresh one
// starts, restoring the tab's recovery copy; open event streams close (useEngine reconnects and fetches everything
// again) and a notice says "The editor restarted; recovered autosave from hh:mm". After three crashes within a
// minute the page stops restarting and every request fails with 503.

import type { BootKind, FromWorker, Notice, PreviewKind, PreviewResult, Recovered, ToWorker } from "./contract.ts";

/** The Worker's part used here (a fake in tests). */
export interface WorkerLike {
  postMessage(m: ToWorker, transfer?: Transferable[]): void;
  onmessage: ((e: { data: FromWorker }) => void) | null;
  onerror: ((e: unknown) => void) | null;
  terminate(): void;
}

/** sessionStorage's part used here. */
export interface SessionStore {
  getItem(k: string): string | null;
  setItem(k: string, v: string): void;
}

export interface RpcOptions {
  createWorker(): WorkerLike;
  /** Where the session id lives (it survives a reload of the tab, not a new tab). */
  session?: SessionStore | null;
  /** How long one request may take (default 5 minutes: a 50 MB image trace is slow in wasm). */
  timeoutMs?: number;
  /** navigator.storage.persist, asked once after the first save. */
  persist?: (() => Promise<boolean>) | null;
  /** Local time zone offset in minutes east of UTC (for `{date}` text). */
  tzOffsetMin?: number;
  /** Clock and timers, injectable for tests. */
  now?: () => number;
  setTimer?: (fn: () => void, ms: number) => unknown;
  clearTimer?: (t: unknown) => void;
  newId?: () => string;
}

export const SESSION_KEY = "openetch.design.session";
const RESTART_WINDOW_MS = 60_000;
const MAX_RESTARTS = 3;

/** The engine's `/api/events` WebSocket, as useEngine uses it. */
export interface SocketLike {
  readonly readyState: number;
  onopen: ((e: unknown) => void) | null;
  onmessage: ((e: { data: string }) => void) | null;
  onclose: ((e: unknown) => void) | null;
  onerror: ((e: unknown) => void) | null;
  send(data: string): void;
  close(): void;
}

/** Event stream fed by the worker's `event` messages; it closes when the worker is replaced. */
export class FakeSocket implements SocketLike {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 3;
  readyState = FakeSocket.CONNECTING;
  onopen: ((e: unknown) => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: ((e: unknown) => void) | null = null;
  onerror: ((e: unknown) => void) | null = null;
  private readonly detach: (s: FakeSocket) => void;

  constructor(detach: (s: FakeSocket) => void) {
    this.detach = detach;
  }

  /** Called by the client. */
  open(): void {
    if (this.readyState !== FakeSocket.CONNECTING) return;
    this.readyState = FakeSocket.OPEN;
    this.onopen?.({ type: "open" });
  }

  deliver(data: string): void {
    if (this.readyState === FakeSocket.OPEN) this.onmessage?.({ data });
  }

  /** The worker went away: like a dropped connection. */
  drop(): void {
    if (this.readyState === FakeSocket.CLOSED) return;
    this.readyState = FakeSocket.CLOSED;
    this.detach(this);
    this.onclose?.({ type: "close", code: 1006 });
  }

  send(): void {
    // The engine's socket takes nothing from the page either.
  }

  close(): void {
    if (this.readyState === FakeSocket.CLOSED) return;
    this.readyState = FakeSocket.CLOSED;
    this.detach(this);
    this.onclose?.({ type: "close", code: 1000 });
  }
}

interface Pending {
  resolve: (r: Response) => void;
  timer: unknown;
  /** Kept until the worker is ready (or restarted). */
  msg: Extract<ToWorker, { kind: "request" }>;
  sent: boolean;
}

type State = "starting" | "ready" | "failed";

/** hh:mm in local time. */
export function clock(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function abortError(): Error {
  return typeof DOMException === "function" ? new DOMException("The operation was aborted.", "AbortError") : Object.assign(new Error("aborted"), { name: "AbortError" });
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

/** Split `/api/x?y=1` (or a full URL) into path and query. */
export function splitPath(path: string): { path: string; query: string } {
  let p = path;
  if (/^[a-z]+:\/\//i.test(p)) {
    const u = new URL(p);
    p = u.pathname + u.search;
  }
  const i = p.indexOf("?");
  return i < 0 ? { path: p, query: "" } : { path: p.slice(0, i), query: p.slice(i + 1) };
}

export class StandaloneClient {
  private readonly opts: RpcOptions;
  private worker: WorkerLike | null = null;
  private state: State = "starting";
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  /** Preview jobs sent to the current worker (a restart drops them: the preview client asks again). */
  private readonly previews = new Map<number, { resolve: (r: PreviewResult & { ms: number }) => void; reject: (e: Error) => void }>();
  private readonly readyWaiters: ((ok: boolean) => void)[] = [];
  private readonly sockets = new Set<FakeSocket>();
  private readonly noticeListeners = new Set<(n: Notice) => void>();
  private restarts: number[] = [];
  private persistAsked = false;
  private readonly sessionId: string;
  private failure = "";
  /** A restarted worker is starting (its ready brings the restart notice). */
  private restarting = false;
  /** The "nothing outlives this tab" notice was shown. */
  private toldMemory = false;
  /** The last recovery, for tests and the page. */
  lastRecovered: Recovered | null = null;
  storage: "indexeddb" | "memory" | null = null;

  constructor(opts: RpcOptions) {
    this.opts = opts;
    let id: string | null = null;
    let boot: BootKind = "fresh";
    try {
      id = opts.session?.getItem(SESSION_KEY) ?? null;
    } catch {
      id = null;
    }
    if (id) boot = "reload";
    else {
      id = (opts.newId ?? defaultId)();
      try {
        opts.session?.setItem(SESSION_KEY, id);
      } catch {
        // Without sessionStorage a reload starts fresh.
      }
    }
    this.sessionId = id;
    this.spawn(boot);
  }

  /** The `fetch` of the design-only build: `/api/...` answered by the backend worker. */
  async fetch(method: string, path: string, init: RequestInit = {}): Promise<Response> {
    const { path: p, query } = splitPath(path);
    if (p.startsWith("/api/_shell/")) return json(404, { ok: false, error: "no such API route" });
    if (this.state === "failed") return json(503, { ok: false, error: this.failure || "The editor could not start in this browser.", code: "editor_failed" });
    const body = init.body === undefined || init.body === null ? new Uint8Array() : new Uint8Array(await new Response(init.body).arrayBuffer());
    const headers: [string, string][] = [];
    new Headers(init.headers).forEach((v, k) => headers.push([k, v]));
    const id = this.nextId++;
    const msg: Extract<ToWorker, { kind: "request" }> = { kind: "request", id, method: method.toUpperCase(), path: p, query, headers, body };
    const signal = init.signal;
    if (signal?.aborted) throw abortError();
    const res = await new Promise<Response>((resolve, reject) => {
      const timer = this.setTimer(() => this.timedOut(id), this.opts.timeoutMs ?? 300_000);
      this.pending.set(id, { resolve, timer, msg, sent: false });
      // Like fetch: the promise rejects; the worker may still run the request.
      signal?.addEventListener(
        "abort",
        () => {
          if (!this.pending.delete(id)) return;
          this.clearTimer(timer);
          reject(abortError());
        },
        { once: true },
      );
      if (this.state === "ready") this.send(id);
    });
    if (msg.method === "POST" && p === "/api/projects/save" && res.ok) this.askPersist();
    return res;
  }

  /** Resolves true when the worker is ready, false when it could not start. */
  whenReady(): Promise<boolean> {
    if (this.state === "ready") return Promise.resolve(true);
    if (this.state === "failed") return Promise.resolve(false);
    return new Promise((r) => this.readyWaiters.push(r));
  }

  /**
   * A preview job on the backend's own document (web/src/wasm/previewClient.ts in the design build): run in order
   * with the requests, so it is of the document every earlier request left.
   */
  preview(job: PreviewKind, params?: unknown): Promise<PreviewResult & { ms: number }> {
    if (this.state !== "ready" || !this.worker) return Promise.reject(new Error("the editor is not ready"));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.previews.set(id, { resolve, reject });
      this.worker?.postMessage({ kind: "preview", id, job, params });
    });
  }

  /** A FakeSocket that opens once the worker is ready. */
  events(): FakeSocket {
    const s = new FakeSocket((x) => this.sockets.delete(x));
    this.sockets.add(s);
    if (this.state === "ready") queueMicrotask(() => s.open());
    else if (this.state === "failed") queueMicrotask(() => s.drop());
    return s;
  }

  onNotice(fn: (n: Notice) => void): () => void {
    this.noticeListeners.add(fn);
    return () => void this.noticeListeners.delete(fn);
  }

  /** The page is being hidden: ask for the recovery copy now. */
  flush(): void {
    if (this.state === "ready") this.worker?.postMessage({ kind: "flush" });
  }

  terminate(): void {
    this.worker?.terminate();
    this.worker = null;
    for (const s of [...this.sockets]) s.drop();
  }

  private setTimer(fn: () => void, ms: number): unknown {
    return (this.opts.setTimer ?? ((f, t) => globalThis.setTimeout(f, t)))(fn, ms);
  }

  private clearTimer(t: unknown): void {
    (this.opts.clearTimer ?? ((x) => globalThis.clearTimeout(x as ReturnType<typeof setTimeout>)))(t);
  }

  private now(): number {
    return (this.opts.now ?? Date.now)();
  }

  private spawn(boot: BootKind): void {
    this.state = "starting";
    let w: WorkerLike;
    try {
      w = this.opts.createWorker();
    } catch (e) {
      this.fail(e instanceof Error ? e.message : String(e));
      return;
    }
    this.worker = w;
    w.onmessage = (e) => {
      if (this.worker === w) this.received(e.data);
    };
    w.onerror = (e) => {
      if (this.worker !== w) return;
      const msg = (e as { message?: unknown } | null)?.message;
      // A script error before `ready` is a broken build: retrying would loop.
      noteCrash(`the editor's worker failed: ${typeof msg === "string" ? msg : String(e)}`);
      if (this.state === "starting") this.fail(typeof msg === "string" ? msg : "The editor could not start in this browser.");
      else this.restart();
    };
    w.postMessage({ kind: "init", sessionId: this.sessionId, boot, tzOffsetMin: this.opts.tzOffsetMin ?? -new Date().getTimezoneOffset() });
  }

  private received(m: FromWorker): void {
    switch (m.kind) {
      case "ready": {
        const wasRestart = this.restarting;
        this.restarting = false;
        this.state = "ready";
        this.storage = m.storage;
        this.lastRecovered = m.recovered;
        for (const id of this.pending.keys()) this.send(id);
        for (const s of this.sockets) s.open();
        for (const r of this.readyWaiters.splice(0)) r(true);
        if (wasRestart) {
          this.notify(
            m.recovered
              ? { key: "ds.restartedRecovered", vars: { time: clock(m.recovered.savedMs) }, level: "warning" }
              : { key: "ds.restartedNothing", vars: {}, level: "warning" },
          );
        } else if (m.recovered?.source === "recovery") {
          this.notify({ key: "ds.reloadRecovered", vars: { time: clock(m.recovered.savedMs) }, level: "info" });
        }
        if (m.storage === "memory" && !this.toldMemory) {
          this.toldMemory = true;
          this.notify({ key: "ds.storageMemory", vars: {}, level: "warning" });
        }
        break;
      }
      case "failed":
        this.fail(m.error);
        break;
      case "reply": {
        const p = this.pending.get(m.id);
        if (!p) return;
        if (m.headers.some(([k]) => k.toLowerCase() === "x-openetch-restarting")) {
          // The crashed worker did not run it: it goes to the next worker.
          p.sent = false;
          return;
        }
        this.pending.delete(m.id);
        this.clearTimer(p.timer);
        const nobody = m.status === 204 || m.status === 304;
        p.resolve(new Response(nobody ? null : m.body, { status: m.status, headers: m.headers }));
        break;
      }
      case "event":
        for (const s of this.sockets) s.deliver(m.data);
        break;
      case "crashed":
        noteCrash(`the editor stopped: ${m.error}`);
        this.restart();
        break;
      case "preview": {
        const p = this.previews.get(m.id);
        if (!p) return;
        this.previews.delete(m.id);
        if (m.result) p.resolve({ ...m.result, ms: m.ms });
        else p.reject(new Error(m.error ?? "the preview failed"));
        break;
      }
    }
  }

  private send(id: number): void {
    const p = this.pending.get(id);
    if (!p || p.sent || !this.worker) return;
    p.sent = true;
    // Copied, not transferred: a restarted worker may need the body again.
    this.worker.postMessage(p.msg);
  }

  private timedOut(id: number): void {
    const p = this.pending.get(id);
    if (!p) return;
    this.pending.delete(id);
    p.resolve(json(504, { ok: false, error: "The editor did not answer in time and is restarting.", code: "editor_timeout" }));
    this.restart();
  }

  /**
   * Replace the worker. Requests sent to it and not answered were never run: the worker runs one at a time, the one
   * that crashed got its own answer before `crashed`, and a stuck one got its 504. They go to the new worker.
   */
  private restart(): void {
    const t = this.now();
    this.restarts = this.restarts.filter((x) => t - x < RESTART_WINDOW_MS);
    this.restarts.push(t);
    this.worker?.terminate();
    this.worker = null;
    for (const s of [...this.sockets]) s.drop();
    for (const p of this.pending.values()) p.sent = false;
    this.dropPreviews("the editor restarted");
    if (this.restarts.length > MAX_RESTARTS) {
      this.fail("The editor stopped several times in a row. Download a copy of your work if you can, then reload the page.");
      return;
    }
    this.restarting = true;
    this.spawn("restart");
  }

  private dropPreviews(why: string): void {
    for (const p of this.previews.values()) p.reject(new Error(why));
    this.previews.clear();
  }

  private fail(error: string): void {
    this.state = "failed";
    this.failure = error;
    this.dropPreviews(error);
    for (const r of this.readyWaiters.splice(0)) r(false);
    this.worker?.terminate();
    this.worker = null;
    for (const [id, p] of this.pending) {
      this.pending.delete(id);
      this.clearTimer(p.timer);
      p.resolve(json(503, { ok: false, error, code: "editor_failed" }));
    }
    for (const s of [...this.sockets]) s.drop();
    this.notify({ key: "ds.failed", vars: { error }, level: "error" });
  }

  private notify(n: Notice): void {
    for (const fn of [...this.noticeListeners]) fn(n);
  }

  private askPersist(): void {
    if (this.persistAsked) return;
    this.persistAsked = true;
    const p = this.opts.persist;
    if (p) void p().catch(() => false);
  }
}

/** Why the worker stopped, for the console and the device harness (`window.__oeDesignCrashes`). */
function noteCrash(text: string): void {
  console.error(`[design] ${text}`);
  const g = globalThis as { __oeDesignCrashes?: string[] };
  (g.__oeDesignCrashes ??= []).push(text.slice(0, 600));
}

function defaultId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c?.randomUUID) return c.randomUUID();
  return `s${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`;
}

// --- The page's single client -------------------------------------------------------------------------------------

let client: StandaloneClient | null = null;
const listeners = new Set<(n: Notice) => void>();
/** Notices from before anyone listened (a reload recovered before the app mounted), given to the first listener. */
const missed: Notice[] = [];

function browserSession(): SessionStore | null {
  try {
    return globalThis.sessionStorage ?? null;
  } catch {
    return null;
  }
}

/** The page's client, started on first use (the worker, its pagehide flush and the persist request). */
export function standaloneClient(): StandaloneClient {
  if (client) return client;
  const c = new StandaloneClient({
    createWorker: () => new Worker(new URL("./worker.ts", import.meta.url), { type: "module" }) as unknown as WorkerLike,
    session: browserSession(),
    persist: () => (navigator.storage?.persist ? navigator.storage.persist() : Promise.resolve(false)),
  });
  c.onNotice((n) => {
    if (listeners.size === 0) missed.push(n);
    for (const fn of [...listeners]) fn(n);
  });
  const flush = () => c.flush();
  globalThis.addEventListener?.("pagehide", flush);
  globalThis.document?.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") flush();
  });
  client = c;
  return c;
}

/** transport() of the design-only build. */
export function standaloneFetch(method: string, path: string, init?: RequestInit): Promise<Response> {
  return standaloneClient().fetch(method, path, init);
}

/** The engine's event WebSocket in the design-only build. */
export function standaloneEvents(): SocketLike {
  return standaloneClient().events();
}

/** Notices for the page's toasts; registered before or after the client starts. */
export function onStandaloneNotice(fn: (n: Notice) => void): () => void {
  listeners.add(fn);
  for (const n of missed.splice(0)) fn(n);
  return () => void listeners.delete(fn);
}
