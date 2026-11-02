// The document's vectors as raster-layer data (components/VectorLayer.tsx draws them, components/vectorGL.ts on the
// GPU). Built once per document, not per view: every visible object is flattened (curves to a fine tolerance, so they
// stay smooth up to the maximum zoom) into one packed array of segments grouped by layer, each carrying its object,
// the distance along its subpath (for dashes) and its subpath's first point (the fan anchor for even-odd fills). What
// changes without a document change rides on top: per-segment flags (selected, outside the workspace) rebuilt when the
// selection changes, and per-frame uniforms (view, live transform, colours, the marching-ants phase). No DOM here.
//
// The look is the old SVG layer's (Viewport.tsx before the raster move): object outlines 1.3 px in the layer colour
// (canvasColor), 40 % for layers that do not output, the filled view's even-odd fills per layer at 35 % (15 % when not
// output), the selected outline a 1.6 px 5/4 dash in the accent colour marching 9 px per 0.8 s, objects outside the
// workspace a 3 px outline, and everything but those highlights at 30 % while the preview is shown. Widths and dashes
// are screen px whatever the zoom.

import type { Affine, GDocument, Layer } from "./api/types.ts";
import { geometryCmds, geometryKey, mul, type PathCmd } from "./geometry.ts";
import { gridSpacing, viewMatrix, visibleBox, type View } from "./sceneLayers.ts";
import { canvasColor, type Theme } from "./workspace/theme.ts";

/** Floats per segment: ax, ay, bx, by (field mm), along (mm from the subpath start to a), anchor x, y, meta. */
export const SEG_STRIDE = 8;
/** meta = object index * 4 + bits: the implicit closing edge of an open subpath (filled, never stroked). */
export const META_FILL_ONLY = 1;
/** meta bit: never filled (image frames). */
export const META_NO_FILL = 2;
/** Per-segment flag bits (segmentFlags). */
export const FLAG_SELECTED = 1;
export const FLAG_OUTSIDE = 2;

/**
 * Curve flattening tolerance, field mm: 0.002 mm is under a screen pixel at the maximum zoom (400 px/mm) and far under
 * at ordinary zooms, so curves look exact without rebuilding anything when the zoom changes.
 */
export const RENDER_TOL_MM = 0.002;
const MAX_STEPS = 512;

/** Screen px and timings of the canvas look. */
export const VECTOR_LOOK = {
  objWidth: 1.3,
  selWidth: 1.6,
  outsideWidth: 3,
  selDash: [5, 4] as [number, number],
  /** The selected outline's dashes advance one period (9 px) per this many ms. */
  antsMs: 800,
  stockWidth: 1.5,
  stockSelWidth: 2,
  stockDash: [6, 3] as [number, number],
};

/** One layer's run of segments (the scene draws a group per layer, in document layer order). */
export interface SceneGroup {
  /** Layer id, or "" for objects on a layer the document does not list. */
  key: string;
  layer: Layer | null;
  /** Segments first .. first + count. */
  first: number;
  count: number;
}

export interface SceneObject {
  id: string;
  group: number;
  first: number;
  count: number;
}

export interface VectorScene {
  /** SEG_STRIDE floats per segment. */
  segs: Float32Array;
  count: number;
  groups: SceneGroup[];
  objects: SceneObject[];
  objIndex: Map<string, number>;
}

/** A growable float buffer. */
class Floats {
  a: Float32Array;
  n = 0;
  constructor(cap = 1024) {
    this.a = new Float32Array(cap);
  }
  reserve(k: number): void {
    if (this.n + k <= this.a.length) return;
    let cap = this.a.length * 2;
    while (cap < this.n + k) cap *= 2;
    const b = new Float32Array(cap);
    b.set(this.a.subarray(0, this.n));
    this.a = b;
  }
}

/** Uniform steps for a Bezier whose second differences reach `dd` mm, so the chord error stays under `tol`. */
function steps(dd: number, cubic: boolean, tol: number): number {
  // Error <= M / (8 n^2) with M = |B''| max: 6 dd for a cubic, 2 dd for a quadratic.
  const m = (cubic ? 6 : 2) * dd;
  return Math.max(1, Math.min(MAX_STEPS, Math.ceil(Math.sqrt(m / (8 * tol)))));
}

/**
 * Flattens one object's commands under `t` into segments (SEG_STRIDE floats each, meta without the object index).
 * Subpaths restart the dash distance; open subpaths get a fill-only closing edge (SVG fills them closed).
 */
export function flattenSegments(cmds: readonly PathCmd[], t: Affine, tolMm = RENDER_TOL_MM, noFill = false): Float32Array {
  const out = new Floats(64);
  const [m0, m1, m2, m3, m4, m5] = t;
  const X = (x: number, y: number) => m0 * x + m2 * y + m4;
  const Y = (x: number, y: number) => m1 * x + m3 * y + m5;
  const nf = noFill ? META_NO_FILL : 0;
  // Current subpath state (field coordinates).
  let sx = 0;
  let sy = 0;
  let cx = 0;
  let cy = 0;
  let along = 0;
  let open = false;
  const seg = (bx: number, by: number, meta: number) => {
    const dx = bx - cx;
    const dy = by - cy;
    if (dx === 0 && dy === 0) return;
    out.reserve(SEG_STRIDE);
    const a = out.a;
    const k = out.n;
    a[k] = cx;
    a[k + 1] = cy;
    a[k + 2] = bx;
    a[k + 3] = by;
    a[k + 4] = along;
    a[k + 5] = sx;
    a[k + 6] = sy;
    a[k + 7] = meta;
    out.n += SEG_STRIDE;
    if (!(meta & META_FILL_ONLY)) along += Math.hypot(dx, dy);
    cx = bx;
    cy = by;
  };
  const closeOpen = () => {
    if (open && (cx !== sx || cy !== sy)) {
      const keepAlong = along;
      seg(sx, sy, META_FILL_ONLY | nf);
      along = keepAlong;
    }
    open = false;
  };
  for (const k of cmds) {
    switch (k.c) {
      case "M":
        closeOpen();
        sx = cx = X(k.p[0], k.p[1]);
        sy = cy = Y(k.p[0], k.p[1]);
        along = 0;
        open = true;
        break;
      case "L":
        if (!open) {
          // A command after Z starts a new subpath at the last start point.
          open = true;
          along = 0;
        }
        seg(X(k.p[0], k.p[1]), Y(k.p[0], k.p[1]), nf);
        break;
      case "Q": {
        if (!open) {
          open = true;
          along = 0;
        }
        const x0 = cx;
        const y0 = cy;
        const x1 = X(k.p1[0], k.p1[1]);
        const y1 = Y(k.p1[0], k.p1[1]);
        const x2 = X(k.p[0], k.p[1]);
        const y2 = Y(k.p[0], k.p[1]);
        const n = steps(Math.hypot(x0 - 2 * x1 + x2, y0 - 2 * y1 + y2), false, tolMm);
        for (let i = 1; i <= n; i++) {
          const s = i / n;
          const u = 1 - s;
          if (i === n) seg(x2, y2, nf);
          else seg(u * u * x0 + 2 * u * s * x1 + s * s * x2, u * u * y0 + 2 * u * s * y1 + s * s * y2, nf);
        }
        break;
      }
      case "C": {
        if (!open) {
          open = true;
          along = 0;
        }
        const x0 = cx;
        const y0 = cy;
        const x1 = X(k.p1[0], k.p1[1]);
        const y1 = Y(k.p1[0], k.p1[1]);
        const x2 = X(k.p2[0], k.p2[1]);
        const y2 = Y(k.p2[0], k.p2[1]);
        const x3 = X(k.p[0], k.p[1]);
        const y3 = Y(k.p[0], k.p[1]);
        const dd = Math.max(Math.hypot(x0 - 2 * x1 + x2, y0 - 2 * y1 + y2), Math.hypot(x1 - 2 * x2 + x3, y1 - 2 * y2 + y3));
        const n = steps(dd, true, tolMm);
        for (let i = 1; i <= n; i++) {
          if (i === n) {
            seg(x3, y3, nf);
            break;
          }
          const s = i / n;
          const u = 1 - s;
          const w0 = u * u * u;
          const w1 = 3 * u * u * s;
          const w2 = 3 * u * s * s;
          const w3 = s * s * s;
          seg(w0 * x0 + w1 * x1 + w2 * x2 + w3 * x3, w0 * y0 + w1 * y1 + w2 * y2 + w3 * y3, nf);
        }
        break;
      }
      case "Z":
        if (open) seg(sx, sy, nf);
        open = false;
        cx = sx;
        cy = sy;
        along = 0;
        break;
    }
  }
  closeOpen();
  return out.a.slice(0, out.n);
}

/**
 * Flattened segments per object id, kept while the geometry and transform stay the same. The UI refetches the whole
 * document after every edit, so objects arrive as new values each time: they are matched by content (the path data
 * itself for paths and text, the size for images), which costs far less than parsing and flattening them again.
 */
const flatCache = new Map<string, { key: string; t: Affine; segs: Float32Array }>();

function objectSegments(o: GDocument["objects"][number], live: Set<string>): Float32Array {
  live.add(o.id);
  const key = geometryKey(o.geometry);
  const t = o.transform;
  const c = flatCache.get(o.id);
  if (c && c.key === key && c.t.every((v, i) => v === t[i])) return c.segs;
  const segs = flattenSegments(geometryCmds(o.geometry), t, RENDER_TOL_MM, o.geometry.kind === "image");
  flatCache.set(o.id, { key, t: [...t] as Affine, segs });
  return segs;
}

/**
 * The scene of a document: the objects on shown layers (and on layers the document does not list, which stay
 * selectable too), grouped by layer in the document's layer order, objects in document order within a layer.
 */
export function buildVectorScene(doc: GDocument | null): VectorScene {
  const groups: SceneGroup[] = [];
  const objects: SceneObject[] = [];
  const objIndex = new Map<string, number>();
  if (!doc) return { segs: new Float32Array(0), count: 0, groups, objects, objIndex };
  const known = new Map(doc.layers.map((l) => [l.id, l]));
  const byLayer = new Map<string, GDocument["objects"]>();
  for (const o of doc.objects) {
    const key = known.has(o.layer) ? o.layer : "";
    let list = byLayer.get(key);
    if (!list) byLayer.set(key, (list = []));
    list.push(o);
  }
  const order: { key: string; layer: Layer | null }[] = doc.layers.filter((l) => l.visible).map((l) => ({ key: l.id, layer: l }));
  if (byLayer.has("")) order.push({ key: "", layer: null });
  const parts: { o: GDocument["objects"][number]; segs: Float32Array; g: number }[] = [];
  const seen = new Set<string>();
  let total = 0;
  for (const { key, layer } of order) {
    const list = byLayer.get(key);
    if (!list) continue;
    const g = groups.length;
    const first = total / SEG_STRIDE;
    for (const o of list) {
      const segs = objectSegments(o, seen);
      parts.push({ o, segs, g });
      total += segs.length;
    }
    groups.push({ key, layer, first, count: total / SEG_STRIDE - first });
  }
  // Objects no longer shown (deleted, or on a hidden layer) leave the cache.
  for (const id of flatCache.keys()) if (!seen.has(id)) flatCache.delete(id);
  const segs = new Float32Array(total);
  let at = 0;
  for (const { o, segs: s, g } of parts) {
    const idx = objects.length;
    const n = s.length / SEG_STRIDE;
    objects.push({ id: o.id, group: g, first: at / SEG_STRIDE, count: n });
    objIndex.set(o.id, idx);
    segs.set(s, at);
    const base = idx * 4;
    for (let k = at + 7; k < at + s.length; k += SEG_STRIDE) segs[k] += base;
    at += s.length;
  }
  return { segs, count: total / SEG_STRIDE, groups, objects, objIndex };
}

/** Per-segment flags (FLAG_SELECTED, FLAG_OUTSIDE), rebuilt when the selection or the outside set changes. */
export function segmentFlags(scene: VectorScene, selected: ReadonlySet<string>, outside: ReadonlySet<string>): Uint8Array {
  const flags = new Uint8Array(scene.count);
  const mark = (ids: ReadonlySet<string>, bit: number) => {
    for (const id of ids) {
      const i = scene.objIndex.get(id);
      if (i === undefined) continue;
      const o = scene.objects[i];
      for (let k = o.first; k < o.first + o.count; k++) flags[k] |= bit;
    }
  };
  mark(selected, FLAG_SELECTED);
  mark(outside, FLAG_OUTSIDE);
  return flags;
}

/** How one layer group is drawn: CSS colours with separate opacities. */
export interface GroupLook {
  stroke: string;
  strokeAlpha: number;
  /** Fill colour in the filled view (fill and fill+line layers), else null. */
  fill: string | null;
  fillAlpha: number;
  /** A moving object's own fill (drawn with the live transform). */
  movingFillAlpha: number;
}

/** The old SVG layer's look per group (see the file comment). `dimmed`: the toolpath preview is shown. */
export function groupLook(g: Pick<SceneGroup, "layer">, theme: Theme, filled: boolean, dimmed: boolean): GroupLook {
  const l = g.layer;
  const dim = dimmed ? 0.3 : 1;
  const noOut = l !== null && !l.output;
  const stroke = canvasColor(l?.color, theme);
  const fills = filled && l !== null && l.settings.mode !== "line";
  return {
    stroke,
    strokeAlpha: (noOut ? 0.4 : 1) * dim,
    fill: fills && l ? canvasColor(l.color, theme) : null,
    fillAlpha: (l?.output ? 0.35 : 0.15) * dim,
    movingFillAlpha: 0.35 * (noOut ? 0.4 : 1) * dim,
  };
}

/** The marching-ants dash phase (screen px into the 9 px period) at time `ms`, in `stepPx` steps (0: smooth). */
export function antsPhase(ms: number, stepPx = 0.5): number {
  const period = VECTOR_LOOK.selDash[0] + VECTOR_LOOK.selDash[1];
  const p = ((ms % VECTOR_LOOK.antsMs) / VECTOR_LOOK.antsMs) * period;
  return stepPx > 0 ? Math.floor(p / stepPx) * stepPx : p;
}

// --- the back layer: field, grid, axes, stock -----------------------------------------------------------------------

/** Lines of the grid on screen: field values of the minor and major lines inside the workspace and the view. */
export function gridLines(
  minor: number,
  major: number,
  half: number,
  lo: number,
  hi: number,
): { minor: number[]; major: number[] } {
  const out = { minor: [] as number[], major: [] as number[] };
  if (!(minor > 0)) return out;
  const from = Math.max(-Math.floor(half / minor), Math.ceil(lo / minor));
  const to = Math.min(Math.floor(half / minor), Math.floor(hi / minor));
  for (let i = from; i <= to; i++) {
    const v = i * minor;
    if (Math.abs(v / major - Math.round(v / major)) < 1e-9) out.major.push(v);
    else out.minor.push(v);
  }
  return out;
}

/** Device-pixel coordinate of a 1-ish px line, on a pixel centre so it stays crisp. */
export function crisp(v: number): number {
  return Math.floor(v) + 0.5;
}

/** Canvas colours read from the theme's CSS tokens (components/SceneCanvas.tsx useCanvasTokens). */
export interface CanvasTokens {
  field: string;
  fieldEdge: string;
  gridMinor: string;
  gridMajor: string;
  axisX: string;
  axisY: string;
  stockFill: string;
  stockStroke: string;
  accent: string;
  accentFill: string;
  outside: string;
}

/** The CSS custom property behind each token. */
export const TOKEN_VARS: Record<keyof CanvasTokens, string> = {
  field: "--field",
  fieldEdge: "--field-edge",
  gridMinor: "--grid-minor",
  gridMajor: "--grid-major",
  axisX: "--axis-x",
  axisY: "--axis-y",
  stockFill: "--stock-fill",
  stockStroke: "--stock-stroke",
  accent: "--accent",
  accentFill: "--accent-fill",
  outside: "--outside",
};

/** What a 2D context needs for the back layer (a mock records it in the tests). */
export type BackCtx = Pick<
  CanvasRenderingContext2D,
  | "beginPath"
  | "moveTo"
  | "lineTo"
  | "bezierCurveTo"
  | "quadraticCurveTo"
  | "closePath"
  | "stroke"
  | "fill"
  | "fillRect"
  | "strokeRect"
  | "setLineDash"
  | "setTransform"
> & { fillStyle: unknown; strokeStyle: unknown; lineWidth: number; lineDashOffset: number };

export interface BackParams {
  /** Half the workspace, mm. */
  half: [number, number];
  /** Preferences > Grid: shown, and a fixed spacing (0: automatic). */
  grid: { visible: boolean; prefMm: number };
  /** The stock outline (field transform, with a live move applied), or null. */
  stock: { cmds: readonly PathCmd[]; m: Affine; selected: boolean } | null;
  colors: CanvasTokens;
}

/**
 * The field, grid, axes, field edge and stock, painted in device pixels for the view (cheap: only the grid lines on
 * screen, each on a pixel centre). Replaces the old SVG back layer with the same colours and screen widths.
 */
export function paintBack(ctx: BackCtx, a: { view: View; size: [number, number]; dpr: number }, p: BackParams): void {
  const { view, dpr } = a;
  const vm = viewMatrix(view, a.size, dpr);
  const W = a.size[0] * dpr;
  const H = a.size[1] * dpr;
  const X = (x: number) => vm[0] * x + vm[4];
  const Y = (y: number) => vm[3] * y + vm[5];
  const [hx, hy] = p.half;
  const c = p.colors;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.setLineDash([]);
  const fx0 = X(-hx);
  const fx1 = X(hx);
  const fy0 = Y(hy);
  const fy1 = Y(-hy);
  ctx.fillStyle = c.field;
  ctx.fillRect(fx0, fy0, fx1 - fx0, fy1 - fy0);
  // Visible part of the field, device px.
  const vx0 = Math.max(0, fx0);
  const vx1 = Math.min(W, fx1);
  const vy0 = Math.max(0, fy0);
  const vy1 = Math.min(H, fy1);
  if (p.grid.visible && vx1 > vx0 && vy1 > vy0) {
    const { minor, major } = gridSpacing(view.scale, p.grid.prefMm, Math.max(hx, hy));
    const vis = visibleBox(view, a.size, 1);
    const xs = gridLines(minor, major, hx, vis.x0, vis.x1);
    const ys = gridLines(minor, major, hy, vis.y0, vis.y1);
    const lines = (vx: number[], vy: number[], color: string, w: number) => {
      if (!vx.length && !vy.length) return;
      ctx.beginPath();
      for (const v of vx) {
        const x = crisp(X(v));
        ctx.moveTo(x, vy0);
        ctx.lineTo(x, vy1);
      }
      for (const v of vy) {
        const y = crisp(Y(v));
        ctx.moveTo(vx0, y);
        ctx.lineTo(vx1, y);
      }
      ctx.strokeStyle = color;
      ctx.lineWidth = w * dpr;
      ctx.stroke();
    };
    lines(xs.minor, ys.minor, c.gridMinor, 0.6);
    lines(xs.major, ys.major, c.gridMajor, 0.9);
  }
  if (vx1 > vx0 && vy1 > vy0) {
    ctx.lineWidth = 1.2 * dpr;
    ctx.beginPath();
    const y = crisp(Y(0));
    ctx.moveTo(vx0, y);
    ctx.lineTo(vx1, y);
    ctx.strokeStyle = c.axisX;
    ctx.stroke();
    ctx.beginPath();
    const x = crisp(X(0));
    ctx.moveTo(x, vy0);
    ctx.lineTo(x, vy1);
    ctx.strokeStyle = c.axisY;
    ctx.stroke();
  }
  ctx.lineWidth = 1.5 * dpr;
  ctx.strokeStyle = c.fieldEdge;
  ctx.strokeRect(fx0, fy0, fx1 - fx0, fy1 - fy0);
  if (p.stock) {
    const m = mul(vm, p.stock.m);
    ctx.beginPath();
    tracePath(ctx, p.stock.cmds, m);
    ctx.fillStyle = p.stock.selected ? c.accentFill : c.stockFill;
    ctx.fill();
    const lw = (p.stock.selected ? VECTOR_LOOK.stockSelWidth : VECTOR_LOOK.stockWidth) * dpr;
    ctx.lineWidth = lw;
    ctx.strokeStyle = p.stock.selected ? c.accent : c.stockStroke;
    const dash = VECTOR_LOOK.stockDash.map((d) => d * dpr) as [number, number];
    ctx.setLineDash(dash);
    strokeVisibleRuns(ctx, flattenSegments(p.stock.cmds, p.stock.m), vm, W, H, lw + 2, dash);
    ctx.setLineDash([]);
    ctx.lineDashOffset = 0;
  }
}

/**
 * Strokes flattened segments (field mm, under the view matrix `vm`) as the runs that reach the `W` x `H` canvas
 * widened by `margin` device px, each continuing the dash pattern where the whole outline would have it. A dashed
 * outline far larger than the screen (the stock at a high zoom) would otherwise cost the dashes of its whole length
 * every frame: tens of thousands of them, most off screen.
 */
export function strokeVisibleRuns(
  ctx: Pick<BackCtx, "beginPath" | "moveTo" | "lineTo" | "stroke"> & { lineDashOffset: number },
  segs: Float32Array,
  vm: Affine,
  W: number,
  H: number,
  margin: number,
  dash: [number, number],
): number {
  const k = Math.sqrt(Math.abs(vm[0] * vm[3] - vm[1] * vm[2]));
  const period = dash[0] + dash[1];
  const X = (x: number, y: number) => vm[0] * x + vm[2] * y + vm[4];
  const Y = (x: number, y: number) => vm[1] * x + vm[3] * y + vm[5];
  let runs = 0;
  let open = false;
  let lastX = NaN;
  let lastY = NaN;
  const end = () => {
    if (open) ctx.stroke();
    open = false;
  };
  for (let i = 0; i < segs.length; i += SEG_STRIDE) {
    if (segs[i + 7] % 4 & META_FILL_ONLY) continue;
    const ax = X(segs[i], segs[i + 1]);
    const ay = Y(segs[i], segs[i + 1]);
    const bx = X(segs[i + 2], segs[i + 3]);
    const by = Y(segs[i + 2], segs[i + 3]);
    const visible = Math.max(ax, bx) >= -margin && Math.min(ax, bx) <= W + margin && Math.max(ay, by) >= -margin && Math.min(ay, by) <= H + margin;
    if (!visible) {
      end();
      continue;
    }
    if (!open || ax !== lastX || ay !== lastY) {
      end();
      ctx.beginPath();
      ctx.lineDashOffset = period > 0 ? (segs[i + 4] * k) % period : 0;
      ctx.moveTo(ax, ay);
      open = true;
      runs++;
    }
    ctx.lineTo(bx, by);
    lastX = bx;
    lastY = by;
  }
  end();
  return runs;
}

/** Adds commands mapped through `m` to the context's current path (an affine map keeps Beziers exact). */
export function tracePath(ctx: Pick<BackCtx, "moveTo" | "lineTo" | "bezierCurveTo" | "quadraticCurveTo" | "closePath">, cmds: readonly PathCmd[], m: Affine): void {
  const X = (q: [number, number]) => m[0] * q[0] + m[2] * q[1] + m[4];
  const Y = (q: [number, number]) => m[1] * q[0] + m[3] * q[1] + m[5];
  for (const k of cmds) {
    switch (k.c) {
      case "M":
        ctx.moveTo(X(k.p), Y(k.p));
        break;
      case "L":
        ctx.lineTo(X(k.p), Y(k.p));
        break;
      case "Q":
        ctx.quadraticCurveTo(X(k.p1), Y(k.p1), X(k.p), Y(k.p));
        break;
      case "C":
        ctx.bezierCurveTo(X(k.p1), Y(k.p1), X(k.p2), Y(k.p2), X(k.p), Y(k.p));
        break;
      case "Z":
        ctx.closePath();
        break;
    }
  }
}
