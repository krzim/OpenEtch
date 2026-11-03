// The contracts of the design-only build's backend (docs/design-only/plan.md, WP4): what the worker shell expects
// from crates/webcore's wasm (`WebcoreBackend`), and the messages between the page's transport (rpc.ts) and the
// backend worker (worker.ts). Types only: nothing here runs.
//
// The page never talks to webcore directly. Every `/api/...` request goes to ONE dedicated worker, which answers
// the large stores (projects, backups, templates, fonts) itself from IndexedDB (routes.ts) and hands everything else
// to webcore's `handle()`, applying the store writes webcore returns and posting its events to the page.

// ---------------------------------------------------------------------------------------------------------------
// webcore (crates/webcore, wasm-bindgen): the interface the shell is written against. The mock (mockCore.ts) and
// the real wasm (core.ts adapts it) both implement it, so integration is a swap.
// ---------------------------------------------------------------------------------------------------------------

/** One write to a store webcore keeps (prefs, devices, libraries, presets, ...): `value` null deletes the key. */
export interface CoreWrite {
  store: string;
  key: string;
  /** JSON text or bytes, exactly as webcore wants it back at boot (`PUT /api/_shell/store`). */
  value: string | Uint8Array | null;
}

/** What `handle()` returns for one request. */
export interface CoreReply {
  status: number;
  /** Response headers, e.g. `[["content-type", "application/json"], ["content-disposition", "attachment; ..."]]`. */
  headers: [string, string][];
  body: Uint8Array;
  /** Engine events (`crates/engine/src/events.rs`, `{"type": ...}`) as JSON text, in order, for the page's socket. */
  events: string[];
  /** Store writes the request made, in order; the shell persists them before it answers the page. */
  writes: CoreWrite[];
}

/**
 * crates/webcore's backend: the engine's HTTP API without the network, synchronous. `query` is the raw query string
 * without `?`. A Rust panic surfaces as a thrown exception (`RuntimeError: unreachable` with `panic = "abort"`), after
 * which the instance is poisoned: the shell then restarts the worker (rpc.ts).
 *
 * Routes the shell itself calls (never reachable from the page: rpc.ts refuses `/api/_shell/...`):
 *
 * | Method | Path | Body → answer |
 * |---|---|---|
 * | PUT | `/api/_shell/store?store=&key=` | the value's bytes (JSON text UTF-8 encoded) → 204; loads one record of a store webcore keeps (boot, or another tab changed it) |
 * | DELETE | `/api/_shell/store?store=&key=` | → 204 |
 * | POST | `/api/_shell/boot` | `{now_unix_ms, tz_offset_min}` → 200 `{ok}` after the stores are loaded (clock for `{date}` text) |
 * | POST | `/api/_shell/font?name=<file name>` | TTF/OTF bytes → `{ok, family}` (`text::registry::add_font_data`), 400 when not a font |
 * | POST | `/api/_shell/open_project?name=&new_document=0\|1` | `.openetch` bytes → like `POST /api/projects/open` (`{ok, name, result, revision}`; the document then belongs to project `name`, saved at that revision); `new_document=1` → like `POST /api/documents {project}` (an open document holding `name` is activated instead) |
 * | POST | `/api/_shell/recover?project=` or `?display=` | `.openetch` bytes → `{ok, result, revision}`: the tab's recovery copy back in the active document, with unsaved changes: it belongs to `project` (no saved revision) or, without one, is named `display` |
 * | POST | `/api/_shell/project_saved` | `{name, revision, document_id}` → `{ok}`; marks that document saved as project `name` at `revision` and emits `project_saved` |
 * | POST | `/api/_shell/thumbnail?width=32..1024` | `.openetch` bytes → `image/png` (render::preview_svg + svg_to_png) |
 * | POST | `/api/_shell/check_template` | a LightBurn `.lbt` → `{ok}` or 400 (fileio::lbrn_legacy::to_lbrn2 + lbrn::parse) |
 *
 * Public routes the shell relies on: `GET /api/project`, `GET /api/documents`,
 * `GET /api/document/download?format=openetch` (the bytes it stores), and
 * `POST /api/document/upload?format=openetch|lbrn2&mode=replace&name=` (restoring an autosave, opening a template).
 * Every route the shell answers itself (routes.ts `SHELL_PATHS`) is never sent to webcore.
 */
export interface WebcoreBackend {
  /** `since`: the page's `X-OpenEtch-Since` (a JSON answer to a POST then carries the document's changes, `sync`). */
  handle(method: string, path: string, query: string, body: Uint8Array, since?: string): CoreReply;
  /**
   * The display preview of the active document (`full`: packed segments and rasters; `stats`: the summary only;
   * `fill`: a Fill Region hover), so the design build needs no second worker or wasm (web/src/wasm/previewClient.ts
   * sends its jobs here). Absent in the mock.
   */
  preview?(kind: PreviewKind, params?: unknown): PreviewResult;
  /** Free the wasm memory (wasm-bindgen's `free`). */
  free?(): void;
}

/** What the preview client asks for (web/src/wasm/protocol.ts `JobKind`). */
export type PreviewKind = "full" | "stats" | "fill";

/** A preview job's answer: the revision it is of, its JSON, and (full) the packed arrays. */
export interface PreviewResult {
  rev: number;
  json: string;
  arrays?: {
    xy: Float32Array;
    t: Float32Array;
    info: Uint32Array;
    pass: Uint32Array;
    power: Float32Array;
    levels: Uint8Array;
    rowSegments: Uint32Array;
    rowSpan: Float32Array;
  };
}

// ---------------------------------------------------------------------------------------------------------------
// Page ⇄ worker messages (rpc.ts ⇄ worker.ts, host.ts). Reply bodies are transferred; request bodies are copied (a
// restarted worker gets the request again).
// ---------------------------------------------------------------------------------------------------------------

/** How the worker starts: `fresh` (a new tab), `reload` (this tab reloaded) or `restart` (after a crash). */
export type BootKind = "fresh" | "reload" | "restart";

export type ToWorker =
  | { kind: "init"; sessionId: string; boot: BootKind; tzOffsetMin: number }
  | { kind: "request"; id: number; method: string; path: string; query: string; headers: [string, string][]; body: Uint8Array }
  /** The page is being hidden or unloaded: write the recovery copy now if the document changed. */
  | { kind: "flush" }
  /** A preview job on the backend's own document (in order with the requests). */
  | { kind: "preview"; id: number; job: PreviewKind; params?: unknown };

/** What the worker restored on boot. */
export interface Recovered {
  /** `recovery` (this tab's own copy), `project` (the saved project it had open, unchanged) or `autosave`. */
  source: "recovery" | "project" | "autosave";
  /** When the copy was written (Unix ms). */
  savedMs: number;
  /** The project or file name it belongs to, if any. */
  name: string | null;
}

export type FromWorker =
  | { kind: "ready"; recovered: Recovered | null; storage: "indexeddb" | "memory" }
  /** The worker could not start (no wasm, a broken build): the page shows the error, nothing more is sent. */
  | { kind: "failed"; error: string }
  | { kind: "reply"; id: number; status: number; headers: [string, string][]; body: Uint8Array }
  /** One engine event (JSON text), for the page's event stream. */
  | { kind: "event"; data: string }
  /** webcore panicked: the instance is poisoned; the page restarts the worker. */
  | { kind: "crashed"; error: string }
  /** A preview job's answer (`error` when it failed); arrays are transferred. */
  | { kind: "preview"; id: number; result?: PreviewResult; error?: string; ms: number }
  /** A start-up step done (opening the storage, loading the wasm, restoring): for diagnostics only. */
  | { kind: "progress"; step: string };

/** Something the page should tell the person (a toast). `key` is a message key in workspace/locales. */
export interface Notice {
  key: "ds.restartedRecovered" | "ds.restartedNothing" | "ds.reloadRecovered" | "ds.storageMemory" | "ds.failed";
  vars: Record<string, string>;
  level: "info" | "warning" | "error";
}
