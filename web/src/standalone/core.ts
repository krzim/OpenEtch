// Loading webcore for the backend worker: the real wasm (crates/webcore's `Backend`, through the wasm-bindgen glue
// "webcore-pkg") or, with VITE_STANDALONE_MOCK=1, the mock (mockCore.ts) until the core track lands. The swap
// point for integration is `loadRealCore` and `normaliseReply` below; the rest of the shell sees only
// contract.ts `WebcoreBackend`.

import type { CoreReply, CoreWrite, WebcoreBackend } from "./contract.ts";

/** The wasm-bindgen glue's shape the shell expects (crates/webcore/src/lib.rs, `#[wasm_bindgen] pub struct Backend`). */
interface BackendPkg {
  default: (opts?: { module_or_path?: unknown }) => Promise<unknown>;
  Backend?: new () => { handle(method: string, path: string, query: string, body: Uint8Array): unknown; free(): void };
}

export async function loadRealCore(): Promise<WebcoreBackend> {
  const pkg = (await import("webcore-pkg")) as unknown as BackendPkg;
  await pkg.default();
  if (typeof pkg.Backend !== "function") throw new Error("this build of the in-browser core has no Backend (crates/webcore)");
  const b = new pkg.Backend();
  return {
    handle: (method, path, query, body) => normaliseReply(b.handle(method, path, query, body)),
    free: () => b.free(),
  };
}

export async function loadMockCore(): Promise<WebcoreBackend> {
  const { MockCore } = await import("./mockCore.ts");
  return new MockCore();
}

/** The core this build uses. */
export function loadCore(): Promise<WebcoreBackend> {
  return import.meta.env.VITE_STANDALONE_MOCK === "1" ? loadMockCore() : loadRealCore();
}

const enc = new TextEncoder();

function toBytes(v: unknown): Uint8Array {
  if (v instanceof Uint8Array) return v;
  if (v instanceof ArrayBuffer) return new Uint8Array(v);
  if (ArrayBuffer.isView(v)) return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  if (typeof v === "string") return enc.encode(v);
  return new Uint8Array();
}

function listOf(v: unknown): unknown[] {
  if (Array.isArray(v)) return v;
  if (typeof v === "string" && v.trim().startsWith("[")) {
    try {
      const a = JSON.parse(v) as unknown;
      return Array.isArray(a) ? a : [];
    } catch {
      return [];
    }
  }
  return [];
}

/**
 * webcore's answer in the contract's exact shape. Tolerates the small variations a wasm-bindgen export may take
 * (headers as an object or JSON text, events as objects, writes as JSON text, an ArrayBuffer body).
 */
export function normaliseReply(raw: unknown): CoreReply {
  const r = (raw ?? {}) as Record<string, unknown>;
  const status = typeof r.status === "number" ? r.status : 500;
  let headers: [string, string][] = [];
  const h = typeof r.headers === "string" ? (JSON.parse(r.headers) as unknown) : r.headers;
  if (Array.isArray(h)) headers = h.filter((p): p is [string, string] => Array.isArray(p) && p.length === 2).map(([k, v]) => [String(k), String(v)]);
  else if (h && typeof h === "object") headers = Object.entries(h as Record<string, unknown>).map(([k, v]) => [k, String(v)]);
  const events = listOf(r.events).map((e) => (typeof e === "string" ? e : JSON.stringify(e)));
  const writes: CoreWrite[] = listOf(r.writes).flatMap((w) => {
    const o = w as Record<string, unknown>;
    if (typeof o?.store !== "string" || typeof o.key !== "string") return [];
    const v = o.value;
    const value = v === null || v === undefined ? null : typeof v === "string" || v instanceof Uint8Array ? v : toBytes(v);
    return [{ store: o.store, key: o.key, value }];
  });
  return { status, headers, body: toBytes(r.body), events, writes };
}
