// LightBurn-style Image mode: a layer's image settings (`CutSettings.image`, doc::ImageSettings) and an image's Adjust
// Image values (`geometry.adjust`, doc::ImageAdjust), line interval <-> DPI, the command builders, and a small
// dithering pipeline used only for the client-side preview (the Image section, the Adjust image dialog and the
// dithered view of images on the canvas). The engine plans the real job (crates/plan/src/image.rs); this preview
// follows its formulas but resamples to an axis-aligned grid at the layer's line interval (capped in size), so it
// ignores the scan angle, the image's rotation and dot width correction. Pure (no DOM), so `npm test` covers it.

import { CMD } from "./commands.ts";
import type { Invocation } from "./client.ts";
import type { CutSettings } from "./types.ts";
import { tr } from "../workspace/tr.ts";

/** doc::DitherMode (LightBurn's `ditherMode`; LightBurn's "Dither" is `floyd_steinberg`). */
export type DitherMode =
  | "threshold"
  | "ordered"
  | "atkinson"
  | "floyd_steinberg"
  | "stucki"
  | "jarvis"
  | "newsprint"
  | "halftone"
  | "sketch"
  | "grayscale"
  /** LightBurn's 3D Sliced (galvo, `3dslice`): a depth map engraved one grey-level slice per pass, deepest first. */
  | "slice3d";

/**
 * One image's Adjust Image values (doc::ImageAdjust, `geometry.adjust`, absent while it changes nothing; LightBurn's
 * Bitmap `Gamma Contrast Brightness EnhanceRadius EnhanceAmount EnhanceDenoise`). Applied before dithering, in this
 * order: enhance, contrast, brightness, gamma.
 */
export interface ImageAdjust {
  /** Mid-tones: grey^gamma, so above 1 darkens them. 0.01..=10. */
  gamma: number;
  /** -100..=100 %: light tones lighter and dark tones darker. */
  contrast: number;
  /** -100..=100 %: every tone lighter or darker. */
  brightness: number;
  /** Unsharp-mask radius in image pixels, 0..=100 (0 = off). */
  enhance_radius: number;
  /** Unsharp-mask strength, 0..=10000 % (0 = off). */
  enhance_amount: number;
  /** Differences smaller than this many grey levels (of 255) are not sharpened. */
  denoise: number;
}

/** A layer's Image mode settings (doc::ImageSettings, `settings.image`): LightBurn's Image mode cut settings. */
export interface ImageSettings {
  dither: DitherMode | string;
  /** Scan line spacing, also the pixel size images are resampled to. DPI = 25.4 / interval. */
  interval_mm: number;
  negative: boolean;
  /** Engrave the image's own pixels as they are (already dithered elsewhere): no resampling, no dither. */
  pass_through: boolean;
  /** Shortens each scan run so dark areas do not bloom; 0..interval_mm. */
  dot_width_mm: number;
  angle_deg: number;
  angle_increment_deg: number;
  bidirectional: boolean;
  /** Grayscale only: power of the lightest engraved pixel (the layer's power is the darkest). */
  min_power_pct: number;
  halftone_cells_per_inch: number;
  halftone_angle_deg: number;
  /** 3D Sliced only (LightBurn's "Clean whole image"): cleanup passes clean everything, not only the last slice. */
  clean_whole_image?: boolean;
}

export const DEFAULT_ADJUST: ImageAdjust = { gamma: 1, contrast: 0, brightness: 0, enhance_radius: 0, enhance_amount: 0, denoise: 0 };

export const DEFAULT_IMAGE_SETTINGS: ImageSettings = {
  dither: "jarvis",
  interval_mm: 0.05,
  negative: false,
  pass_through: false,
  dot_width_mm: 0,
  angle_deg: 0,
  angle_increment_deg: 0,
  bidirectional: true,
  min_power_pct: 0,
  halftone_cells_per_inch: 50,
  halftone_angle_deg: 22.5,
};

/** The dither modes in LightBurn's order, with their LightBurn names. */
export const DITHER_MODES: { value: DitherMode; label: string; hint: string }[] = [
  { value: "threshold", label: "Threshold", hint: tr("apii.blackOrWhiteAt50Grey") },
  { value: "ordered", label: "Ordered", hint: tr("apii.regular8x8PatternFastVisibleTexture") },
  { value: "atkinson", label: "Atkinson", hint: tr("apii.errorDiffusionThatKeepsHighlightsAnd") },
  { value: "floyd_steinberg", label: "Dither", hint: tr("apii.floydSteinbergErrorDiffusion") },
  { value: "stucki", label: "Stucki", hint: tr("apii.wideErrorDiffusionSmoothPhotos") },
  { value: "jarvis", label: "Jarvis", hint: tr("apii.wideErrorDiffusionTheUsualChoice") },
  { value: "newsprint", label: "Newsprint", hint: tr("apii.clustered45DotsLikeNewspaperPrint") },
  { value: "halftone", label: "Halftone", hint: tr("apii.roundDotsOnAScreenOf") },
  { value: "sketch", label: "Sketch", hint: tr("apii.keepsEdgesAndDropsFlatAreas") },
  { value: "grayscale", label: "Grayscale", hint: tr("apii.variesThePowerPerPixelBetween") },
  { value: "slice3d", label: "3D Sliced", hint: tr("apii.depthMapOneGreyLevelSlice") },
];

/** DPI for a line interval in mm (null for an interval that is not a positive number). */
export function dpiFromInterval(intervalMm: number): number | null {
  return Number.isFinite(intervalMm) && intervalMm > 0 ? 25.4 / intervalMm : null;
}

/** Line interval in mm for a DPI, rounded to 0.1 µm (null for a DPI that is not a positive number). */
export function intervalFromDpi(dpi: number): number | null {
  if (!(Number.isFinite(dpi) && dpi > 0)) return null;
  return Math.round((25.4 / dpi) * 1e7) / 1e7;
}

type WithImage = CutSettings & { image?: Partial<ImageSettings> | null };

/** The layer settings' image settings over the defaults (`image` may be absent or partial). */
export function imageSettingsOf(s: CutSettings | undefined | null): ImageSettings {
  const img = (s as WithImage | undefined | null)?.image ?? undefined;
  return img ? { ...DEFAULT_IMAGE_SETTINGS, ...img } : DEFAULT_IMAGE_SETTINGS;
}

/** An image's Adjust Image values over the defaults (`adjust` is absent while it changes nothing). */
export function adjustOf(g: { adjust?: Partial<ImageAdjust> | null } | null | undefined): ImageAdjust {
  return g?.adjust ? { ...DEFAULT_ADJUST, ...g.adjust } : DEFAULT_ADJUST;
}

/** The Adjust Image fields that differ from the defaults. */
export function changedAdjust(a: ImageAdjust): (keyof ImageAdjust)[] {
  return (Object.keys(DEFAULT_ADJUST) as (keyof ImageAdjust)[]).filter((k) => a[k] !== DEFAULT_ADJUST[k]);
}

/** A partial change to a layer's image settings; `dpi` sets `interval_mm` to 25.4 / dpi (give one of the two). */
export type ImagePatch = Partial<ImageSettings> & { dpi?: number };

/** `layer.update { id, settings: { image: patch } }`: only the given image fields change. */
export function imageUpdate(layerId: string, patch: ImagePatch): Invocation {
  return { id: CMD.layerUpdate, params: { id: layerId, settings: { image: patch } } };
}

/** The command that sets images' Adjust Image values (optional: older engines lack it). */
export const IMAGE_ADJUST_CMD = "image.adjust";

/** Set the Adjust Image values of the image objects `ids` (only the given fields change). */
export function imageAdjust(ids: string[], adjust: Partial<ImageAdjust>): Invocation {
  return { id: IMAGE_ADJUST_CMD, params: { ids, adjust } };
}

// ---------------------------------------------------------------------------------------------------------------
// Preview pipeline. Grey values are 0 (black, full burn) .. 255 (white, no burn).

/** A single-channel image, row-major, values 0..255 (floats while processing). */
export interface GrayImage {
  width: number;
  height: number;
  data: Float32Array;
}

/** Pixel size of the preview raster for an object `widthMm` x `heightMm`, capped to `maxPx` on the longer side. */
export function previewSize(widthMm: number, heightMm: number, intervalMm: number, maxPx: number): { width: number; height: number; capped: boolean } {
  const iv = intervalMm > 0 && Number.isFinite(intervalMm) ? intervalMm : DEFAULT_IMAGE_SETTINGS.interval_mm;
  let w = Math.max(1, Math.round(Math.abs(widthMm) / iv));
  let h = Math.max(1, Math.round(Math.abs(heightMm) / iv));
  const k = Math.max(w, h) > maxPx ? maxPx / Math.max(w, h) : 1;
  if (k < 1) {
    w = Math.max(1, Math.round(w * k));
    h = Math.max(1, Math.round(h * k));
  }
  return { width: w, height: h, capped: k < 1 };
}

/** RGBA (as from `getImageData`) to grey, transparent pixels over white (Rec. 601 luma). */
export function grayFromRgba(rgba: ArrayLike<number>, width: number, height: number): GrayImage {
  const n = width * height;
  const data = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    const a = rgba[i * 4 + 3] / 255;
    const y = 0.299 * rgba[i * 4] + 0.587 * rgba[i * 4 + 1] + 0.114 * rgba[i * 4 + 2];
    data[i] = y * a + 255 * (1 - a);
  }
  return { width, height, data };
}

const clamp255 = (v: number) => (v < 0 ? 0 : v > 255 ? 255 : v);

/**
 * Adjust Image as the engine does it: enhance (unsharp mask against three box blurs of the radius, skipping
 * differences below `denoise` grey levels), then contrast (k = contrast / 100: a slope of 1 / (1 - k) above 0, 1 + k
 * below, around mid-grey) plus brightness (shift by brightness / 100), clamped, then gamma (grey^gamma on 0..1).
 * `radiusScale` converts the radius from source-image pixels to this image's pixels. Returns a new image.
 */
export function adjustImage(img: GrayImage, a: ImageAdjust, radiusScale = 1): GrayImage {
  const { width, height } = img;
  const out = new Float32Array(img.data);
  const amt = (a.enhance_amount || 0) / 100;
  if ((a.enhance_radius || 0) > 0 && amt > 0) {
    const r = Math.max(1, Math.round(a.enhance_radius * radiusScale));
    let blur = img;
    for (let i = 0; i < 3; i++) blur = boxBlur(blur, r);
    const th = Math.max(0, a.denoise || 0);
    for (let i = 0; i < out.length; i++) {
      const d = out[i] - blur.data[i];
      if (d !== 0 && Math.abs(d) >= th) out[i] = clamp255(out[i] + d * amt);
    }
  }
  const k = Math.max(-100, Math.min(100, a.contrast || 0)) / 100;
  const cf = k >= 0 ? 1 / Math.max(1e-3, 1 - k) : 1 + k;
  const b = (a.brightness || 0) / 100;
  const g = a.gamma > 0 && Number.isFinite(a.gamma) ? a.gamma : 1;
  if ((a.contrast || 0) !== 0 || b !== 0 || g !== 1) {
    for (let i = 0; i < out.length; i++) {
      const x = Math.min(1, Math.max(0, (out[i] / 255 - 0.5) * cf + 0.5 + b));
      out[i] = 255 * Math.pow(x, g);
    }
  }
  return { width, height, data: out };
}

/** Separable box blur of radius `r` pixels (edges clamped). */
export function boxBlur(img: GrayImage, r: number): GrayImage {
  const { width: w, height: h } = img;
  const tmp = new Float32Array(w * h);
  const out = new Float32Array(w * h);
  const span = 2 * r + 1;
  for (let y = 0; y < h; y++) {
    const row = y * w;
    let acc = 0;
    for (let k = -r; k <= r; k++) acc += img.data[row + Math.min(w - 1, Math.max(0, k))];
    for (let x = 0; x < w; x++) {
      tmp[row + x] = acc / span;
      acc += img.data[row + Math.min(w - 1, x + r + 1)] - img.data[row + Math.max(0, x - r)];
    }
  }
  for (let x = 0; x < w; x++) {
    let acc = 0;
    for (let k = -r; k <= r; k++) acc += tmp[Math.min(h - 1, Math.max(0, k)) * w + x];
    for (let y = 0; y < h; y++) {
      out[y * w + x] = acc / span;
      acc += tmp[Math.min(h - 1, y + r + 1) * w + x] - tmp[Math.max(0, y - r) * w + x];
    }
  }
  return { width: w, height: h, data: out };
}

/** Error-diffusion kernels: [dx, dy, weight] with their divisor. */
const KERNELS: Record<string, { taps: [number, number, number][]; div: number }> = {
  floyd_steinberg: { taps: [[1, 0, 7], [-1, 1, 3], [0, 1, 5], [1, 1, 1]], div: 16 },
  atkinson: { taps: [[1, 0, 1], [2, 0, 1], [-1, 1, 1], [0, 1, 1], [1, 1, 1], [0, 2, 1]], div: 8 },
  jarvis: {
    taps: [[1, 0, 7], [2, 0, 5], [-2, 1, 3], [-1, 1, 5], [0, 1, 7], [1, 1, 5], [2, 1, 3], [-2, 2, 1], [-1, 2, 3], [0, 2, 5], [1, 2, 3], [2, 2, 1]],
    div: 48,
  },
  stucki: {
    taps: [[1, 0, 8], [2, 0, 4], [-2, 1, 2], [-1, 1, 4], [0, 1, 8], [1, 1, 4], [2, 1, 2], [-2, 2, 1], [-1, 2, 2], [0, 2, 4], [1, 2, 2], [2, 2, 1]],
    div: 42,
  },
};

const BAYER8 = (() => {
  // Recursive Bayer matrix, thresholds (i + 0.5) / 64.
  let m = [[0]];
  while (m.length < 8) {
    const n = m.length;
    const next: number[][] = Array.from({ length: 2 * n }, () => new Array<number>(2 * n).fill(0));
    for (let y = 0; y < n; y++)
      for (let x = 0; x < n; x++) {
        const v = 4 * m[y][x];
        next[y][x] = v;
        next[y][x + n] = v + 2;
        next[y + n][x] = v + 3;
        next[y + n][x + n] = v + 1;
      }
    m = next;
  }
  return m.map((row) => row.map((v) => (v + 0.5) / 64));
})();

export interface DitherOptions {
  /** Pixel size in mm (for halftone and newsprint cells). */
  pixelMm: number;
  /** The layer's line interval (newsprint cells are 4 dots of it); defaults to `pixelMm`. */
  intervalMm?: number;
  halftoneCellsPerInch: number;
  halftoneAngleDeg: number;
  /** Grayscale: the lightest engraved pixel's power as a share of the layer's power (0..1). */
  minPowerFraction: number;
}

/**
 * Dither a grey image the way `mode` does. Returns one grey byte per pixel: 0 = burn, 255 = no burn; grayscale returns
 * the burn darkness as grey. LightBurn's names `dither` / `dithered` mean Floyd-Steinberg; unknown modes dither like
 * Jarvis.
 */
export function ditherImage(img: GrayImage, mode: string, o: DitherOptions): Uint8ClampedArray {
  const { width: w, height: h, data } = img;
  const out = new Uint8ClampedArray(w * h);
  const darkness = (i: number) => 1 - clamp255(data[i]) / 255;
  switch (mode) {
    case "threshold":
      for (let i = 0; i < out.length; i++) out[i] = darkness(i) >= 0.5 ? 0 : 255;
      return out;
    case "ordered":
      for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) out[y * w + x] = darkness(y * w + x) > BAYER8[y & 7][x & 7] ? 0 : 255;
      return out;
    case "halftone":
    case "newsprint": {
      // A round spot per screen cell: a dot fires when its darkness exceeds the area of the disc through it (the
      // engine's rule). Newsprint is a fixed 45° screen of cells 4 dots wide.
      const px = Math.max(1e-6, o.pixelMm);
      const [cellMm, deg] =
        mode === "halftone" ? [25.4 / Math.max(1e-6, o.halftoneCellsPerInch), o.halftoneAngleDeg] : [4 * Math.max(px, o.intervalMm ?? px), 45];
      const a = (-deg * Math.PI) / 180;
      const [cs, sn] = [Math.cos(a), Math.sin(a)];
      for (let y = 0; y < h; y++)
        for (let x = 0; x < w; x++) {
          const [mx, my] = [(x + 0.5) * px, (y + 0.5) * px];
          const u = (mx * cs - my * sn) / cellMm;
          const v = (mx * sn + my * cs) / cellMm;
          const [fx, fy] = [u - Math.round(u), v - Math.round(v)];
          out[y * w + x] = darkness(y * w + x) > Math.min(0.999, Math.PI * (fx * fx + fy * fy)) ? 0 : 255;
        }
      return out;
    }
    case "grayscale": {
      const min = Math.max(0, Math.min(1, o.minPowerFraction));
      for (let i = 0; i < out.length; i++) {
        const d = darkness(i);
        // Pure white is not engraved; everything else runs between min power and the layer's power.
        out[i] = d < 0.5 / 255 ? 255 : Math.round(255 * (1 - (min + d * (1 - min))));
      }
      return out;
    }
    case "slice3d":
    case "3dslice":
      // A depth map: shown as its grey levels (how many slices reach each dot); white is never engraved.
      for (let i = 0; i < out.length; i++) {
        const d = darkness(i);
        out[i] = d < 0.5 / 255 ? 255 : Math.round(255 * (1 - d));
      }
      return out;
    case "sketch": {
      // Sobel edge strength on darkness, as the engine: fires where |gradient| / 4 >= 0.25; outside dots repeat the
      // centre, so the border is no edge.
      for (let y = 0; y < h; y++)
        for (let x = 0; x < w; x++) {
          const c = darkness(y * w + x);
          const at = (dx: number, dy: number) => {
            const [xx, yy] = [x + dx, y + dy];
            return xx < 0 || yy < 0 || xx >= w || yy >= h ? c : darkness(yy * w + xx);
          };
          const gx = at(1, -1) + 2 * at(1, 0) + at(1, 1) - at(-1, -1) - 2 * at(-1, 0) - at(-1, 1);
          const gy = at(-1, 1) + 2 * at(0, 1) + at(1, 1) - at(-1, -1) - 2 * at(0, -1) - at(1, -1);
          out[y * w + x] = Math.hypot(gx, gy) / 4 >= 0.25 ? 0 : 255;
        }
      return out;
    }
    case "dither":
    case "dithered":
      return diffuse(img, KERNELS.floyd_steinberg, out);
    default:
      return diffuse(img, KERNELS[mode] ?? KERNELS.jarvis, out);
  }
}

function diffuse(img: GrayImage, k: { taps: [number, number, number][]; div: number }, out: Uint8ClampedArray): Uint8ClampedArray {
  const { width: w, height: h } = img;
  const buf = new Float32Array(img.data);
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const i = y * w + x;
      const old = buf[i];
      const nv = old <= 127.5 ? 0 : 255;
      out[i] = nv;
      const err = old - nv;
      if (err === 0) continue;
      for (const [dx, dy, wt] of k.taps) {
        const xx = x + dx;
        const yy = y + dy;
        if (xx >= 0 && xx < w && yy < h) buf[yy * w + xx] += (err * wt) / k.div;
      }
    }
  return out;
}

/**
 * The full preview pipeline on an image already resampled to the dot grid (with pass-through, its own pixels): the
 * image's adjustments (`radiusScale` = grid pixels per source pixel), negative, then the layer's dither.
 * `layerPowerPct` is the layer's (maximum) power, for grayscale min power.
 */
export function processImage(
  img: GrayImage,
  s: ImageSettings,
  adjust: ImageAdjust,
  pixelMm: number,
  layerPowerPct: number,
  radiusScale = 1,
): Uint8ClampedArray {
  let g = adjustImage(img, adjust, radiusScale);
  if (s.negative) {
    const d = g.data;
    for (let i = 0; i < d.length; i++) d[i] = 255 - d[i];
    g = { ...g, data: d };
  }
  return ditherImage(g, String(s.dither), {
    pixelMm,
    intervalMm: s.interval_mm,
    halftoneCellsPerInch: s.halftone_cells_per_inch,
    halftoneAngleDeg: s.halftone_angle_deg,
    minPowerFraction: layerPowerPct > 0 ? s.min_power_pct / layerPowerPct : 0,
  });
}

/** Grey bytes (0 = burn) to opaque RGBA for a canvas. */
export function grayToRgba(gray: Uint8ClampedArray): Uint8ClampedArray {
  const out = new Uint8ClampedArray(gray.length * 4);
  for (let i = 0; i < gray.length; i++) {
    out[i * 4] = out[i * 4 + 1] = out[i * 4 + 2] = gray[i];
    out[i * 4 + 3] = 255;
  }
  return out;
}

/** Share of burned pixels in a dither result (0..1, grey counts by its darkness). */
export function burnShare(gray: Uint8ClampedArray): number {
  let s = 0;
  for (let i = 0; i < gray.length; i++) s += 1 - gray[i] / 255;
  return gray.length ? s / gray.length : 0;
}

// ---------------------------------------------------------------------------------------------------------------
// Canvas view: whether images on the canvas show the original bitmap or the dithered preview, plus the Adjust image
// dialog's unsaved draft per image object (shown dithered on the canvas while the dialog is open). A tiny store for
// `useSyncExternalStore`.

export type ImageView = "original" | "dithered";

interface ViewState {
  view: ImageView;
  /** Object id -> the Adjust image dialog's unsaved values. */
  drafts: ReadonlyMap<string, ImageAdjust>;
}

let state: ViewState = { view: loadView(), drafts: new Map() };
const listeners = new Set<() => void>();

function loadView(): ImageView {
  try {
    return globalThis.localStorage?.getItem("openetch.imageView") === "dithered" ? "dithered" : "original";
  } catch {
    return "original";
  }
}

function emit(next: ViewState) {
  state = next;
  for (const l of listeners) l();
}

export const imageView = {
  subscribe(l: () => void): () => void {
    listeners.add(l);
    return () => listeners.delete(l);
  },
  get(): ViewState {
    return state;
  },
  setView(view: ImageView) {
    if (view === state.view) return;
    try {
      globalThis.localStorage?.setItem("openetch.imageView", view);
    } catch {
      // Per-viewer convenience only.
    }
    emit({ ...state, view });
  },
  setDraft(objectId: string, a: ImageAdjust | null) {
    const drafts = new Map(state.drafts);
    if (a) drafts.set(objectId, a);
    else if (!drafts.delete(objectId)) return;
    emit({ ...state, drafts });
  },
};

// ---- Adjust Image presets (LightBurn's Adjust Image preset list; engine /api/image-adjust/presets) ----

/** A saved preset: the image adjustments and/or the layer's image mode settings (as LightBurn's Save Preset keeps). */
export interface ImagePreset {
  name: string;
  adjust?: Partial<ImageAdjust>;
  image?: Partial<ImageSettings>;
}

/** The presets of GET /api/image-adjust/presets (rows without a name or without either part dropped), by name. */
export function imagePresetList(raw: unknown): ImagePreset[] {
  const list = raw && typeof raw === "object" ? (raw as { presets?: unknown }).presets : null;
  if (!Array.isArray(list)) return [];
  const part = (v: unknown) => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : undefined);
  const out: ImagePreset[] = [];
  for (const r of list) {
    if (!r || typeof r !== "object" || typeof (r as { name?: unknown }).name !== "string") continue;
    const adjust = part((r as { adjust?: unknown }).adjust) as Partial<ImageAdjust> | undefined;
    const image = part((r as { image?: unknown }).image) as Partial<ImageSettings> | undefined;
    if (!adjust && !image) continue;
    out.push({ name: (r as { name: string }).name, ...(adjust ? { adjust } : {}), ...(image ? { image } : {}) });
  }
  return out.sort((a, b) => a.name.localeCompare(b.name));
}

/** The dialog values after picking `p`: its adjustments over the defaults, or `current` when it holds none. */
export function adjustFromPreset(p: ImagePreset, current: ImageAdjust): ImageAdjust {
  if (!p.adjust) return current;
  const out: ImageAdjust = { ...DEFAULT_ADJUST };
  for (const k of Object.keys(DEFAULT_ADJUST) as (keyof ImageAdjust)[]) {
    const v = p.adjust[k];
    if (typeof v === "number" && Number.isFinite(v)) out[k] = v;
  }
  return out;
}

/** The body of PUT /api/image-adjust/presets/{name}: the adjustments, plus the layer's image settings when asked. */
export function imagePresetBody(a: ImageAdjust, image: ImageSettings | null): { adjust: ImageAdjust; image?: Partial<ImageSettings> } {
  return image ? { adjust: a, image } : { adjust: a };
}
