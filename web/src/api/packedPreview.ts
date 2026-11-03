// A preview's segments in flat typed arrays (crates/webcore/src/preview.rs `Packed`), as the browser's preview worker
// (src/wasm/) and GET /api/preview.bin (previewBinary.ts) give them: `Preview.packed`, with `Preview.segments` left
// empty, and image passes as rasters (`Preview.rasters`, below) drawn as textures instead of one segment per run.
// 371k segments are 12 MB of arrays here, against 58 MB of JSON objects from GET /api/preview. The readers below
// serve both forms, so every consumer (toolpath layer, timeline, stats card) takes either.

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

/** Length of segment `i`, mm; 0 for a segment inside a raster (it has no geometry here: the raster draws it). */
export function segmentLength(pk: PackedSegments, i: number): number {
  const o = 4 * i;
  const len = Math.hypot(pk.xy[o + 2] - pk.xy[o], pk.xy[o + 3] - pk.xy[o + 1]);
  return Number.isFinite(len) ? len : 0;
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

/** The arrays of a full worker job: the packed segments, and the rasters' when the preview has any. */
export type WorkerArrays = Omit<PackedSegments, "layerIds"> & Partial<RasterArrays>;

/**
 * Turn a worker answer (`meta` JSON plus the packed arrays of a full job) into a Preview whose `packed` holds the
 * segments and `rasters` the image rasters. Without arrays (a stats job) only the summary is filled.
 */
export function previewFromWorker(json: string, arrays?: WorkerArrays): WorkerPreview {
  let meta: Record<string, unknown>;
  try {
    meta = JSON.parse(json) as Record<string, unknown>;
  } catch {
    return { preview: null, error: "the preview worker answered nonsense", summary: null };
  }
  if (typeof meta.error === "string") return { preview: null, error: meta.error, summary: null };
  const { layer_ids, segment_count, rasters: headers, ...rest } = meta;
  const summary: PreviewSummary = { ...(rest as PreviewSummary), segment_count: typeof segment_count === "number" ? segment_count : undefined };
  if (!arrays) return { preview: null, error: null, summary };
  const layerIds = Array.isArray(layer_ids) ? layer_ids.map(String) : [];
  const { levels, rowSegments, rowSpan, ...segs } = arrays;
  let rasters: PreviewRaster[];
  try {
    rasters = rastersOf(headers, { levels: levels ?? new Uint8Array(0), rowSegments: rowSegments ?? new Uint32Array(0), rowSpan: rowSpan ?? new Float32Array(0) });
  } catch (e) {
    return { preview: null, error: e instanceof Error ? e.message : String(e), summary };
  }
  const preview: Preview = { ...(rest as Omit<Preview, "segments">), segments: [], packed: { ...segs, layerIds }, rasters };
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

// --- image rasters ------------------------------------------------------------------------------------------------

/**
 * One image's scan runs in one pass as a raster (crates/plan `PreviewRaster`): segments `first..end` of the preview
 * (the runs and the jumps between them) drawn as the dots they mark. The packed segments in that range have no
 * geometry (xy NaN); their times, kinds and passes stay, so the slider and Start Here count them as the engine does.
 */
export interface PreviewRaster {
  /** Index into `Preview.passes`. */
  pass: number;
  first: number;
  end: number;
  /** Dot (column c, row r) covers c..c+1 x r..r+1 under [a, b, c, d, e, f]: x = a·c + c·r + e, y = b·c + d·r + f (mm). */
  grid: [number, number, number, number, number, number];
  cols: number;
  rows: number;
  /** Per dot, rows from row 0 (burned first): 0 unmarked, k marked at `powers[k - 1]`. */
  levels: Uint8Array;
  /** Power of each level, %. */
  powers: number[];
  /** The pass's power, %. */
  passPowerPct: number;
  /** Per row, two each: its first and one-past-the-last segment (never decreasing; empty rows an empty range). */
  rowSegments: Uint32Array;
  /** Per row, two each: the column its first run starts at and its last run ends at, in burn direction. */
  rowSpan: Float32Array;
}

/** The three shared arrays the rasters of one preview live in (worker answer, GET /api/preview.bin). */
export interface RasterArrays {
  levels: Uint8Array;
  rowSegments: Uint32Array;
  rowSpan: Float32Array;
}

/** A raster header as the worker's meta JSON and the binary preview's header give it (crates/plan `RasterHeader`). */
interface RasterHeader {
  pass: number;
  first: number;
  end: number;
  grid: number[];
  cols: number;
  rows: number;
  powers: number[];
  pass_power_pct: number;
  levels_at: number;
  rows_at: number;
}

/** The rasters of `headers` over `arrays` (views, no copies). Throws on a header the arrays do not hold. */
export function rastersOf(headers: unknown, arrays: RasterArrays): PreviewRaster[] {
  if (!Array.isArray(headers)) return [];
  return (headers as RasterHeader[]).map((h) => {
    const dots = h.cols * h.rows;
    if (!(Array.isArray(h.grid) && h.grid.length === 6) || h.levels_at + dots > arrays.levels.length || 2 * (h.rows_at + h.rows) > arrays.rowSegments.length || 2 * (h.rows_at + h.rows) > arrays.rowSpan.length)
      throw new Error("a preview raster does not fit its arrays");
    return {
      pass: h.pass,
      first: h.first,
      end: h.end,
      grid: h.grid as PreviewRaster["grid"],
      cols: h.cols,
      rows: h.rows,
      levels: arrays.levels.subarray(h.levels_at, h.levels_at + dots),
      powers: h.powers,
      passPowerPct: h.pass_power_pct,
      rowSegments: arrays.rowSegments.subarray(2 * h.rows_at, 2 * (h.rows_at + h.rows)),
      rowSpan: arrays.rowSpan.subarray(2 * h.rows_at, 2 * (h.rows_at + h.rows)),
    };
  });
}

/**
 * How much of raster `r` is shown with the first `n` segments of the job shown: `rows` whole rows, then, when the
 * slider stops inside row `rows`, that row from column `from` to `to` (burn direction, so `to` may be below `from`).
 * `rows` is `r.rows` (and no partial row) when the whole raster is shown, 0 with nothing.
 */
export function rasterReveal(r: PreviewRaster, n: number): { rows: number; partial: { from: number; to: number } | null } {
  if (n >= r.end) return { rows: r.rows, partial: null };
  if (n <= r.first) return { rows: 0, partial: null };
  // The first row not yet finished.
  let lo = 0;
  let hi = r.rows;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (r.rowSegments[2 * mid + 1] <= n) lo = mid + 1;
    else hi = mid;
  }
  if (lo >= r.rows) return { rows: r.rows, partial: null };
  const a = r.rowSegments[2 * lo];
  const b = r.rowSegments[2 * lo + 1];
  if (n <= a || b <= a) return { rows: lo, partial: null };
  // Part-way along the row: its runs spread over the segments between them.
  const from = r.rowSpan[2 * lo];
  const to = r.rowSpan[2 * lo + 1];
  return { rows: lo, partial: { from, to: from + ((to - from) * (n - a)) / (b - a) } };
}

/** Field mm of raster grid point (c, r). */
export function rasterPoint(r: PreviewRaster, c: number, row: number): Vec2 {
  const [a, b, cc, d, e, f] = r.grid;
  return [a * c + cc * row + e, b * c + d * row + f];
}

/** Where the laser is after the first `n` segments when the last of them is one of raster `r`'s (the time slider's head). */
export function rasterHead(r: PreviewRaster, n: number): Vec2 | null {
  if (n <= r.first || n > r.end) return null;
  const v = rasterReveal(r, n);
  if (v.partial) return rasterPoint(r, v.partial.to, v.rows + 0.5);
  const row = Math.max(0, v.rows - 1);
  return rasterPoint(r, r.rowSpan[2 * row + 1], row + 0.5);
}

/** 0..1 shade of each level: its power against the pass's (at least 0.15, so the faintest grey still shows). */
export function levelShades(r: PreviewRaster): Uint8Array {
  const lut = new Uint8Array(256);
  const ref = r.passPowerPct > 0 ? r.passPowerPct : Math.max(1e-9, ...r.powers);
  r.powers.forEach((p, k) => {
    if (k < 255) lut[k + 1] = Math.round(255 * (0.15 + 0.85 * Math.min(1, Math.max(0, p / ref))));
  });
  return lut;
}
