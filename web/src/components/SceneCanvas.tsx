// The Viewport's raster layers: bitmaps and the toolpath preview are drawn on <canvas> elements sized to the canvas
// area at device-pixel resolution, under (images) and over (toolpath) the SVG outlines. A view change repaints the
// visible region, once per frame, from caches that do not depend on the view: decoded ImageBitmaps per image (and per
// dithered preview), and the toolpath's segments uploaded once to the GPU (toolpathGL.ts; without WebGL2, per-tile
// Path2Ds on a 2D canvas). Hit-testing and selection stay in the SVG/interaction layer; these canvases never take
// pointer events.

import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Affine } from "../api/types.ts";
import { mul } from "../geometry.ts";
import {
  batchLook,
  devicePxPerImagePx,
  filledTiles,
  imageSmoothing,
  parseHexColor,
  viewMatrix,
  visibleBox,
  visibleTiles,
  type Toolpath,
  type ToolpathBatch,
  type ToolpathStyle,
  type View,
} from "../sceneLayers.ts";
import { ToolpathGL, type BatchStyle } from "./toolpathGL.ts";

export interface PaintArgs {
  view: View;
  size: [number, number];
  dpr: number;
}

/**
 * A canvas the size of the viewport, repainted (in the commit, so in step with the SVG transform) whenever the view,
 * size or `paint` changes. `paint` gets a context with an identity transform and a cleared canvas.
 */
export const CanvasLayer = memo(function CanvasLayer(props: {
  className: string;
  view: View;
  size: [number, number];
  dpr: number;
  paint: ((ctx: CanvasRenderingContext2D, a: PaintArgs) => void) | null;
  /** Data attributes for tests and measurements. */
  data?: Record<string, string | number>;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  const { view, size, dpr, paint } = props;
  useLayoutEffect(() => {
    const c = ref.current;
    if (!c) return;
    const w = Math.max(1, Math.round(size[0] * dpr));
    const h = Math.max(1, Math.round(size[1] * dpr));
    if (c.width !== w || c.height !== h) {
      c.width = w;
      c.height = h;
    }
    const ctx = c.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    ctx.clearRect(0, 0, w, h);
    paint?.(ctx, { view, size, dpr });
  }, [view, size, dpr, paint]);
  const data: Record<string, string | number> = {};
  for (const [k, v] of Object.entries(props.data ?? {})) data[`data-${k}`] = v;
  return <canvas ref={ref} className={`vp-layer ${props.className}`} style={{ width: size[0], height: size[1] }} {...data} />;
});

/** Device pixels per CSS pixel, following browser zoom and moves between screens. */
export function useDpr(): number {
  const [dpr, setDpr] = useState(() => (typeof window === "undefined" ? 1 : window.devicePixelRatio || 1));
  useEffect(() => {
    const on = () => setDpr(window.devicePixelRatio || 1);
    window.addEventListener("resize", on);
    return () => window.removeEventListener("resize", on);
  }, []);
  return dpr;
}

// --- images -----------------------------------------------------------------------------------------------------

export interface Bitmap {
  src: CanvasImageSource;
  width: number;
  height: number;
}

/** Decoded bitmaps by source URL (original images and dithered previews). */
const bitmaps = new Map<string, Bitmap>();
const loading = new Map<string, Promise<Bitmap>>();
/** Bitmaps kept beyond the ones on screen, for undo/redo and toggling the dithered view. */
const SPARE = 8;

async function loadBitmap(href: string): Promise<Bitmap> {
  const img = new Image();
  img.decoding = "async";
  img.src = href;
  await img.decode();
  if (typeof createImageBitmap === "function") {
    try {
      const b = await createImageBitmap(img);
      return { src: b, width: b.width, height: b.height };
    } catch {
      // Fall back to the decoded element.
    }
  }
  return { src: img, width: img.naturalWidth, height: img.naturalHeight };
}

/**
 * Loads the bitmaps for `hrefs` (once each) and returns a number that changes whenever one finishes, so a paint
 * callback depending on it runs again. Bitmaps no longer in use are dropped beyond a few spares.
 */
export function useBitmaps(hrefs: readonly string[]): number {
  const [version, setVersion] = useState(0);
  useEffect(() => {
    let live = true;
    for (const h of hrefs) {
      if (bitmaps.has(h)) {
        // Most recently used last.
        const b = bitmaps.get(h) as Bitmap;
        bitmaps.delete(h);
        bitmaps.set(h, b);
        continue;
      }
      let p = loading.get(h);
      if (!p) {
        p = loadBitmap(h);
        loading.set(h, p);
        p.then(
          (b) => {
            loading.delete(h);
            bitmaps.set(h, b);
          },
          () => loading.delete(h),
        );
      }
      p.then(
        () => live && setVersion((v) => v + 1),
        () => {},
      );
    }
    const used = new Set(hrefs);
    let spare = [...bitmaps.keys()].filter((k) => !used.has(k));
    spare = spare.slice(0, Math.max(0, spare.length - SPARE));
    for (const k of spare) {
      const b = bitmaps.get(k);
      bitmaps.delete(k);
      if (b && typeof ImageBitmap !== "undefined" && b.src instanceof ImageBitmap) b.src.close();
    }
    return () => {
      live = false;
    };
  }, [hrefs]);
  return version;
}

export function bitmapFor(href: string): Bitmap | undefined {
  return bitmaps.get(href);
}

/** `data:` URL of an image's own bytes, built once per image data (a document refetch brings equal new strings). */
const originals = new Map<string, string>();
export function originalHref(g: { mime: string; data_base64: string }): string {
  let h = originals.get(g.data_base64);
  if (h === undefined) {
    h = `data:${g.mime};base64,${g.data_base64}`;
    originals.set(g.data_base64, h);
    while (originals.size > 16) originals.delete(originals.keys().next().value as string);
  }
  return h;
}

export interface ImageItem {
  id: string;
  href: string;
  /** Object transform (field mm), including a live drag. */
  m: Affine;
  widthMm: number;
  heightMm: number;
}

/** Draws images centred on their local origin (bitmaps are y-down, the field is y-up). */
export function paintImages(ctx: CanvasRenderingContext2D, a: PaintArgs, items: readonly ImageItem[], alpha: number): void {
  const vm = viewMatrix(a.view, a.size, a.dpr);
  ctx.globalAlpha = alpha;
  for (const it of items) {
    const bmp = bitmapFor(it.href);
    if (!bmp) continue;
    const m = mul(vm, mul(it.m, [1, 0, 0, -1, 0, 0]));
    const sm = imageSmoothing(devicePxPerImagePx(m, it.widthMm, bmp.width));
    ctx.imageSmoothingEnabled = sm.enabled;
    if (sm.enabled) ctx.imageSmoothingQuality = sm.quality;
    ctx.setTransform(m[0], m[1], m[2], m[3], m[4], m[5]);
    ctx.drawImage(bmp.src, -it.widthMm / 2, -it.heightMm / 2, it.widthMm, it.heightMm);
  }
  ctx.globalAlpha = 1;
}

// --- toolpath ---------------------------------------------------------------------------------------------------

/** Path2Ds per batch: the whole batch and each tile, built on first use. */
const paths = new WeakMap<ToolpathBatch, { whole: Path2D | null; tiles: (Path2D | undefined)[] }>();

function pathOf(b: ToolpathBatch, from: number, to: number): Path2D {
  const p = new Path2D();
  const c = b.coords;
  for (let i = from; i < to; i++) {
    p.moveTo(c[4 * i], c[4 * i + 1]);
    p.lineTo(c[4 * i + 2], c[4 * i + 3]);
  }
  return p;
}

function strokeBatch(ctx: CanvasRenderingContext2D, b: ToolpathBatch, a: PaintArgs): void {
  let cache = paths.get(b);
  if (!cache) {
    cache = { whole: null, tiles: [] };
    paths.set(b, cache);
  }
  const vis = visibleTiles(b, visibleBox(a.view, a.size, 8));
  if (vis.length === 0) return;
  // Most of it on screen: one stroke of the whole batch beats many small ones.
  if (vis.length * 2 > filledTiles(b)) {
    cache.whole ??= pathOf(b, 0, b.start[b.start.length - 1]);
    ctx.stroke(cache.whole);
    return;
  }
  for (const t of vis) {
    let p = cache.tiles[t];
    if (!p) {
      p = pathOf(b, b.start[t], b.start[t + 1]);
      cache.tiles[t] = p;
    }
    ctx.stroke(p);
  }
}

/** The toolpath, 2D canvas version (no WebGL2): dashed travel, then marks; only the tiles on screen when zoomed in. */
export function paintToolpath(ctx: CanvasRenderingContext2D, a: PaintArgs, tp: Toolpath, st: ToolpathStyle): void {
  const m = viewMatrix(a.view, a.size, a.dpr);
  ctx.setTransform(m[0], m[1], m[2], m[3], m[4], m[5]);
  const px = 1 / a.view.scale;
  for (const b of tp.batches) {
    const look = batchLook(b, tp.maxE, st);
    ctx.globalAlpha = look.alpha;
    ctx.strokeStyle = look.color;
    ctx.lineWidth = look.widthPx * px;
    ctx.lineCap = look.caps ? "round" : "butt";
    ctx.setLineDash(look.dash ? [look.dash[0] * px, look.dash[1] * px] : []);
    strokeBatch(ctx, b, a);
  }
  ctx.setLineDash([]);
  ctx.globalAlpha = 1;
}

/** Any CSS colour as 0..1 RGBA (hex directly, anything else through a 2D canvas). */
const parsed = new Map<string, [number, number, number, number]>();
function rgbaOf(color: string, alpha: number): [number, number, number, number] {
  let c = parsed.get(color) ?? parseHexColor(color);
  if (!c) {
    const ctx = document.createElement("canvas").getContext("2d");
    if (ctx) {
      ctx.fillStyle = color;
      const norm = String(ctx.fillStyle);
      c = parseHexColor(norm);
      const m = c ? null : /rgba?\(([^)]+)\)/.exec(norm);
      if (m) {
        const v = m[1].split(",").map((x) => parseFloat(x));
        c = [v[0] / 255, v[1] / 255, v[2] / 255, v.length > 3 ? v[3] : 1];
      }
    }
    c ??= [0.87, 0.13, 0.13, 1];
  }
  parsed.set(color, c);
  return [c[0], c[1], c[2], c[3] * alpha];
}

/**
 * The toolpath layer: WebGL2 (toolpathGL.ts) with the segments uploaded once per preview and redrawn per view, or
 * the 2D canvas where WebGL2 is not available.
 */
export const ToolpathLayer = memo(function ToolpathLayer(props: { view: View; size: [number, number]; dpr: number; toolpath: Toolpath; style: ToolpathStyle }) {
  const { view, size, dpr, toolpath, style } = props;
  const ref = useRef<HTMLCanvasElement>(null);
  const gl = useRef<ToolpathGL | null>(null);
  const [fallback, setFallback] = useState(false);
  useLayoutEffect(() => {
    if (fallback || !ref.current) return;
    try {
      gl.current = new ToolpathGL(ref.current);
    } catch {
      setFallback(true);
      return;
    }
    return () => {
      gl.current?.dispose();
      gl.current = null;
    };
  }, [fallback]);
  useLayoutEffect(() => gl.current?.setData(toolpath), [toolpath, fallback]);
  const looks = useMemo(() => {
    const m = new Map<string, BatchStyle>();
    for (const b of toolpath.batches) {
      const l = batchLook(b, toolpath.maxE, style);
      m.set(b.key, { rgba: rgbaOf(l.color, l.alpha), widthPx: l.widthPx, dash: l.dash, caps: l.caps });
    }
    return m;
  }, [toolpath, style]);
  useLayoutEffect(() => gl.current?.draw(view, size, dpr, (k) => looks.get(k) ?? null), [view, size, dpr, looks, fallback]);
  const paint2d = useCallback((ctx: CanvasRenderingContext2D, a: PaintArgs) => paintToolpath(ctx, a, toolpath, style), [toolpath, style]);
  const data = useMemo(() => ({ "preview-segments": toolpath.segments, "preview-renderer": "2d" }), [toolpath]);
  if (fallback) return <CanvasLayer className="vp-toolpath" view={view} size={size} dpr={dpr} paint={paint2d} data={data} />;
  return (
    <canvas
      ref={ref}
      className="vp-layer vp-toolpath"
      style={{ width: size[0], height: size[1] }}
      data-preview-segments={toolpath.segments}
      data-preview-renderer="webgl2"
    />
  );
});
