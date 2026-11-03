// The 3D relief preview's model (pure; the worker in reliefWorker.ts runs it off the page's thread): a height field of
// what the job removes, built from the preview the Laser workspace already has (api/packedPreview.ts: the packed
// segments and the image rasters, pass by pass), up to a point of the job's time.
//
// What counts as relief: passes of layers whose images are depth maps (3D Sliced, Grayscale) and passes with a Z step
// (LightBurn's Z step per pass), by default; or every marking pass. Each pass removes `depth per pass` where it marks
// (the Z step itself when the layer has one: the operator set it to follow the material): a cell of an image goes as
// deep as the average of its dots, each at its power against the pass's (a grayscale dot at half power removes
// half), a vector mark
// (fill hatch, outline) adds the pass's depth once to every cell its lines cross. Real removal depends on the
// material, the lens and the source: the view shows where and in which order material goes, and how the depth
// builds up relative to the rest, not micrometres. Shown deeper as it goes (heights are depths below the surface).

import { EMITS, MARK, NO_PASS, type PackedSegments, type PreviewRaster } from "../api/packedPreview.ts";

export type PassKind = "slice3d" | "grayscale" | "zstep" | "other";

export interface ReliefPass {
  layerId: string;
  kind: PassKind;
  /** Removed per full-power pass, mm. */
  depthMm: number;
}

/** What the worker needs: the preview's arrays (copied in) and the passes. */
export interface ReliefInput {
  xy: Float32Array;
  info: Uint32Array;
  pass: Uint32Array;
  power: Float32Array;
  rasters: PreviewRaster[];
  passes: ReliefPass[];
}

export interface ReliefOptions {
  /** Draw only relief passes (3D Sliced, grayscale depth maps, Z steps); false: every marking pass. */
  reliefOnly: boolean;
  /** Longest side of the grid, cells. */
  maxSide: number;
}

export interface ReliefField {
  cols: number;
  rows: number;
  /** Field mm of cell (0, 0)'s centre; cells are `cell` mm square, +X right, +Y up. */
  x0: number;
  y0: number;
  cell: number;
  /** Depth below the surface per cell, mm (row-major, rows from y0 up). */
  depth: Float32Array;
  maxDepth: number;
}

const obj = (v: unknown): Record<string, unknown> | null => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null);

/** A layer's settings as the document holds them (main settings, or sub-layer `n - 1`). */
interface LayerLike {
  id: string;
  settings: unknown;
  sub_layers?: unknown[];
}

/** The relief kind of one set of cut settings. */
export function settingsKind(settings: unknown): { kind: PassKind; zStepMm: number } {
  const s = obj(settings) ?? {};
  const z = obj(s.options)?.z_step_mm;
  const zStepMm = typeof z === "number" && Number.isFinite(z) ? Math.abs(z) : 0;
  if (zStepMm > 0) return { kind: "zstep", zStepMm };
  const dither = obj(s.image)?.dither;
  if (dither === "slice3d" || dither === "3dslice") return { kind: "slice3d", zStepMm };
  if (dither === "grayscale") return { kind: "grayscale", zStepMm };
  return { kind: "other", zStepMm };
}

/**
 * The relief kind and depth of every job pass (`previewPasses`, the preview's `passes`), from the document's layers. A
 * sub-layer of a depth-map layer (a 3D Sliced cleanup pass) counts with its layer.
 */
export function reliefPasses(
  previewPasses: readonly { layer_id: string; sub_layer: number }[],
  layers: readonly LayerLike[],
  depthPerPassMm: number,
): ReliefPass[] {
  return previewPasses.map((p) => {
    const layer = layers.find((l) => l.id === p.layer_id);
    if (!layer) return { layerId: p.layer_id, kind: "other", depthMm: depthPerPassMm };
    const own = p.sub_layer > 0 ? layer.sub_layers?.[p.sub_layer - 1] : layer.settings;
    let k = settingsKind(own);
    if (k.kind === "other" && p.sub_layer > 0) {
      const main = settingsKind(layer.settings);
      if (main.kind === "slice3d" || main.kind === "grayscale") k = { kind: main.kind, zStepMm: k.zStepMm };
    }
    return { layerId: p.layer_id, kind: k.kind, depthMm: k.kind === "zstep" ? k.zStepMm : depthPerPassMm };
  });
}

/** Which passes the field draws: the relief ones, or every one when there are none or `reliefOnly` is off. */
export function includedPasses(passes: readonly ReliefPass[], reliefOnly: boolean): { include: boolean[]; fallback: boolean } {
  const relief = passes.map((p) => p.kind !== "other");
  if (!reliefOnly) return { include: passes.map(() => true), fallback: false };
  if (relief.some(Boolean)) return { include: relief, fallback: false };
  return { include: passes.map(() => true), fallback: true };
}

function rasterCorners(r: PreviewRaster): [number, number][] {
  const [a, b, c, d, e, f] = r.grid;
  return [
    [0, 0],
    [r.cols, 0],
    [0, r.rows],
    [r.cols, r.rows],
  ].map(([col, row]) => [a * col + c * row + e, b * col + d * row + f]);
}

/** Rows of `r` whose runs all end before segment `upTo` (null: all). */
export function rasterRowsDone(r: PreviewRaster, upTo: number | null): number {
  if (upTo === null || upTo >= r.end) return r.rows;
  if (upTo <= r.first) return 0;
  let lo = 0;
  let hi = r.rows;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (r.rowSegments[2 * mid + 1] <= upTo) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * The height field of `input` up to segment `upTo` (null: the whole job) for the passes `include` says, or null when
 * nothing of them marks. The grid covers everything those passes mark (whatever `upTo`, so scrubbing keeps the frame),
 * with two flat cells of margin; its cells are at least as large as the coarsest dot.
 */
export function buildField(input: ReliefInput, include: readonly boolean[], upTo: number | null, maxSide: number): ReliefField | null {
  const { xy, info, pass, rasters, passes } = input;
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  let dotSide = 0;
  const grow = (x: number, y: number) => {
    if (x < x0) x0 = x;
    if (x > x1) x1 = x;
    if (y < y0) y0 = y;
    if (y > y1) y1 = y;
  };
  const used = (p: number) => p !== NO_PASS && include[p] === true;
  for (const r of rasters) {
    if (!used(r.pass)) continue;
    for (const [x, y] of rasterCorners(r)) grow(x, y);
    const [a, b, c, d] = r.grid;
    dotSide = Math.max(dotSide, Math.sqrt(Math.abs(a * d - b * c)));
  }
  const n = info.length;
  const marks = (i: number) => (info[i] & (MARK | EMITS)) === (MARK | EMITS) && used(pass[i]);
  for (let i = 0; i < n; i++) {
    if (!marks(i)) continue;
    const o = 4 * i;
    const ax = xy[o];
    if (!Number.isFinite(ax)) continue;
    grow(ax, xy[o + 1]);
    grow(xy[o + 2], xy[o + 3]);
  }
  if (!(x1 >= x0 && y1 >= y0)) return null;
  const extent = Math.max(x1 - x0, y1 - y0, 1e-3);
  const side = Math.max(16, Math.round(maxSide));
  const cell = Math.max(extent / (side - 4), dotSide, 1e-3);
  const cols = Math.ceil((x1 - x0) / cell) + 5;
  const rows = Math.ceil((y1 - y0) / cell) + 5;
  const ox = x0 - 2 * cell;
  const oy = y0 - 2 * cell;
  const depth = new Float32Array(cols * rows);
  const at = (x: number, y: number) => {
    const cx = Math.floor((x - ox) / cell);
    const cy = Math.floor((y - oy) / cell);
    return cx < 0 || cy < 0 || cx >= cols || cy >= rows ? -1 : cy * cols + cx;
  };

  // Image rasters: per pass, a cell goes as deep as the average of its dots (each at its power against the pass's,
  // unmarked dots 0), so a cell that holds one dot or two (a grid coarser than the dots, not aligned to them) never
  // shows the difference as spikes.
  const sum = new Float32Array(cols * rows);
  const count = new Uint32Array(cols * rows);
  const touched: number[] = [];
  for (const r of rasters) {
    if (!used(r.pass)) continue;
    const done = rasterRowsDone(r, upTo);
    if (done === 0) continue;
    const per = passes[r.pass]?.depthMm ?? 0;
    const [a, b, c, d, e, f] = r.grid;
    const ref = r.passPowerPct > 0 ? r.passPowerPct : Math.max(1e-9, ...r.powers);
    const frac = new Float32Array(Math.max(1, r.powers.length + 1));
    r.powers.forEach((pw, k) => {
      frac[k + 1] = Math.min(1, Math.max(0, pw / ref));
    });
    for (let row = 0; row < done; row++) {
      const base = row * r.cols;
      const ry = row + 0.5;
      for (let col = 0; col < r.cols; col++) {
        const cx = col + 0.5;
        const k = at(a * cx + c * ry + e, b * cx + d * ry + f);
        if (k < 0) continue;
        if (count[k] === 0) touched.push(k);
        count[k] += 1;
        const lv = r.levels[base + col];
        if (lv !== 0) sum[k] += frac[lv] ?? 0;
      }
    }
    for (const k of touched) {
      depth[k] += (per * sum[k]) / count[k];
      sum[k] = 0;
      count[k] = 0;
    }
    touched.length = 0;
  }

  // Vector marks: once per pass for every cell a line crosses (hatch lines closer than a cell count once).
  const stamp = new Int32Array(cols * rows);
  const end = upTo === null ? n : Math.min(n, upTo);
  for (let i = 0; i < end; i++) {
    if (!marks(i)) continue;
    const o = 4 * i;
    const ax = xy[o];
    const ay = xy[o + 1];
    if (!Number.isFinite(ax)) continue;
    const bx = xy[o + 2];
    const by = xy[o + 3];
    const p = pass[i];
    const per = passes[p]?.depthMm ?? 0;
    const tag = p + 1;
    const len = Math.hypot(bx - ax, by - ay);
    const steps = Math.max(1, Math.ceil(len / (cell * 0.5)));
    for (let s = 0; s <= steps; s++) {
      const t = s / steps;
      const k = at(ax + (bx - ax) * t, ay + (by - ay) * t);
      if (k >= 0 && stamp[k] !== tag) {
        stamp[k] = tag;
        depth[k] += per;
      }
    }
  }

  let maxDepth = 0;
  for (let k = 0; k < depth.length; k++) if (depth[k] > maxDepth) maxDepth = depth[k];
  return { cols, rows, x0: ox + cell / 2, y0: oy + cell / 2, cell, depth, maxDepth };
}

/** The packed arrays and rasters of a preview, as the worker takes them (JSON previews are packed first). */
export function reliefArrays(packed: PackedSegments, rasters: readonly PreviewRaster[] | undefined): Omit<ReliefInput, "passes"> {
  return { xy: packed.xy, info: packed.info, pass: packed.pass, power: packed.power, rasters: rasters ? [...rasters] : [] };
}

/** Vertex normals of the field drawn with depth × `exaggeration` (unit vectors, 3 per cell, +Z up). */
export function fieldNormals(f: ReliefField, exaggeration: number): Float32Array {
  const { cols, rows, depth, cell } = f;
  const out = new Float32Array(cols * rows * 3);
  const z = (cx: number, cy: number) => -depth[Math.min(rows - 1, Math.max(0, cy)) * cols + Math.min(cols - 1, Math.max(0, cx))] * exaggeration;
  for (let cy = 0; cy < rows; cy++) {
    for (let cx = 0; cx < cols; cx++) {
      const dzdx = (z(cx + 1, cy) - z(cx - 1, cy)) / (2 * cell);
      const dzdy = (z(cx, cy + 1) - z(cx, cy - 1)) / (2 * cell);
      const len = Math.hypot(dzdx, dzdy, 1);
      const o = 3 * (cy * cols + cx);
      out[o] = -dzdx / len || 0;
      out[o + 1] = -dzdy / len || 0;
      out[o + 2] = 1 / len;
    }
  }
  return out;
}

/** A depth's text: "0.12 mm", or "35 µm" below 0.1 mm. */
export function depthText(mm: number): string {
  if (!(mm > 0)) return "0 mm";
  if (mm < 0.1) return `${Math.round(mm * 1000)} µm`;
  return `${Number(mm.toFixed(mm < 1 ? 3 : 2))} mm`;
}

/** A sensible starting exaggeration: the deepest point drawn at about a sixth of the field's width. */
export function autoExaggeration(f: ReliefField): number {
  const width = Math.max(f.cols, f.rows) * f.cell;
  if (!(f.maxDepth > 0)) return 1;
  const k = width / 6 / f.maxDepth;
  // Rounded to a slider step: 1, 2, 5, 10, 20, 50, ...
  const steps = [1, 2, 5, 10, 20, 50, 100, 200, 500];
  return steps.reduce((best, s) => (Math.abs(Math.log(s / k)) < Math.abs(Math.log(best / k)) ? s : best), 1);
}
