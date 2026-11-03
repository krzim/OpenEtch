// Pure maths behind the Viewport's layered canvas (components/SceneCanvas.tsx): the view transform, the visible field
// region, grid spacing, image smoothing, one-view-update-per-frame coalescing and the toolpath preview packed into
// typed arrays bucketed by tile, so a frame strokes only the tiles it can see. No DOM here (tested in Node).

import { energyOpacity, layerEnergy } from "./api/laser.ts";
import { NO_PASS, type PreviewRaster, isMark, layerOf, passSubLayers, rasterHead, rasterReveal, segmentCount } from "./api/packedPreview.ts";
import { markHidden, passColor, passEnergy, passResolver } from "./api/passes.ts";
import type { Affine, GDocument, Layer, Preview, Vec2 } from "./api/types.ts";
import type { Box } from "./geometry.ts";
import { canvasColor, type Theme } from "./workspace/theme.ts";

/** View transform: field point (cx, cy) at the canvas centre, `scale` screen px per mm. */
export interface View {
  cx: number;
  cy: number;
  scale: number;
}

/**
 * Field mm (+Y up) to device pixels of a canvas `size` CSS px large at `dpr` device px per CSS px. With dpr 1 this is
 * the matrix of the SVG view group.
 */
export function viewMatrix(v: View, size: Vec2, dpr = 1): Affine {
  const s = v.scale * dpr;
  return [s, 0, 0, -s, (size[0] / 2 - v.scale * v.cx) * dpr, (size[1] / 2 + v.scale * v.cy) * dpr];
}

/** The field region on screen, widened by `marginPx` screen px on every side. */
export function visibleBox(v: View, size: Vec2, marginPx = 0): Box {
  const hw = (size[0] / 2 + marginPx) / v.scale;
  const hh = (size[1] / 2 + marginPx) / v.scale;
  return { x0: v.cx - hw, y0: v.cy - hh, x1: v.cx + hw, y1: v.cy + hh };
}

export function boxesTouch(a: Box, b: Box): boolean {
  return a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1;
}

/**
 * Grid line spacing (mm) for a zoom: a fixed spacing from Preferences while it is at least 4 px on screen and the
 * line count stays sane, else the first of 0.5, 1, 2, 5 ... 50 mm that is at least 10 px. Only changes at those
 * steps, so the grid path is rebuilt a handful of times over the whole zoom range, not on every frame.
 */
export function gridSpacing(scale: number, prefMm: number, halfMm: number): { minor: number; major: number } {
  const fixed = prefMm > 0 && prefMm * scale >= 4 && halfMm / prefMm <= 2000 ? prefMm : 0;
  const minor = fixed || ([0.5, 1, 2, 5, 10, 20, 50].find((st) => st * scale >= 10) ?? 50);
  const major = fixed ? fixed * 10 : minor * (minor === 2 || minor === 20 ? 5 : minor === 0.5 || minor === 5 || minor === 50 ? 2 : 10);
  return { minor, major };
}

/** Device px one image pixel covers under transform `m` (field mm to device px) for an image `widthMm` wide. */
export function devicePxPerImagePx(m: Affine, widthMm: number, imagePx: number): number {
  const sx = Math.hypot(m[0], m[1]);
  const sy = Math.hypot(m[2], m[3]);
  return (Math.sqrt(sx * sy) * widthMm) / Math.max(1, imagePx);
}

/**
 * Image smoothing for a bitmap shown at `pxPerImagePx` device px per image pixel: off once it is magnified (crisp
 * pixels, as LightBurn shows a zoomed bitmap), on with mipmapped filtering while it is shrunk.
 */
export function imageSmoothing(pxPerImagePx: number): { enabled: boolean; quality: ImageSmoothingQuality } {
  return pxPerImagePx > 1.001 ? { enabled: false, quality: "low" } : { enabled: true, quality: "high" };
}

/**
 * Coalesces updates to one per animation frame: `push` keeps the newest value and `flush` gets it once, on the next
 * frame. `raf` is requestAnimationFrame (injected for tests).
 */
export function frameCoalescer<T>(raf: (cb: () => void) => unknown, flush: (v: T) => void): { push: (v: T) => void; pending: () => boolean } {
  let pending = false;
  let value: T;
  return {
    push(v: T) {
      value = v;
      if (pending) return;
      pending = true;
      raf(() => {
        pending = false;
        flush(value);
      });
    },
    pending: () => pending,
  };
}

// --- toolpath preview -------------------------------------------------------------------------------------------

/** Segments of one kind in one style (a layer's pass, or the jumps), bucketed by tile. */
export interface ToolpathBatch {
  key: string;
  kind: "mark" | "jump";
  layer: string;
  pass: number | null;
  /** x0, y0, x1, y1 per segment, field mm, sorted by tile. */
  coords: Float32Array;
  /** Segments of tile t are `start[t]` .. `start[t + 1]`. */
  start: Uint32Array;
  /** Bounds of each tile's segments (x0, y0, x1, y1); empty tiles hold +inf/-inf. */
  tileBox: Float32Array;
}

/** An image raster of the preview as the toolpath layer draws it (a texture): its style and how much of it shows. */
export interface ToolpathRaster {
  /** The style key of its pass's marks (`ToolpathBatch.key`). */
  key: string;
  layer: string;
  /** Sub-layer, as a mark batch's `pass`. */
  pass: number | null;
  raster: PreviewRaster;
  /** Whole rows shown (from row 0), then the partial row `rows` from column `from` to `to` (api/packedPreview.ts rasterReveal). */
  rows: number;
  partial: { from: number; to: number } | null;
}

export interface Toolpath {
  batches: ToolpathBatch[];
  /** Image passes, drawn as textures over the travel and under the vector marks. */
  rasters: ToolpathRaster[];
  /** Tile grid over the bounds of every shown segment. */
  grid: { x0: number; y0: number; tw: number; th: number; cols: number; rows: number };
  /** Where the draw-order animation stopped, or null when it shows everything. */
  head: Vec2 | null;
  /** Highest layer/pass energy in the document (for shading). */
  maxE: number;
  /** Segments shown. */
  segments: number;
}

export interface ToolpathOptions {
  hidden: ReadonlySet<string>;
  hiddenPasses: ReadonlySet<string>;
  upTo: number | null;
  jumps: boolean;
}

/** About this many segments per tile (a frame strokes whole tiles). */
const TILE_TARGET = 1500;
const MAX_TILES_PER_SIDE = 64;

/**
 * Packs the preview's segments by style (layer and pass for marks; one batch of jumps) and tile. The segments of the
 * preview's image rasters are skipped as a whole (the rasters draw them), so a large image costs nothing here.
 */
export function buildToolpath(preview: Preview, doc: GDocument | null, o: ToolpathOptions): Toolpath {
  const total = segmentCount(preview);
  const n = o.upTo === null ? total : Math.min(total, Math.max(0, o.upTo));
  const groups = new Map<string, { kind: "mark" | "jump"; layer: string; pass: number | null; xy: number[] }>();
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  let shown = 0;
  let head: Vec2 | null = null;
  const add = (mark: boolean, layer: string, pass: number | null, ax: number, ay: number, bx: number, by: number) => {
    let key: string;
    if (mark) {
      if (markHidden(o, layer, pass)) return;
      key = `${layer}#${pass ?? ""}`;
    } else {
      if (!o.jumps) return;
      key = "jump";
      layer = "";
      pass = null;
    }
    let g = groups.get(key);
    if (!g) {
      g = { kind: mark ? "mark" : "jump", layer, pass, xy: [] };
      groups.set(key, g);
    }
    g.xy.push(ax, ay, bx, by);
    x0 = Math.min(x0, ax, bx);
    y0 = Math.min(y0, ay, by);
    x1 = Math.max(x1, ax, bx);
    y1 = Math.max(y1, ay, by);
    shown++;
  };
  const rasters: ToolpathRaster[] = [];
  const pk = preview.packed;
  if (pk) {
    // The browser's preview: read the arrays directly; a segment's pass is `passes[pass].sub_layer`.
    const subs = passSubLayers(preview);
    const xy = pk.xy;
    const rs = preview.rasters ?? [];
    let ri = 0;
    for (let i = 0; i < n; i++) {
      while (ri < rs.length && rs[ri].end <= i) ri++;
      const r = rs[ri];
      if (r && r.first <= i) {
        // An image pass: the raster, up to the slider, then on after its last run.
        const layer = preview.passes?.[r.pass]?.layer_id ?? "";
        const pass = subs[r.pass] ?? null;
        if (!markHidden(o, layer, pass)) {
          rasters.push({ key: `${layer}#${pass ?? ""}`, layer, pass, raster: r, ...rasterReveal(r, n) });
          shown += Math.min(n, r.end) - i;
        }
        i = Math.min(n, r.end) - 1;
        continue;
      }
      const j = 4 * i;
      const mark = isMark(pk, i);
      let pass: number | null = null;
      if (mark) {
        const p = pk.pass[i];
        pass = p === NO_PASS ? null : (subs[p] ?? null);
      }
      add(mark, mark ? (layerOf(pk, i) ?? "") : "", pass, xy[j], xy[j + 1], xy[j + 2], xy[j + 3]);
    }
    if (n > 0) {
      const last = rs.find((r) => r.first < n && n <= r.end);
      head = last ? rasterHead(last, n) : [xy[4 * n - 2], xy[4 * n - 1]];
    }
  } else {
    const segs = preview.segments ?? [];
    const resolve = passResolver(preview, doc);
    for (let i = 0; i < n; i++) {
      const s = segs[i];
      head = s.to;
      const mark = s.kind === "mark";
      add(mark, mark ? (s.layer ?? "") : "", mark ? resolve(s) : null, s.from[0], s.from[1], s.to[0], s.to[1]);
    }
  }
  const side = Math.max(1, Math.min(MAX_TILES_PER_SIDE, Math.ceil(Math.sqrt(shown / TILE_TARGET))));
  const grid = shown
    ? { x0, y0, tw: Math.max((x1 - x0) / side, 1e-9), th: Math.max((y1 - y0) / side, 1e-9), cols: side, rows: side }
    : { x0: 0, y0: 0, tw: 1, th: 1, cols: 1, rows: 1 };
  const batches: ToolpathBatch[] = [];
  // Jumps first: marks are drawn over them.
  const ordered = [...groups.entries()].sort((a, b) => (a[1].kind === b[1].kind ? 0 : a[1].kind === "jump" ? -1 : 1));
  for (const [key, g] of ordered) batches.push({ key, kind: g.kind, layer: g.layer, pass: g.pass, ...tileSort(g.xy, grid) });
  const maxE = Math.max(0, ...(doc?.layers ?? []).flatMap((l) => [l.settings, ...(l.sub_layers ?? [])].map(layerEnergy)));
  return { batches, rasters, grid, head: o.upTo !== null && n < total ? head : null, maxE, segments: shown };
}

/** Counting sort of segments (x0, y0, x1, y1 each) by the tile of their midpoint. */
function tileSort(xy: number[], g: Toolpath["grid"]): { coords: Float32Array; start: Uint32Array; tileBox: Float32Array } {
  const count = xy.length / 4;
  const tiles = g.cols * g.rows;
  const tileOf = new Uint32Array(count);
  const start = new Uint32Array(tiles + 1);
  for (let i = 0; i < count; i++) {
    const mx = (xy[4 * i] + xy[4 * i + 2]) / 2;
    const my = (xy[4 * i + 1] + xy[4 * i + 3]) / 2;
    const c = Math.min(g.cols - 1, Math.max(0, Math.floor((mx - g.x0) / g.tw)));
    const r = Math.min(g.rows - 1, Math.max(0, Math.floor((my - g.y0) / g.th)));
    const t = r * g.cols + c;
    tileOf[i] = t;
    start[t + 1]++;
  }
  for (let t = 0; t < tiles; t++) start[t + 1] += start[t];
  const next = start.slice(0, tiles);
  const coords = new Float32Array(xy.length);
  const tileBox = new Float32Array(tiles * 4);
  for (let t = 0; t < tiles; t++) tileBox.set([Infinity, Infinity, -Infinity, -Infinity], 4 * t);
  for (let i = 0; i < count; i++) {
    const t = tileOf[i];
    const j = next[t]++;
    const ax = xy[4 * i];
    const ay = xy[4 * i + 1];
    const bx = xy[4 * i + 2];
    const by = xy[4 * i + 3];
    coords[4 * j] = ax;
    coords[4 * j + 1] = ay;
    coords[4 * j + 2] = bx;
    coords[4 * j + 3] = by;
    tileBox[4 * t] = Math.min(tileBox[4 * t], ax, bx);
    tileBox[4 * t + 1] = Math.min(tileBox[4 * t + 1], ay, by);
    tileBox[4 * t + 2] = Math.max(tileBox[4 * t + 2], ax, bx);
    tileBox[4 * t + 3] = Math.max(tileBox[4 * t + 3], ay, by);
  }
  return { coords, start, tileBox };
}

/** Tiles of `b` holding segments that reach into `view` (field mm). */
export function visibleTiles(b: ToolpathBatch, view: Box): number[] {
  const out: number[] = [];
  const tiles = b.start.length - 1;
  for (let t = 0; t < tiles; t++) {
    if (b.start[t + 1] === b.start[t]) continue;
    const k = 4 * t;
    if (b.tileBox[k] <= view.x1 && view.x0 <= b.tileBox[k + 2] && b.tileBox[k + 1] <= view.y1 && view.y0 <= b.tileBox[k + 3]) out.push(t);
  }
  return out;
}

/** Tiles of `b` that hold segments. */
export function filledTiles(b: ToolpathBatch): number {
  let n = 0;
  for (let t = 0; t + 1 < b.start.length; t++) if (b.start[t + 1] > b.start[t]) n++;
  return n;
}

// --- toolpath look ----------------------------------------------------------------------------------------------

export interface ToolpathStyle {
  layers: ReadonlyMap<string, Layer>;
  theme: Theme;
  /** Shade marks by energy (darker = more) instead of the pass colours. */
  shade: boolean;
  /** Travel moves and the energy shade (workspace/theme.ts TOOLPATH_INK). */
  jumpColor: string;
  shadeColor: string;
}

/** How a batch is drawn: colour and opacity, width and dashes in screen px, square caps. */
export interface BatchLook {
  color: string;
  alpha: number;
  widthPx: number;
  dash: [number, number] | null;
  caps: boolean;
}

/** Dashed grey 0.8 px travel; 1.2 px marks in their pass colour (or the shade colour at the pass's energy). */
export function batchLook(b: Pick<ToolpathBatch, "kind" | "layer" | "pass">, maxE: number, st: ToolpathStyle): BatchLook {
  if (b.kind === "jump") return { color: st.jumpColor, alpha: 1, widthPx: 0.8, dash: [4, 3], caps: false };
  const l = st.layers.get(b.layer);
  if (st.shade) return { color: st.shadeColor, alpha: energyOpacity(passEnergy(l, b.pass), maxE), widthPx: 1.2, dash: null, caps: true };
  return { color: canvasColor(l ? passColor(l.color, b.pass ?? 0) : "#dd2222", st.theme), alpha: 1, widthPx: 1.2, dash: null, caps: true };
}

/** #rgb / #rrggbb (and #rgba / #rrggbbaa) as 0..1 RGBA, or null for anything else. */
export function parseHexColor(c: string): [number, number, number, number] | null {
  const m = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(c.trim());
  if (!m) return null;
  let h = m[1];
  if (h.length <= 4) h = [...h].map((x) => x + x).join("");
  const v = (i: number) => parseInt(h.slice(i, i + 2), 16) / 255;
  return [v(0), v(2), v(4), h.length === 8 ? v(6) : 1];
}
