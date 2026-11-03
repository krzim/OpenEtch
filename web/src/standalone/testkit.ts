// Test helpers for the worker shell (node --test): a "tab" is a ShellBackend over the mock webcore and a
// MemoryStore, with a fake clock, timers that only fire when asked, and a BroadcastChannel pair. Not used by the app.

import type { Timers } from "./autosave.ts";
import { ShellBackend, type ChannelLike } from "./backend.ts";
import { MemoryStore } from "./idb.ts";
import { MockCore } from "./mockCore.ts";
import { splitPath } from "./rpc.ts";

export interface Clock {
  t: number;
}

/** Timers that run only when `fire()` is called. */
export function manualTimers(): Timers & { fire(): void; count(): number } {
  const due = new Map<number, () => void>();
  let n = 0;
  return {
    setTimeout: (fn) => {
      due.set(++n, fn);
      return n;
    },
    clearTimeout: (t) => void due.delete(t as number),
    fire() {
      const fns = [...due.values()];
      due.clear();
      for (const f of fns) f();
    },
    count: () => due.size,
  };
}

/** Two connected channels, like two tabs' BroadcastChannels of one name (messages are cloned, delivered later). */
export function channelPair(): [ChannelLike, ChannelLike] {
  const a: ChannelLike = { onmessage: null, postMessage: (m) => setTimeout(() => b.onmessage?.({ data: structuredClone(m) }), 0), close() {} };
  const b: ChannelLike = { onmessage: null, postMessage: (m) => setTimeout(() => a.onmessage?.({ data: structuredClone(m) }), 0), close() {} };
  return [a, b];
}

export interface Answer {
  status: number;
  headers: [string, string][];
  body: Uint8Array;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  json: any;
}

export function makeTab(opts: { store?: MemoryStore; sessionId?: string; channel?: ChannelLike | null; clock?: Clock; core?: MockCore } = {}) {
  const store = opts.store ?? new MemoryStore();
  const clock = opts.clock ?? { t: Date.UTC(2026, 10, 3, 9, 0, 0) };
  const core = opts.core ?? new MockCore();
  const events: string[] = [];
  const crashes: string[] = [];
  const timers = manualTimers();
  const backend = new ShellBackend({
    core,
    store,
    sessionId: opts.sessionId ?? "tab-a",
    now: () => clock.t,
    channel: opts.channel ?? null,
    timers,
    onEvent: (e) => events.push(e),
    onCrash: (e) => crashes.push(e),
  });
  async function call(method: string, url: string, body?: unknown): Promise<Answer> {
    const { path, query } = splitPath(url);
    const bytes = body === undefined ? new Uint8Array() : body instanceof Uint8Array ? body : new TextEncoder().encode(typeof body === "string" ? body : JSON.stringify(body));
    const r = await backend.request({ method, path, query, body: bytes });
    let j: unknown = null;
    try {
      j = JSON.parse(new TextDecoder().decode(r.body));
    } catch {
      j = null;
    }
    return { ...r, json: j };
  }
  /** Event types posted so far. */
  const types = () => events.map((e) => (JSON.parse(e) as { type: string }).type);
  return { store, clock, core, events, types, crashes, timers, backend, call };
}

/** Let channel messages and queued work finish. */
export async function settle(...tabs: { backend: ShellBackend }[]): Promise<void> {
  for (let i = 0; i < 3; i++) {
    await new Promise((r) => setTimeout(r, 2));
    for (const t of tabs) await t.backend.enqueue(async () => undefined);
  }
}
