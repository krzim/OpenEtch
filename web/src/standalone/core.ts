// Loading webcore for the backend worker: the real wasm (crates/webcore's `Backend`, through the wasm-bindgen glue
// "webcore-pkg", built with `npm run wasm:design`) or, with VITE_STANDALONE_MOCK=1, the mock (mockCore.ts). The rest
// of the shell sees only contract.ts `WebcoreBackend`.
//
// The adapter (`WasmCore`) maps the shell's internal `/api/_shell/...` routes onto the core's own interface
// (crates/webcore/src/backend/shell.rs):
//
// | Shell route | crates/webcore |
// |---|---|
// | `PUT/DELETE /api/_shell/store` before boot | kept, then given to `new Backend({stores, epoch})` at boot |
// | `PUT/DELETE /api/_shell/store` after boot (another tab) | `PUT/DELETE /core/store` |
// | `POST /api/_shell/boot` | `new Backend(init)` (the clock needs nothing: the core reads JS `Date`) |
// | `POST /api/_shell/font?name=` | `add_font(family, bytes)` |
// | `POST /api/_shell/open_project?name=&new_document=` | `POST /core/open?kind=project&name=&new_document=` |
// | `POST /api/_shell/recover?project=` or `?display=` | `POST /core/open?kind=recovery&...` |
// | `POST /api/_shell/project_saved` `{name, revision, document_id}` | `POST /core/saved?name=&revision=&document=` |
// | `POST /api/_shell/thumbnail?width=` | `POST /core/thumbnail.png` (feature `png`), else `/core/thumbnail.svg` |
// | `POST /api/_shell/check_template` | `POST /core/template/check?format=lbt` |

import type { CoreReply, CoreWrite, PreviewKind, PreviewResult, WebcoreBackend } from "./contract.ts";

/** A packed preview as crates/webcore's `PackedPreview` hands it over. */
interface RawPacked {
  meta(): string;
  take_xy(): Float32Array;
  take_t(): Float32Array;
  take_info(): Uint32Array;
  take_pass(): Uint32Array;
  take_power(): Float32Array;
  take_levels(): Uint8Array;
  take_row_segments(): Uint32Array;
  take_row_span(): Float32Array;
  free(): void;
}

/** crates/webcore/src/design.rs `Backend`. */
export interface RawBackend {
  handle(method: string, path: string, query: string, body: Uint8Array, since?: string): unknown;
  notes(): string;
  preview(): RawPacked;
  stats(): string;
  fill_region_preview(params: string): string;
  revision(): number;
  free(): void;
}

/** The wasm-bindgen glue's shape the shell expects (crates/webcore/src/design.rs and fonts.rs). */
export interface BackendPkg {
  default?: (opts?: { module_or_path?: unknown }) => Promise<unknown>;
  Backend?: new (init: string) => RawBackend;
  add_font?: (family: string, bytes: Uint8Array) => string;
}

export async function loadRealCore(): Promise<WebcoreBackend> {
  const pkg = (await import("webcore-pkg")) as unknown as BackendPkg;
  await pkg.default?.();
  if (typeof pkg.Backend !== "function") throw new Error("this build of the in-browser core has no Backend (crates/webcore, feature design)");
  return new WasmCore(pkg);
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
const dec = new TextDecoder();

function plain(status: number, body: unknown): CoreReply {
  return { status, headers: [["content-type", "application/json"]], body: enc.encode(JSON.stringify(body)), events: [], writes: [] };
}

function randomEpoch(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c?.randomUUID) return c.randomUUID().slice(0, 8);
  return Math.random().toString(36).slice(2, 10);
}

/** The real core behind the shell's contract. See the table above. */
export class WasmCore implements WebcoreBackend {
  private readonly pkg: BackendPkg;
  private b: RawBackend | null = null;
  /** Records given before boot, by store and key. */
  private readonly staged = new Map<string, { store: string; key: string; value: string }>();
  /** Problems with stored records the core found at boot (skipped records). */
  notes: string[] = [];

  constructor(pkg: BackendPkg) {
    this.pkg = pkg;
  }

  handle(method: string, path: string, query: string, body: Uint8Array, since?: string): CoreReply {
    if (path.startsWith("/api/_shell/")) return this.shell(method, path.slice("/api/_shell/".length), query, body);
    return normaliseReply(this.backend().handle(method, path, query, body, since || undefined));
  }

  preview(kind: PreviewKind, params?: unknown): PreviewResult {
    const b = this.backend();
    const rev = b.revision();
    if (kind === "stats") return { rev, json: b.stats() };
    if (kind === "fill") return { rev, json: b.fill_region_preview(JSON.stringify(params ?? {})) };
    const p = b.preview();
    try {
      const json = p.meta();
      const arrays = {
        xy: p.take_xy(),
        t: p.take_t(),
        info: p.take_info(),
        pass: p.take_pass(),
        power: p.take_power(),
        levels: p.take_levels(),
        rowSegments: p.take_row_segments(),
        rowSpan: p.take_row_span(),
      };
      return { rev, json, arrays };
    } finally {
      p.free();
    }
  }

  free(): void {
    this.b?.free();
    this.b = null;
  }

  private backend(): RawBackend {
    if (!this.b) this.boot();
    return this.b as RawBackend;
  }

  private boot(): void {
    const Backend = this.pkg.Backend as NonNullable<BackendPkg["Backend"]>;
    this.b?.free();
    this.b = new Backend(JSON.stringify({ stores: [...this.staged.values()], epoch: randomEpoch() }));
    this.staged.clear();
    try {
      const n = JSON.parse(this.b.notes()) as unknown;
      this.notes = Array.isArray(n) ? n.map(String) : [];
    } catch {
      this.notes = [];
    }
    if (this.notes.length) console.warn(`[design] stored records skipped: ${this.notes.join("; ")}`);
  }

  private core(method: string, path: string, query: string, body: Uint8Array = new Uint8Array()): CoreReply {
    return normaliseReply(this.backend().handle(method, path, query, body));
  }

  private shell(method: string, route: string, query: string, body: Uint8Array): CoreReply {
    const q = new URLSearchParams(query);
    switch (`${method} ${route}`) {
      case "PUT store":
      case "DELETE store": {
        const store = q.get("store") ?? "";
        const key = q.get("key") ?? "";
        if (!this.b) {
          const id = JSON.stringify([store, key]);
          if (method === "PUT") this.staged.set(id, { store, key, value: dec.decode(body) });
          else this.staged.delete(id);
          return { status: 204, headers: [], body: new Uint8Array(), events: [], writes: [] };
        }
        return this.core(method, "/core/store", new URLSearchParams({ store, key }).toString(), method === "PUT" ? body : new Uint8Array());
      }
      case "POST boot":
        if (!this.b) this.boot();
        return plain(200, { ok: true, notes: this.notes });
      case "POST font": {
        const name = q.get("name") ?? "";
        const add = this.pkg.add_font;
        if (!add) return plain(404, { ok: false, error: "this build has no added fonts" });
        // TrueType/OpenType fonts are named by their own family; the file name is only a fallback.
        const r = JSON.parse(add(name.replace(/\.[^.]+$/, ""), body)) as { families?: string[]; error?: string };
        if (r.error || !r.families?.length) return plain(400, { ok: false, error: r.error ?? `${name}: no font in the file` });
        return plain(200, { ok: true, family: r.families[0], families: r.families });
      }
      case "POST open_project": {
        const p = new URLSearchParams({ kind: "project", name: q.get("name") ?? "" });
        if (q.get("new_document") === "1") p.set("new_document", "1");
        return this.core("POST", "/core/open", p.toString(), body);
      }
      case "POST recover": {
        const p = new URLSearchParams({ kind: "recovery" });
        const project = q.get("project");
        const display = q.get("display");
        if (project) p.set("project", project);
        else if (display) p.set("display", display);
        return this.core("POST", "/core/open", p.toString(), body);
      }
      case "POST project_saved": {
        let v: { name?: unknown; revision?: unknown; document_id?: unknown };
        try {
          v = JSON.parse(dec.decode(body)) as typeof v;
        } catch {
          return plain(400, { ok: false, error: "project_saved: not JSON" });
        }
        const p = new URLSearchParams({ name: String(v.name ?? ""), revision: String(v.revision ?? "") });
        if (typeof v.document_id === "string") p.set("document", v.document_id);
        return this.core("POST", "/core/saved", p.toString());
      }
      case "POST thumbnail": {
        const png = this.core("POST", "/core/thumbnail.png", query, body);
        return png.status === 404 ? this.core("POST", "/core/thumbnail.svg", query, body) : png;
      }
      case "POST check_template":
        return this.core("POST", "/core/template/check", "format=lbt", body);
      default:
        return plain(404, { ok: false, error: "no such shell route" });
    }
  }
}

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
