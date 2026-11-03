// The backend worker's logic, independent of the worker itself so Node can test it (backend.test.ts): requests in
// order, one at a time (webcore is synchronous; shell routes await IndexedDB between webcore calls, and nothing may
// interleave there); the shell's own routes first, webcore for the rest; webcore's store writes persisted before the
// answer; its events to the page; autosave, the recovery copy, and other tabs' changes through a BroadcastChannel.
//
// A webcore panic poisons the instance: the request gets a 500 (`code: "editor_crashed"`), later ones a 503, and
// `onCrash` tells the worker, whose page then starts a fresh worker that restores the recovery copy (rpc.ts).

import { Autosaver, RecoveryKeeper, type Timers } from "./autosave.ts";
import type { BootKind, CoreReply, CoreWrite, Recovered, WebcoreBackend } from "./contract.ts";
import { normaliseReply } from "./core.ts";
import { loadFont, loadFonts } from "./fonts.ts";
import { StorageFullError, type KeyValueStore } from "./idb.ts";
import { CORE_PREFIX, S, SHARED_STORES, type ShellContext } from "./records.ts";
import { json, type ShellReply, type ShellRequest } from "./reply.ts";
import { shellRoute } from "./routes.ts";

/** The BroadcastChannel name every tab of the design build shares. */
export const CHANNEL = "openetch-design";

/** BroadcastChannel's part the backend uses (a fake in tests). */
export interface ChannelLike {
  postMessage(m: unknown): void;
  onmessage: ((e: { data: unknown }) => void) | null;
  close(): void;
}

/** What one tab tells the others. */
export interface ChangeMessage {
  kind: "changed";
  from: string;
  /** Shared shell stores that changed (projects, templates, ...). */
  stores: string[];
  /** webcore records that changed (without the prefix). */
  core: { store: string; key: string }[];
}

export interface BackendOptions {
  core: WebcoreBackend;
  store: KeyValueStore;
  sessionId: string;
  /** Unix ms (tests pass a fake clock). */
  now?: () => number;
  channel?: ChannelLike | null;
  timers?: Timers;
  recovery?: { debounceMs?: number; minGapMs?: number };
  onEvent(data: string): void;
  onCrash(error: string): void;
}

/** Events after which the recovery copy is due. */
const CHANGES = new Set(["document_changed", "history_changed", "documents_changed", "project_saved"]);

class Crashed extends Error {}

export class ShellBackend {
  readonly ctx: ShellContext;
  readonly autosaver: Autosaver;
  readonly recovery: RecoveryKeeper;
  private readonly core: WebcoreBackend;
  private readonly opts: BackendOptions;
  private queue: Promise<unknown> = Promise.resolve();
  private crashed: string | null = null;
  private outbox: ChangeMessage | null = null;
  private readonly fontsLoaded = new Set<string>();

  constructor(opts: BackendOptions) {
    this.opts = opts;
    this.core = opts.core;
    const now = opts.now ?? (() => Date.now());
    this.ctx = {
      store: opts.store,
      core: (m, p, q = "", b = new Uint8Array()) => this.callCore(m, p, q, b),
      now,
      sessionId: opts.sessionId,
      changed: (stores) => this.queueChange(stores, []),
      emit: (e) => opts.onEvent(e),
      known: new Map(),
    };
    this.autosaver = new Autosaver(this.ctx);
    this.recovery = new RecoveryKeeper(this.ctx, (fn) => this.enqueue(fn), { timers: opts.timers, ...opts.recovery });
    if (opts.channel) opts.channel.onmessage = (e) => void this.heard(e.data);
  }

  /** Whether webcore panicked (the worker is to be replaced). */
  get isCrashed(): boolean {
    return this.crashed !== null;
  }

  /** Run `fn` after everything queued before it, alone. */
  enqueue<T>(fn: () => Promise<T>): Promise<T> {
    const run = this.queue.then(fn, fn);
    this.queue = run.catch(() => undefined);
    return run;
  }

  /** Load the stores into webcore, then restore the tab's document for a reload or a restart. */
  boot(kind: BootKind, tzOffsetMin = 0): Promise<Recovered | null> {
    return this.enqueue(async () => {
      for (const [store, key, value] of await this.opts.store.scan<string | Uint8Array>(CORE_PREFIX)) {
        await this.callCore("PUT", "/api/_shell/store", storeQuery(store.slice(CORE_PREFIX.length), key), asBytes(value));
      }
      await this.callCore("POST", "/api/_shell/boot", "", asBytes(JSON.stringify({ now_unix_ms: this.ctx.now(), tz_offset_min: tzOffsetMin })));
      for (const f of await loadFonts(this.ctx)) this.fontsLoaded.add(f);
      await RecoveryKeeper.prune(this.ctx).catch(() => undefined);
      if (kind === "fresh") return null;
      try {
        return await this.recovery.recover();
      } catch (e) {
        if (!(e instanceof Crashed)) throw e;
        // The copy itself makes webcore panic: drop it, or every restart and reload would panic again.
        await this.opts.store
          .batch([
            { store: S.recovery, key: this.opts.sessionId, value: null },
            { store: S.recoveryData, key: this.opts.sessionId, value: null },
          ])
          .catch(() => undefined);
        return null;
      }
    });
  }

  /** One request from the page. */
  request(req: ShellRequest): Promise<ShellReply> {
    return this.enqueue(async () => {
      if (this.crashed) return crashedReply(503, "editor_restarting");
      if (req.path.startsWith("/api/_shell/")) return json({ ok: false, error: "no such API route" }, 404);
      try {
        const own = await shellRoute(this.ctx, req);
        if (own) return own;
        const r = await this.callCore(req.method, req.path, req.query, req.body);
        return { status: r.status, headers: r.headers, body: r.body };
      } catch (e) {
        if (e instanceof Crashed) return crashedReply(500, "editor_crashed");
        if (e instanceof StorageFullError) return json({ ok: false, error: e.message, code: "storage_full" }, 507);
        return json({ ok: false, error: e instanceof Error ? e.message : String(e) }, 500);
      }
    });
  }

  /** The 30 s timer: autosave when due. */
  tick(): Promise<void> {
    return this.enqueue(async () => {
      if (!this.crashed) await this.autosaver.tick().catch(() => undefined);
    });
  }

  /** The page is hidden or closing: write the recovery copy now. */
  flush(): Promise<void> {
    return this.enqueue(async () => {
      if (!this.crashed) await this.recovery.writeNow().catch(() => undefined);
    });
  }

  private async callCore(method: string, path: string, query: string, body: Uint8Array): Promise<CoreReply> {
    if (this.crashed) throw new Crashed(this.crashed);
    let r: CoreReply;
    try {
      r = normaliseReply(this.core.handle(method, path, query, body));
    } catch (e) {
      this.crashed = e instanceof Error ? e.message : String(e);
      this.opts.onCrash(this.crashed);
      throw new Crashed(this.crashed);
    }
    if (r.writes.length) await this.persist(r.writes);
    let changed = false;
    for (const ev of r.events) {
      this.opts.onEvent(ev);
      if (!changed && CHANGES.has(eventType(ev))) changed = true;
    }
    if (changed) this.recovery.noteChange();
    return r;
  }

  private async persist(writes: CoreWrite[]): Promise<void> {
    await this.opts.store.batch(writes.map((w) => ({ store: CORE_PREFIX + w.store, key: w.key, value: w.value })));
    this.queueChange([], writes.map((w) => ({ store: w.store, key: w.key })));
  }

  /** Collect changes and tell the other tabs once, after the current request. */
  private queueChange(stores: string[], core: { store: string; key: string }[]): void {
    const ch = this.opts.channel;
    if (!ch) return;
    const shared = stores.filter((s) => SHARED_STORES.includes(s));
    if (!shared.length && !core.length) return;
    if (!this.outbox) {
      this.outbox = { kind: "changed", from: this.opts.sessionId, stores: [], core: [] };
      queueMicrotask(() => {
        const m = this.outbox;
        this.outbox = null;
        if (m) ch.postMessage(m);
      });
    }
    for (const s of shared) if (!this.outbox.stores.includes(s)) this.outbox.stores.push(s);
    this.outbox.core.push(...core);
  }

  /** Another tab changed stores: load webcore's records again, add new fonts, and tell the page. */
  private async heard(data: unknown): Promise<void> {
    const m = data as Partial<ChangeMessage> | null;
    if (!m || m.kind !== "changed" || m.from === this.opts.sessionId) return;
    const stores = Array.isArray(m.stores) ? m.stores.filter((s): s is string => typeof s === "string") : [];
    const core = Array.isArray(m.core) ? m.core.filter((c) => c && typeof c.store === "string" && typeof c.key === "string") : [];
    await this.enqueue(async () => {
      if (this.crashed) return;
      const seen = new Set<string>();
      for (const { store, key } of core) {
        const id = `${store}\u0000${key}`;
        if (seen.has(id)) continue;
        seen.add(id);
        const v = await this.opts.store.get<string | Uint8Array>(CORE_PREFIX + store, key);
        if (v === undefined || v === null) await this.callCore("DELETE", "/api/_shell/store", storeQuery(store, key), new Uint8Array());
        else await this.callCore("PUT", "/api/_shell/store", storeQuery(store, key), asBytes(v));
      }
      if (stores.includes(S.fonts)) {
        for (const name of await this.opts.store.keys(S.fonts)) {
          if (this.fontsLoaded.has(name)) continue;
          await loadFont(this.ctx, name);
          this.fontsLoaded.add(name);
        }
      }
      const names = [...new Set([...stores, ...core.map((c) => c.store)])];
      this.opts.onEvent(JSON.stringify({ type: "stores_changed", stores: names }));
    }).catch(() => undefined);
  }
}

function storeQuery(store: string, key: string): string {
  return new URLSearchParams({ store, key }).toString();
}

function asBytes(v: string | Uint8Array): Uint8Array {
  return typeof v === "string" ? new TextEncoder().encode(v) : v;
}

function eventType(ev: string): string {
  try {
    const t = (JSON.parse(ev) as { type?: unknown }).type;
    return typeof t === "string" ? t : "";
  } catch {
    return "";
  }
}

/** Header of a request a crashed backend did not run: the page sends it again to the next worker. */
export const RESTARTING_HEADER = "x-openetch-restarting";

function crashedReply(status: number, code: string): ShellReply {
  const r = json(
    {
      ok: false,
      error: "The editor stopped after an internal error and is restarting with your last recovery copy. Please try again in a moment.",
      code,
    },
    status,
  );
  if (code === "editor_restarting") r.headers.push([RESTARTING_HEADER, "1"]);
  return r;
}
