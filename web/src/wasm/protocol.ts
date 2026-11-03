// Messages between the page (previewClient.ts) and the preview worker (previewWorker.ts). Buffers travel as
// transferables: the worker gives its typed arrays away instead of copying them.

import type { DocChanges } from "../api/docSync.ts";

/** What the worker is asked to compute. `full`: the packed preview; `stats`: its summary only; `fill`: Fill Region. */
export type JobKind = "full" | "stats" | "fill";

export type ToWorker =
  /** The engine's GET /api/preview/context JSON. */
  | { type: "context"; json: string }
  /** The engine's document (structured clone of GET /api/document) at revision `rev`. */
  | { type: "doc"; rev: number; doc: unknown }
  /**
   * The engine's change sets (docSync.ts) from the revision last sent to revision `rev`, in order: the worker patches
   * its copy instead of the page cloning the whole document (megabytes) to it after every edit.
   */
  | { type: "changes"; rev: number; changes: DocChanges[] }
  | { type: "job"; id: number; kind: JobKind; rev: number; params?: unknown };

export type FromWorker =
  | { type: "ready" }
  | { type: "failed"; error: string }
  | {
      type: "done";
      id: number;
      kind: JobKind;
      rev: number;
      /** The preview without segments (full, stats), or the fill answer, as JSON; `{"error": ...}` when it failed. */
      json: string;
      /** Packed segments (full only). */
      arrays?: { xy: Float32Array; t: Float32Array; info: Uint32Array; pass: Uint32Array; power: Float32Array };
      /** Time in the worker: parsing the document and computing. */
      ms: { doc: number; job: number };
    }
  | { type: "error"; id: number; error: string };
