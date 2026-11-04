// Pure model of the MOPA materials UI (ARCHITECTURE.md "MOPA materials"): a library entry's finish (photo, result
// colour, surface, notes, lens, focus, lot), the library as a palette, the hue step model, the canvas's Result colours
// view (each layer drawn in the colour it should come out), bracket tests in the document, and the photo scaling the
// camera flow uses. Everything the components show or send is built here so `npm test` covers it (model.test.ts).

import type { GDocument, Layer } from "../api/types.ts";

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const obj = (v: unknown): Record<string, unknown> | null => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null);

/** A layer's library link (as libs/model.ts layerLink; kept here so libs/model.ts can import this module). */
function layerLink(layer: Layer): { library: string; path: string } | null {
  const l = obj((layer as { link?: unknown }).link);
  return l && typeof l.library === "string" && typeof l.path === "string" ? { library: l.library, path: l.path } : null;
}

/** The layer extras keys the engine writes (crates/commands/src/builtin/finish.rs, color_separate.rs, bracket.rs). */
export const RESULT_COLOR_KEY = "finish.color";
export const TARGET_COLOR_KEY = "finish.target";
export const BRACKET_CELL_KEY = "bracket.cell";

export const SURFACES = ["mill", "brushed", "polished", "mirror"] as const;
export type Surface = (typeof SURFACES)[number];

export const HUE_PARAMS = ["auto", "power", "speed", "frequency"] as const;
export type HueParam = (typeof HUE_PARAMS)[number];

export interface FinishPhoto {
  mime: string;
  width: number;
  height: number;
  /** The photo's average colour (#rrggbb). */
  average: string;
  takenUnixS: number;
}

/** A library entry's finish as GET /api/library shows it (`finish` on each entry; null when it has none). */
export interface EntryFinish {
  color: string | null;
  colorFromPhoto: boolean;
  surface: Surface | null;
  notes: string;
  lens: string;
  focusMm: number | null;
  supplier: string;
  lot: string;
  date: string;
  photo: FinishPhoto | null;
  /** The settings changed (hue step, bracket pick) after the photo was taken. */
  photoStale: boolean;
  updatedUnixS: number | null;
}

export function isSurface(v: unknown): v is Surface {
  return typeof v === "string" && (SURFACES as readonly string[]).includes(v);
}

/** `#rgb` / `#rrggbb` as lower-case `#rrggbb`, else null. */
export function normalHex(v: unknown): string | null {
  if (typeof v !== "string") return null;
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(v.trim());
  if (!m) return null;
  const h = m[1].length === 3 ? [...m[1]].map((c) => c + c).join("") : m[1];
  return `#${h.toLowerCase()}`;
}

export function parseFinish(raw: unknown): EntryFinish | null {
  const f = obj(raw);
  if (!f) return null;
  const p = obj(f.photo);
  return {
    color: normalHex(f.color),
    colorFromPhoto: f.color_from_photo === true,
    surface: isSurface(f.surface) ? f.surface : null,
    notes: str(f.notes),
    lens: str(f.lens),
    focusMm: isNum(f.focus_mm) ? f.focus_mm : null,
    supplier: str(f.supplier),
    lot: str(f.lot),
    date: str(f.date),
    photo: p
      ? { mime: str(p.mime), width: isNum(p.width) ? p.width : 0, height: isNum(p.height) ? p.height : 0, average: normalHex(p.average) ?? "#808080", takenUnixS: isNum(p.taken_unix_s) ? p.taken_unix_s : 0 }
      : null,
    photoStale: f.photo_stale === true,
    updatedUnixS: isNum(f.updated_unix_s) ? f.updated_unix_s : null,
  };
}

// ---- the finish form --------------------------------------------------------------------------------------------

export interface FinishForm {
  color: string | null;
  surface: Surface | null;
  notes: string;
  lens: string;
  focusMm: number | null;
  supplier: string;
  lot: string;
  date: string;
}

export function finishForm(f: EntryFinish | null): FinishForm {
  return {
    color: f && !f.colorFromPhoto ? f.color : null,
    surface: f?.surface ?? null,
    notes: f?.notes ?? "",
    lens: f?.lens ?? "",
    focusMm: f?.focusMm ?? null,
    supplier: f?.supplier ?? "",
    lot: f?.lot ?? "",
    date: f?.date ?? "",
  };
}

/** A `YYYY-MM-DD` that names a day of the calendar (the engine's rule, loosely: day 1..31). */
export function validDate(s: string): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (!m) return false;
  const mo = Number(m[2]);
  const d = Number(m[3]);
  return mo >= 1 && mo <= 12 && d >= 1 && d <= 31;
}

/** Why the form cannot be saved, per field (empty: it can). */
export function finishFormProblem(f: FinishForm): Partial<Record<keyof FinishForm, string>> {
  const out: Partial<Record<keyof FinishForm, string>> = {};
  if (f.color !== null && !normalHex(f.color)) out.color = "#rrggbb";
  if (f.date.trim() && !validDate(f.date.trim())) out.date = "YYYY-MM-DD";
  if (f.focusMm !== null && !(f.focusMm >= -100 && f.focusMm <= 100)) out.focusMm = "-100 … 100";
  if (f.notes.length > 4000) out.notes = "≤ 4000";
  for (const k of ["lens", "supplier", "lot"] as const) if (f[k].length > 200) out[k] = "≤ 200";
  return out;
}

/**
 * The PUT .../finish body for the changes from `before` to `after` (only changed fields). A colour of null means
 * "the photo's average" (or none).
 */
export function finishPatch(before: FinishForm, after: FinishForm): Record<string, unknown> {
  const p: Record<string, unknown> = {};
  if ((before.color ?? null) !== (after.color ?? null)) p.color = after.color ? normalHex(after.color) : null;
  if (before.surface !== after.surface) p.surface = after.surface;
  if (before.notes !== after.notes) p.notes = after.notes;
  if (before.lens !== after.lens) p.lens = after.lens.trim();
  if (before.focusMm !== after.focusMm) p.focus_mm = after.focusMm;
  if (before.supplier !== after.supplier) p.supplier = after.supplier.trim();
  if (before.lot !== after.lot) p.lot = after.lot.trim();
  if (before.date !== after.date) p.date = after.date.trim();
  return p;
}

/** The size a photo is scaled to before upload (long side at most `max`, never enlarged). */
export function scaledSize(w: number, h: number, max = 1280): { w: number; h: number } {
  if (!(w > 0 && h > 0)) return { w: 0, h: 0 };
  const f = Math.min(1, max / Math.max(w, h));
  return { w: Math.max(1, Math.round(w * f)), h: Math.max(1, Math.round(h * f)) };
}

/** Black or white text on a swatch of `hex` (relative luminance, WCAG). */
export function inkOn(hex: string): "#000000" | "#ffffff" {
  const h = normalHex(hex) ?? "#808080";
  const lin = (i: number) => {
    const c = parseInt(h.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const y = 0.2126 * lin(1) + 0.7152 * lin(3) + 0.0722 * lin(5);
  return y > 0.18 ? "#000000" : "#ffffff";
}

// ---- palette ---------------------------------------------------------------------------------------------------

export interface PaletteSwatch {
  id: string;
  path: string;
  material: string;
  group: string;
  description: string;
  color: string;
  surface: Surface | null;
  lot: string;
  date: string;
  hasPhoto: boolean;
  photoStale: boolean;
}

/** Swatches of a library tree: the entries with a result colour (in tree order). */
export function paletteOf(tree: readonly { name: string; groups: readonly { title: string; entries: readonly { id: string; path: string; description: string; finish?: EntryFinish | null }[] }[] }[]): PaletteSwatch[] {
  const out: PaletteSwatch[] = [];
  for (const m of tree)
    for (const g of m.groups)
      for (const e of g.entries) {
        const f = e.finish;
        if (!f?.color) continue;
        out.push({
          id: e.id,
          path: e.path,
          material: m.name,
          group: g.title,
          description: e.description,
          color: f.color,
          surface: f.surface,
          lot: f.lot,
          date: f.date,
          hasPhoto: !!f.photo,
          photoStale: f.photoStale,
        });
      }
  return out;
}

/** Swatches grouped by surface (mill, brushed, polished, mirror, then "" for none), only the groups that have some. */
export function bySurface(swatches: readonly PaletteSwatch[], only: Surface | "all" = "all"): { surface: Surface | null; swatches: PaletteSwatch[] }[] {
  const order: (Surface | null)[] = [...SURFACES, null];
  return order
    .filter((s) => only === "all" || s === only)
    .map((s) => ({ surface: s, swatches: swatches.filter((w) => w.surface === s) }))
    .filter((g) => g.swatches.length > 0);
}

// ---- Result colours view ------------------------------------------------------------------------------------------

/**
 * The colour `layer` is expected to come out: its linked preset's result colour (it follows the library), else the
 * one it was given (`finish.color`, set when a preset was assigned or by colour separation), else null.
 */
export function resultColorOf(layer: Layer, linkedColor: (library: string, path: string) => string | null): string | null {
  const link = layerLink(layer);
  if (link) {
    const c = linkedColor(link.library, link.path);
    if (c) return c;
  }
  return normalHex(layer.extras?.[RESULT_COLOR_KEY]);
}

/**
 * The document as the canvas draws it in Result colours view: each layer that has a result colour drawn in it and
 * marked `shownAsResult` (its fills are then drawn nearly solid, the finish rather than a tint); its identity colour
 * elsewhere is unchanged. The same object when no layer has a result colour.
 */
export function withResultColors(doc: GDocument, linkedColor: (library: string, path: string) => string | null): GDocument {
  let changed = false;
  const layers = doc.layers.map((l) => {
    const c = resultColorOf(l, linkedColor);
    if (!c) return l;
    changed = true;
    return { ...l, color: c, shownAsResult: true };
  });
  return changed ? { ...doc, layers } : doc;
}

/** A lookup of result colours by (library, path), library names ignoring case as the engine compares them. */
export function linkedColorLookup(library: string | null, tree: readonly { groups: readonly { entries: readonly { path: string; finish?: EntryFinish | null }[] }[] }[]): (lib: string, path: string) => string | null {
  const map = new Map<string, string>();
  for (const m of tree) for (const g of m.groups) for (const e of g.entries) if (e.finish?.color) map.set(e.path, e.finish.color);
  const name = library?.toLowerCase() ?? null;
  return (lib, path) => (name !== null && lib.toLowerCase() === name ? (map.get(path) ?? null) : null);
}

// ---- bracket tests ------------------------------------------------------------------------------------------------

export interface BracketCell {
  layer: string;
  /** Percent from the preset down the rows and across the columns. */
  rowPct: number;
  colPct: number;
  objects: string[];
}

export interface BracketTest {
  group: string;
  library: string;
  path: string;
  rows: string;
  columns: string;
  cells: BracketCell[];
}

/** The bracket tests in a document (groups the engine tagged `bracket.path`). */
export function bracketTests(doc: GDocument | null): BracketTest[] {
  if (!doc) return [];
  const out: BracketTest[] = [];
  for (const g of doc.groups ?? []) {
    const ex = (g as { extras?: Record<string, string> }).extras ?? {};
    if (!ex["bracket.path"]) continue;
    const members = doc.objects.filter((o) => o.group === g.id);
    const byLayer = new Map<string, string[]>();
    for (const o of members) byLayer.set(o.layer, [...(byLayer.get(o.layer) ?? []), o.id]);
    const cells: BracketCell[] = [];
    for (const [layer, objects] of byLayer) {
      const l = doc.layers.find((x) => x.id === layer);
      const cell = l?.extras?.[BRACKET_CELL_KEY];
      if (!cell) continue;
      const [r, c] = cell.split(",").map(Number);
      if (!Number.isFinite(r) || !Number.isFinite(c)) continue;
      cells.push({ layer, rowPct: r, colPct: c, objects });
    }
    out.push({ group: g.id, library: ex["bracket.library"] ?? "", path: ex["bracket.path"], rows: ex["bracket.rows"] ?? "", columns: ex["bracket.columns"] ?? "", cells });
  }
  return out;
}

/** The bracket box the selection is (exactly one box's objects), with its test. */
export function selectedCell(tests: readonly BracketTest[], selection: readonly string[]): { test: BracketTest; cell: BracketCell } | null {
  if (selection.length === 0) return null;
  const set = new Set(selection);
  for (const test of tests)
    for (const cell of test.cells) if (cell.objects.length === set.size && cell.objects.every((o) => set.has(o))) return { test, cell };
  return null;
}

/** Whether two boxes share a row or a column (calibration needs one setting to differ). */
export function sameLine(a: BracketCell, b: BracketCell): boolean {
  return a.layer !== b.layer && (a.rowPct === b.rowPct || a.colPct === b.colPct);
}

/** "+5 %" / "−10 %" / "0 %". */
export function pctText(v: number): string {
  if (v === 0) return "0 %";
  return `${v > 0 ? "+" : "−"}${Math.abs(v)} %`;
}

// ---- hue model -------------------------------------------------------------------------------------------------

export interface HueModel {
  power_pct: number;
  speed_mm_s: number;
  frequency_khz: number;
  frequency_from_khz: number;
  min_pulse_ns: number;
  big_jump_hues: number;
  brushed_power_pct: number;
  polished_power_pct: number;
  mirror_power_pct: number;
  source: string;
}

export const XTOOL_MODEL: HueModel = {
  power_pct: 0.5,
  speed_mm_s: 100,
  frequency_khz: 150,
  frequency_from_khz: 500,
  min_pulse_ns: 20,
  big_jump_hues: 4,
  brushed_power_pct: 0,
  polished_power_pct: 1,
  mirror_power_pct: 2,
  source: "xtool",
};

export function parseHueModel(raw: unknown): HueModel {
  const m = obj(raw) ?? {};
  const n = (k: keyof HueModel) => (isNum(m[k]) ? (m[k] as number) : (XTOOL_MODEL[k] as number));
  return {
    power_pct: n("power_pct"),
    speed_mm_s: n("speed_mm_s"),
    frequency_khz: n("frequency_khz"),
    frequency_from_khz: n("frequency_from_khz"),
    min_pulse_ns: n("min_pulse_ns"),
    big_jump_hues: n("big_jump_hues"),
    brushed_power_pct: n("brushed_power_pct"),
    polished_power_pct: n("polished_power_pct"),
    mirror_power_pct: n("mirror_power_pct"),
    source: typeof m.source === "string" ? m.source : "xtool",
  };
}

/** A nudge answer (POST /api/materials/nudge): the proposed settings and what changed. */
export interface NudgeResult {
  settings: Record<string, unknown>;
  param: string | null;
  delta: Record<string, number>;
  warnings: string[];
  applied: boolean;
}

export function parseNudge(raw: unknown): NudgeResult {
  const r = obj(raw) ?? {};
  const d = obj(r.delta) ?? {};
  const delta: Record<string, number> = {};
  for (const [k, v] of Object.entries(d)) if (isNum(v)) delta[k] = v;
  return {
    settings: obj(r.settings) ?? {},
    param: typeof r.param === "string" ? r.param : null,
    delta,
    warnings: Array.isArray(r.warnings) ? r.warnings.filter((w): w is string => typeof w === "string") : [],
    applied: r.applied === true,
  };
}

/** "Power 40 → 39.5 %" lines for a nudge's changes (speed through `speed`, which formats the user's unit). */
export function deltaLines(before: { power_pct: number; speed_mm_s: number; frequency_khz: number }, n: NudgeResult, speed: (v: number) => string): string[] {
  const s = n.settings;
  const out: string[] = [];
  const num = (v: unknown, fb: number) => (isNum(v) ? v : fb);
  const r = (v: number) => String(Math.round(v * 100) / 100);
  if ("power_pct" in n.delta) out.push(`${r(before.power_pct)} → ${r(num(s.power_pct, before.power_pct))} %`);
  if ("speed_mm_s" in n.delta) out.push(`${speed(before.speed_mm_s)} → ${speed(num(s.speed_mm_s, before.speed_mm_s))}`);
  if ("frequency_khz" in n.delta) out.push(`${r(before.frequency_khz)} → ${r(num(s.frequency_khz, before.frequency_khz))} kHz`);
  return out;
}

// ---- colour separation ------------------------------------------------------------------------------------------

export interface SepColor {
  index: number;
  color: string;
  pixels: number;
  share: number;
  background: boolean;
  traced: boolean;
  swatch: { name: string; color: string; distance: number; path: string | null } | null;
}

export function parseSeparation(raw: unknown): { colors: SepColor[]; png: string | null; width: number; height: number } {
  const r = obj(raw) ?? {};
  const colors = (Array.isArray(r.colors) ? r.colors : []).flatMap((c): SepColor[] => {
    const o = obj(c);
    if (!o || !isNum(o.index)) return [];
    const sw = obj(o.swatch);
    const link = obj(sw?.link);
    return [
      {
        index: o.index,
        color: normalHex(o.color) ?? "#808080",
        pixels: isNum(o.pixels) ? o.pixels : 0,
        share: isNum(o.share) ? o.share : 0,
        background: o.background === true,
        traced: o.traced !== false,
        swatch: sw ? { name: str(sw.name), color: normalHex(sw.color) ?? "#808080", distance: isNum(sw.distance) ? sw.distance : 0, path: link && typeof link.path === "string" ? link.path : null } : null,
      },
    ];
  });
  return { colors, png: typeof r.png_base64 === "string" ? r.png_base64 : null, width: isNum(r.width) ? r.width : 0, height: isNum(r.height) ? r.height : 0 };
}

export interface SeparateOptions {
  colors: number;
  background: "auto" | "none";
  minSpeckPx: number | null;
  smoothness: number;
  /** Library to match ("" for none). */
  library: string;
  link: boolean;
  maxDistance: number;
  skip: number[];
  deleteImage: boolean;
}

export const DEFAULT_SEPARATE: SeparateOptions = { colors: 4, background: "auto", minSpeckPx: null, smoothness: 1, library: "", link: true, maxDistance: 30, skip: [], deleteImage: false };

/** The POST /api/materials/separate body. */
export function separateBody(image: string, o: SeparateOptions, preview: boolean): Record<string, unknown> {
  const b: Record<string, unknown> = {
    image,
    colors: Math.max(1, Math.min(16, Math.round(o.colors))),
    background: o.background,
    smoothness: o.smoothness,
    library: o.library,
    link: o.link,
    max_distance: o.maxDistance,
    skip: o.skip,
  };
  if (o.minSpeckPx !== null) b.min_speck_px = Math.max(0, Math.round(o.minSpeckPx));
  if (preview) b.preview = true;
  else if (o.deleteImage) b.delete_image = true;
  return b;
}
