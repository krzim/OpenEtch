// A preview's segments in flat typed arrays (crates/webcore/src/preview.rs `Packed`), as the browser's preview worker
// (src/wasm/) returns them: `Preview.packed`, with `Preview.segments` left empty. 371k segments are 12 MB of arrays
// here, against 58 MB of JSON objects from GET /api/preview. The readers below serve both forms, so every consumer
// (toolpath layer, timeline, stats card) takes either.

import type { Preview, PreviewSegment, Vec2 } from "./types.ts";

/** `info` bit: a mark (else a jump). */
export const MARK = 1;
/** `info` bit: the mark fires. */
export const EMITS = 2;
/** `info >>> LAYER_SHIFT` is the index in `layerIds` plus one (0: no layer). */
export const LAYER_SHIFT = 2;
/** `pass` of a segment without a pass. */
export const NO_PASS = 0xffffffff;

export interface PackedSegments {
  /** x0, y0, x1, y1 per segment, field mm. */
  xy: Float32Array;
  /** Estimated time from the job's start to the end of each segment, s. */
  t: Float32Array;
  /** MARK | EMITS | (layer index + 1) << LAYER_SHIFT. */
  info: Uint32Array;
  /** Index into `Preview.passes`, or NO_PASS. */
  pass: Uint32Array;
  /** The mark's own power in %, NaN where it is its pass's. */
  power: Float32Array;
  /** The segments' layer ids. */
  layerIds: string[];
}

/** How many segments the preview has, in either form. */
export function segmentCount(p: Preview | null | undefined): number {
  if (!p) return 0;
  return p.packed ? p.packed.t.length : (p.segments?.length ?? 0);
}

export function isMark(pk: PackedSegments, i: number): boolean {
  return (pk.info[i] & MARK) !== 0;
}

export function layerOf(pk: PackedSegments, i: number): string | null {
  const k = pk.info[i] >>> LAYER_SHIFT;
  return k === 0 ? null : (pk.layerIds[k - 1] ?? null);
}

export function passOf(pk: PackedSegments, i: number): number | null {
  const p = pk.pass[i];
  return p === NO_PASS ? null : p;
}

export function segmentLength(pk: PackedSegments, i: number): number {
  const o = 4 * i;
  return Math.hypot(pk.xy[o + 2] - pk.xy[o], pk.xy[o + 3] - pk.xy[o + 1]);
}

/** Segment `i` as the JSON form (for the odd single lookup; loops read the arrays). */
export function segmentAt(pk: PackedSegments, i: number): PreviewSegment {
  const o = 4 * i;
  const s: PreviewSegment = {
    kind: isMark(pk, i) ? "mark" : "jump",
    layer: layerOf(pk, i),
    from: [pk.xy[o], pk.xy[o + 1]] as Vec2,
    to: [pk.xy[o + 2], pk.xy[o + 3]] as Vec2,
    t_s: pk.t[i],
  };
  const pass = passOf(pk, i);
  if (pass !== null) s.pass = pass;
  return s;
}

/** Pack JSON segments (tests, and comparing the two forms). Fields a segment lacks become NaN / no layer / no pass. */
export function packSegments(segs: readonly (PreviewSegment & { emits?: boolean; power_pct?: number })[]): PackedSegments {
  const n = segs.length;
  const pk: PackedSegments = {
    xy: new Float32Array(4 * n),
    t: new Float32Array(n),
    info: new Uint32Array(n),
    pass: new Uint32Array(n),
    power: new Float32Array(n),
    layerIds: [],
  };
  const index = new Map<string, number>();
  segs.forEach((s, i) => {
    pk.xy.set([s.from[0], s.from[1], s.to[0], s.to[1]], 4 * i);
    pk.t[i] = typeof s.t_s === "number" ? s.t_s : NaN;
    let info = (s.kind === "mark" ? MARK : 0) | (s.emits ? EMITS : 0);
    if (s.layer) {
      let k = index.get(s.layer);
      if (k === undefined) {
        k = pk.layerIds.length;
        index.set(s.layer, k);
        pk.layerIds.push(s.layer);
      }
      info |= (k + 1) << LAYER_SHIFT;
    }
    pk.info[i] = info >>> 0;
    pk.pass[i] = typeof s.pass === "number" ? s.pass : NO_PASS;
    pk.power[i] = typeof s.power_pct === "number" ? s.power_pct : NaN;
  });
  return pk;
}

/** The preview's summary without segments: what the Design workspace shows (stats-only worker jobs). */
export interface PreviewSummary {
  stats?: Record<string, unknown>;
  passes?: Preview["passes"];
  layers?: Preview["layers"];
  warnings?: string[];
  segment_count?: number;
}

/** A worker result: the preview (or why there is none, the engine's 422 text) and its summary. */
export interface WorkerPreview {
  preview: Preview | null;
  error: string | null;
  summary: PreviewSummary | null;
}

/**
 * Turn a worker answer (`meta` JSON plus the packed arrays of a full job) into a Preview whose `packed` holds the
 * segments. Without arrays (a stats job) only the summary is filled.
 */
export function previewFromWorker(json: string, arrays?: Omit<PackedSegments, "layerIds">): WorkerPreview {
  let meta: Record<string, unknown>;
  try {
    meta = JSON.parse(json) as Record<string, unknown>;
  } catch {
    return { preview: null, error: "the preview worker answered nonsense", summary: null };
  }
  if (typeof meta.error === "string") return { preview: null, error: meta.error, summary: null };
  const { layer_ids, segment_count, ...rest } = meta;
  const summary: PreviewSummary = { ...(rest as PreviewSummary), segment_count: typeof segment_count === "number" ? segment_count : undefined };
  if (!arrays) return { preview: null, error: null, summary };
  const layerIds = Array.isArray(layer_ids) ? layer_ids.map(String) : [];
  const preview: Preview = { ...(rest as Omit<Preview, "segments">), segments: [], packed: { ...arrays, layerIds } };
  return { preview, error: null, summary };
}

/** Sub-layer of each job pass (`passes[k].sub_layer`), for resolving packed segments without per-segment objects. */
export function passSubLayers(p: Preview): (number | null)[] {
  return (p.passes ?? []).map((q) => (q && typeof q.sub_layer === "number" ? q.sub_layer : null));
}

/** The whole job's estimated time from a summary, s, or null. */
export function estimateOf(s: PreviewSummary | null | undefined): number | null {
  const t = s?.stats?.estimated_time_s;
  return typeof t === "number" && Number.isFinite(t) ? t : null;
}
