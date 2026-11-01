// Pure logic for the File menu's further formats and the project / image tools (ARCHITECTURE.md "Files and image
// tools"; routes in crates/engine/src/userfiles.rs): which extension goes to which engine import format and which
// options each format takes, the query strings of /api/document/import, /api/export and /api/print, LightBurn's
// import options (group, select, centre), the engine's backup / template / prefs-backup lists, recent templates,
// pages, image masks, Trace Image options, 3D Sliced settings, autosave status and project notes. No DOM and no fetch,
// so `npm test` covers it (files.test.ts).

import { geometryCmds, toPathData } from "../geometry.ts";
import type { CutSettings, DocObject, GDocument, Layer } from "../api/types.ts";

/** Largest file `POST /api/document/import` takes (the engine's MAX_IMPORT_BYTES). */
export const MAX_IMPORT_BYTES = 100 * 1024 * 1024;

/** `format=` of POST /api/document/import. */
export type EngineFormat =
  | "svg"
  | "dxf"
  | "image"
  | "tiff"
  | "tga"
  | "pdf"
  | "ai"
  | "eps"
  | "ezd"
  | "hpgl"
  | "plt"
  | "gerber"
  | "drill"
  | "model"
  | "lbrn"
  /** A LightBurn template: opened as a new untitled document. */
  | "lbt";

/** Bitmaps browsers show themselves (sent as `format=image` with their MIME type). */
const BITMAP_MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  bmp: "image/bmp",
  gif: "image/gif",
  webp: "image/webp",
};

/** Gerber layer extensions (RS-274X: KiCad, Eagle, Altium, generic) and Excellon drill files. */
const GERBER_EXT = ["gbr", "ger", "gtl", "gbl", "gts", "gbs", "gto", "gbo", "gtp", "gbp", "gko", "gm1", "gml", "cmp", "sol", "art", "pho"];
const DRILL_EXT = ["drl", "xln", "exc", "drd", "txt-drill"];
const MODEL_EXT = ["stl", "obj", "off", "ply", "3mf", "amf"];

/** The engine import format of a file name, or null when the engine has no importer for it (by extension). */
export function engineFormatOf(name: string): EngineFormat | null {
  const ext = extOf(name);
  if (ext === "svg" || ext === "dxf" || ext === "pdf" || ext === "ai" || ext === "eps" || ext === "ezd") return ext;
  if (ext in BITMAP_MIME) return "image";
  if (ext === "tif" || ext === "tiff") return "tiff";
  if (ext === "tga") return "tga";
  if (ext === "hpgl" || ext === "hpg") return "hpgl";
  if (ext === "plt") return "plt";
  if (GERBER_EXT.includes(ext)) return "gerber";
  if (DRILL_EXT.includes(ext)) return "drill";
  if (MODEL_EXT.includes(ext)) return "model";
  if (ext === "lbrn") return "lbrn";
  if (ext === "lbt") return "lbt";
  return null;
}

/** Lower-case extension without the dot ("" when there is none). */
export function extOf(name: string): string {
  const base = name.split(/[\\/]/).pop() ?? name;
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : "";
}

/** MIME of a bitmap the engine is told about (`mime=`), or undefined to let it sniff the header. */
export function bitmapMime(name: string): string | undefined {
  return BITMAP_MIME[extOf(name)];
}

/** `model_format=` for a 3D model file. */
export function modelFormatOf(name: string): string | undefined {
  const ext = extOf(name);
  return MODEL_EXT.includes(ext) ? ext : undefined;
}

/** What the import dialog shows for a format. */
export type FormatGroup = "vector" | "bitmap" | "pdf" | "hpgl" | "gerber" | "model" | "lightburn";

export function formatGroup(f: EngineFormat): FormatGroup {
  switch (f) {
    case "image":
    case "tiff":
    case "tga":
      return "bitmap";
    case "pdf":
    case "ai":
      return "pdf";
    case "hpgl":
    case "plt":
      return "hpgl";
    case "gerber":
    case "drill":
      return "gerber";
    case "model":
      return "model";
    case "lbrn":
    case "lbt":
      return "lightburn";
    default:
      return "vector";
  }
}

/** Human name of a format, for toasts and the dialog. */
export function formatLabel(f: EngineFormat): string {
  return (
    {
      svg: "SVG",
      dxf: "DXF",
      image: "Image",
      tiff: "TIFF",
      tga: "TGA",
      pdf: "PDF",
      ai: "Adobe Illustrator",
      eps: "EPS",
      ezd: "EzCad",
      hpgl: "HPGL",
      plt: "HPGL",
      gerber: "Gerber",
      drill: "Excellon drill",
      model: "3D model",
      lbrn: "LightBurn (.lbrn)",
      lbt: "LightBurn template",
    } as const
  )[f];
}

/** Formats whose import takes `keep_position` (the others are always centred by the engine). */
const KEEPS_POSITION: ReadonlySet<EngineFormat> = new Set(["dxf", "pdf", "ai", "eps", "ezd", "hpgl", "plt", "gerber", "drill"]);
/** Formats whose import takes `split` (one object per path in a group). */
const SPLITS: ReadonlySet<EngineFormat> = new Set(["svg", "dxf", "pdf", "ai", "eps", "ezd", "hpgl", "plt", "gerber", "drill"]);

export function takesKeepPosition(f: EngineFormat): boolean {
  return KEEPS_POSITION.has(f);
}

/** Formats that need the options dialog before they are imported (LightBurn shows one for 3D models and Gerber). */
export function needsOptions(f: EngineFormat): boolean {
  return f === "model" || f === "gerber" || f === "drill";
}

/** `accept` of the Import / Open pickers: every extension the engine imports. */
export const ENGINE_ACCEPT = [
  ".svg",
  ".dxf",
  ...Object.keys(BITMAP_MIME).map((e) => `.${e}`),
  ".tif",
  ".tiff",
  ".tga",
  ".pdf",
  ".ai",
  ".eps",
  ".ezd",
  ".hpgl",
  ".hpg",
  ".plt",
  ...GERBER_EXT.map((e) => `.${e}`),
  ".drl",
  ".xln",
  ".exc",
  ...MODEL_EXT.map((e) => `.${e}`),
  ".lbrn",
  ".lbt",
].join(",");

// --- import options -----------------------------------------------------------------------------------------------

/** LightBurn's Settings > Import/Export toggles (kept in this browser). */
export interface ImportPrefs {
  /** Auto-group imported shapes (else the import's group is ungrouped). */
  group: boolean;
  /** Select imported shapes. */
  select: boolean;
  /** Centre imported shapes on the field (else keep the file's own coordinates where the format has them). */
  center: boolean;
  /** Import hidden Illustrator / PDF layers. */
  hiddenLayers: boolean;
  /** LightBurn's "Allow Importing to Tool Layers": the import may land on tool layer T1 / T2 (else never). */
  toolLayers: boolean;
}

export const DEFAULT_IMPORT_PREFS: ImportPrefs = { group: true, select: true, center: true, hiddenLayers: false, toolLayers: false };

/**
 * The layer an import goes to: the active layer, unless it is a tool layer (T1 / T2) and importing to tool layers is
 * off (LightBurn's default); then the first ordinary layer, or null (the engine picks).
 */
export function importTargetLayer(
  layers: readonly { id: string; tool?: string | null }[] | undefined,
  active: string | null,
  toolLayers: boolean,
): string | null {
  if (!active || toolLayers || !layers) return active;
  const l = layers.find((x) => x.id === active);
  if (!l?.tool) return active;
  return layers.find((x) => !x.tool)?.id ?? null;
}

export function importPrefsFrom(raw: unknown): ImportPrefs {
  const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const b = (k: keyof ImportPrefs) => (typeof o[k] === "boolean" ? (o[k] as boolean) : DEFAULT_IMPORT_PREFS[k]);
  return { group: b("group"), select: b("select"), center: b("center"), hiddenLayers: b("hiddenLayers"), toolLayers: b("toolLayers") };
}

/** 3D model options (LightBurn 2.2 Import 3D Model). */
export interface ModelOptions {
  rotation: [number, number, number];
  /** null: the model's own size. */
  widthMm: number | null;
  dpi: number;
  sixteenBit: boolean;
  invert: boolean;
  transparentBackground: boolean;
  /** null: automatic (the lowest point). */
  depthCutoffMm: number | null;
  equalize: boolean;
  outline: boolean;
}

export const DEFAULT_MODEL_OPTIONS: ModelOptions = {
  rotation: [0, 0, 0],
  widthMm: null,
  dpi: 254,
  sixteenBit: false,
  invert: false,
  transparentBackground: false,
  depthCutoffMm: null,
  equalize: false,
  outline: false,
};

export interface GerberOptions {
  kind: "auto" | "gerber" | "excellon";
  /** The board area outside the copper (isolation / resist removal). */
  invert: boolean;
}

export const DEFAULT_GERBER_OPTIONS: GerberOptions = { kind: "auto", invert: false };

/** Everything one import request can carry. */
export interface ImportRequest {
  format: EngineFormat;
  fileName: string;
  mode: "merge" | "replace";
  layer?: string | null;
  prefs: ImportPrefs;
  /** Scale to this width (vector, bitmap and model formats). */
  widthMm?: number | null;
  /** PDF / AI page (1-based). */
  page?: number | null;
  /** HPGL plotter units per mm (default 40). */
  unitsPerMm?: number | null;
  gerber?: GerberOptions;
  model?: ModelOptions;
}

/** The query string (without "?") of POST /api/document/import for `r`. Only options the format takes are sent. */
export function importQuery(r: ImportRequest): string {
  const q = new URLSearchParams();
  q.set("format", r.format);
  q.set("mode", r.mode);
  q.set("name", r.fileName);
  const lb = r.format === "lbrn" || r.format === "lbt";
  if (!lb && r.layer && r.mode === "merge") q.set("layer", r.layer);
  if (r.format === "image") {
    const m = bitmapMime(r.fileName);
    if (m) q.set("mime", m);
  }
  if (lb) return q.toString();
  // Allow Importing to Tool Layers: the engine then accepts a tool layer (or T1 / T2) as the target.
  if (r.prefs.toolLayers) q.set("tool_layers", "true");
  if (KEEPS_POSITION.has(r.format) && !r.prefs.center) q.set("keep_position", "true");
  const width = r.format === "model" ? r.model?.widthMm : r.widthMm;
  if (width != null && Number.isFinite(width) && width > 0) q.set("width_mm", String(width));
  const g = formatGroup(r.format);
  if (g === "pdf") {
    if (r.prefs.hiddenLayers) q.set("hidden_layers", "true");
    if (r.page != null && Number.isInteger(r.page) && r.page > 1) q.set("page", String(r.page));
  }
  if (g === "hpgl" && r.unitsPerMm != null && Number.isFinite(r.unitsPerMm) && r.unitsPerMm > 0) q.set("units_per_mm", String(r.unitsPerMm));
  if (g === "gerber" && r.gerber) {
    // A drill file is sent as format=drill, which already means Excellon.
    if (r.format === "gerber" && r.gerber.kind !== "auto") q.set("kind", r.gerber.kind);
    if (r.gerber.invert) q.set("invert", "true");
  }
  if (g === "model") {
    const m = r.model ?? DEFAULT_MODEL_OPTIONS;
    const f = modelFormatOf(r.fileName);
    if (f) q.set("model_format", f);
    if (m.rotation.some((v) => v !== 0)) q.set("rotation_deg", m.rotation.map((v) => String(v)).join(","));
    if (m.dpi !== 254 && m.dpi > 0) q.set("dpi", String(m.dpi));
    if (m.sixteenBit) q.set("sixteen_bit", "true");
    if (m.invert) q.set("invert", "true");
    if (m.transparentBackground) q.set("transparent_background", "true");
    if (m.depthCutoffMm != null && m.depthCutoffMm > 0) q.set("depth_cutoff_mm", String(m.depthCutoffMm));
    if (m.equalize) q.set("equalize", "true");
    if (m.outline) q.set("outline", "true");
  }
  return q.toString();
}

/** Whether the import's group should be dissolved afterwards (LightBurn "Auto-group imported shapes" off). */
export function ungroupAfter(f: EngineFormat, prefs: ImportPrefs): boolean {
  return !prefs.group && SPLITS.has(f);
}

/** `group` id of an import result, or null. */
export function resultGroup(result: unknown): string | null {
  const g = result && typeof result === "object" ? (result as { group?: unknown }).group : null;
  return typeof g === "string" && g ? g : null;
}

// --- export and print ---------------------------------------------------------------------------------------------

export type ExportFormat = "svg" | "dxf" | "ai" | "pdf";

export interface ExportOptions {
  format: ExportFormat;
  /** Objects to export; empty: the whole project. */
  ids: string[];
  colors: "layers" | "black";
  area: "bounds" | "workspace";
  /** SVG/AI/PDF stroke in mm. */
  strokeMm: number;
  /** SVG/AI/PDF: embed images. */
  images: boolean;
  /** DXF: curve flattening tolerance in mm. */
  toleranceMm: number;
  /** DXF: CIRCLE entities (LightBurn's keep arcs). */
  keepCircles: boolean;
}

export const DEFAULT_EXPORT: ExportOptions = {
  format: "svg",
  ids: [],
  colors: "layers",
  area: "bounds",
  strokeMm: 0.1,
  images: true,
  toleranceMm: 0.01,
  keepCircles: true,
};

/** Query of GET /api/export; only values that differ from the engine's defaults are sent. */
export function exportQuery(o: ExportOptions): string {
  const q = new URLSearchParams();
  q.set("format", o.format);
  if (o.ids.length) q.set("ids", o.ids.join(","));
  if (o.colors !== "layers") q.set("colors", o.colors);
  if (o.area !== "bounds") q.set("area", o.area);
  if (o.format === "dxf") {
    if (o.toleranceMm !== 0.01 && o.toleranceMm > 0) q.set("tolerance_mm", String(o.toleranceMm));
    if (!o.keepCircles) q.set("keep_circles", "false");
  } else {
    if (o.strokeMm !== 0.1 && o.strokeMm > 0) q.set("stroke_mm", String(o.strokeMm));
    if (!o.images) q.set("images", "false");
  }
  return q.toString();
}

/** The extension the engine gives an export. */
export function exportExt(f: ExportFormat): string {
  return f;
}

export interface PrintOptions {
  format: "pdf" | "svg";
  colors: "black" | "layers";
  lineWidthMm: number;
  /** Paper in mm; null: the area at 1:1. */
  paper: [number, number] | null;
  marginMm: number;
  area: "workspace" | "bounds";
  ids: string[];
}

export const PAPER_SIZES: { id: string; label: string; mm: [number, number] | null }[] = [
  { id: "actual", label: "Actual size (1:1)", mm: null },
  { id: "a4", label: "A4 (210 × 297 mm)", mm: [210, 297] },
  { id: "a3", label: "A3 (297 × 420 mm)", mm: [297, 420] },
  { id: "letter", label: "Letter (8.5 × 11 in)", mm: [215.9, 279.4] },
  { id: "legal", label: "Legal (8.5 × 14 in)", mm: [215.9, 355.6] },
];

export const DEFAULT_PRINT: PrintOptions = { format: "svg", colors: "black", lineWidthMm: 0.25, paper: null, marginMm: 10, area: "workspace", ids: [] };

export function printQuery(o: PrintOptions): string {
  const q = new URLSearchParams();
  q.set("format", o.format);
  q.set("colors", o.colors);
  if (o.lineWidthMm !== 0.25 && o.lineWidthMm > 0) q.set("line_width_mm", String(o.lineWidthMm));
  if (o.paper) {
    q.set("paper", `${o.paper[0]},${o.paper[1]}`);
    if (o.marginMm !== 10 && o.marginMm >= 0) q.set("margin_mm", String(o.marginMm));
  }
  if (o.area !== "workspace") q.set("area", o.area);
  if (o.ids.length) q.set("ids", o.ids.join(","));
  return q.toString();
}

/**
 * The page the browser prints an SVG from: the SVG at its own physical size (its width/height are in mm), centred,
 * with no browser margins. The SVG is the engine's own output, embedded as a data URL image so none of its content
 * runs as markup in the page.
 */
export function printPageHtml(svgDataUrl: string, title: string): string {
  const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c] ?? c);
  return `<!doctype html><html><head><meta charset="utf-8"><title>${esc(title)}</title><style>@page{margin:0}html,body{margin:0;background:#fff}body{display:flex;align-items:center;justify-content:center;min-height:100vh}img{display:block;max-width:100%}</style></head><body><img src="${esc(svgDataUrl)}" alt=""></body></html>`;
}

// --- engine lists -------------------------------------------------------------------------------------------------

export interface StoredFile {
  file: string;
  project: string | null;
  savedUnixS: number;
  size: number;
}

function storedOf(x: unknown): StoredFile | null {
  if (!x || typeof x !== "object") return null;
  const o = x as Record<string, unknown>;
  if (typeof o.file !== "string" || !o.file) return null;
  return {
    file: o.file,
    project: typeof o.project === "string" && o.project ? o.project : null,
    savedUnixS: typeof o.saved_unix_s === "number" ? o.saved_unix_s : 0,
    size: typeof o.size === "number" ? o.size : 0,
  };
}

function storedList(x: unknown): StoredFile[] {
  return (Array.isArray(x) ? x : [])
    .map(storedOf)
    .filter((s): s is StoredFile => s !== null)
    .sort((a, b) => b.savedUnixS - a.savedUnixS);
}

/** GET /api/backups: newest first. */
export function backupLists(res: unknown): { autosaves: StoredFile[]; backups: StoredFile[] } {
  const o = res && typeof res === "object" ? (res as Record<string, unknown>) : {};
  return { autosaves: storedList(o.autosaves), backups: storedList(o.backups) };
}

/** GET /api/prefs/backups: newest first. */
export function prefsBackupList(res: unknown): StoredFile[] {
  return storedList(res);
}

export interface FileSettings {
  autosave_minutes: number;
  autosave_keep: number;
  backups_keep: number;
}

export const DEFAULT_FILE_SETTINGS: FileSettings = { autosave_minutes: 5, autosave_keep: 10, backups_keep: 5 };

export function fileSettingsOf(res: unknown): FileSettings {
  const o = res && typeof res === "object" ? (res as Record<string, unknown>) : {};
  const n = (k: keyof FileSettings) => (typeof o[k] === "number" && Number.isFinite(o[k]) ? (o[k] as number) : DEFAULT_FILE_SETTINGS[k]);
  return { autosave_minutes: n("autosave_minutes"), autosave_keep: n("autosave_keep"), backups_keep: n("backups_keep") };
}

/** The engine's limits on the file settings, as a problem text or null. */
export function fileSettingsProblem(s: FileSettings): string | null {
  if (!Number.isInteger(s.autosave_minutes) || s.autosave_minutes < 0 || s.autosave_minutes > 1440) return "Autosave interval must be 0 to 1440 minutes.";
  if (!Number.isInteger(s.autosave_keep) || s.autosave_keep < 1 || s.autosave_keep > 50) return "Autosaves kept must be 1 to 50.";
  if (!Number.isInteger(s.backups_keep) || s.backups_keep < 0 || s.backups_keep > 50) return "Backups kept must be 0 to 50.";
  return null;
}

/** "Autosave every 5 min · last 10:42" style status parts. */
export function autosaveStatus(s: FileSettings, newest: StoredFile | null, nowMs: number): { on: boolean; minutes: number; agoMin: number | null } {
  const on = s.autosave_minutes > 0;
  const agoMin = newest && newest.savedUnixS > 0 ? Math.max(0, Math.round((nowMs / 1000 - newest.savedUnixS) / 60)) : null;
  return { on, minutes: s.autosave_minutes, agoMin };
}

export interface TemplateEntry {
  name: string;
  kind: "openetch" | "lbt";
  modifiedUnixS: number;
  size: number;
}

export function templateList(res: unknown): TemplateEntry[] {
  return (Array.isArray(res) ? res : [])
    .map((x): TemplateEntry | null => {
      if (!x || typeof x !== "object") return null;
      const o = x as Record<string, unknown>;
      if (typeof o.name !== "string" || !o.name) return null;
      return {
        name: o.name,
        kind: o.kind === "lbt" ? "lbt" : "openetch",
        modifiedUnixS: typeof o.modified_unix_s === "number" ? o.modified_unix_s : 0,
        size: typeof o.size === "number" ? o.size : 0,
      };
    })
    .filter((t): t is TemplateEntry => t !== null);
}

/** Most-recent-first list with `name` moved to the front (names compared ignoring case), at most `max`. */
export function pushRecent(list: readonly string[], name: string, max = 8): string[] {
  const n = name.trim();
  if (!n) return [...list].slice(0, max);
  return [n, ...list.filter((x) => x.toLowerCase() !== n.toLowerCase())].slice(0, max);
}

/** Recent templates that still exist (the engine's list decides), in recent order. */
export function liveRecent(recent: readonly string[], existing: readonly TemplateEntry[]): TemplateEntry[] {
  return recent.map((r) => existing.find((t) => t.name.toLowerCase() === r.toLowerCase())).filter((t): t is TemplateEntry => !!t);
}

/** A template name from a `.lbt` file name. */
export function templateNameOf(fileName: string): string {
  const base = fileName.split(/[\\/]/).pop() ?? fileName;
  return base.replace(/\.lbt$/i, "").replace(/[\\/:*?"<>|\u0000-\u001f]+/g, "_").trim().slice(0, 100) || "Template";
}

/** POST /api/bundle/import's answer, for the result list. */
export function bundleResult(res: unknown): { prefs: boolean; devices: string[]; libraries: string[]; skipped: string[] } {
  const o = res && typeof res === "object" ? (res as Record<string, unknown>) : {};
  const imp = o.imported && typeof o.imported === "object" ? (o.imported as Record<string, unknown>) : {};
  const strs = (x: unknown) => (Array.isArray(x) ? x.filter((s): s is string => typeof s === "string") : []);
  return { prefs: imp.prefs === true, devices: strs(imp.devices), libraries: strs(imp.libraries), skipped: strs(o.skipped) };
}

// --- project notes ------------------------------------------------------------------------------------------------

export interface Notes {
  text: string;
  show_on_load: boolean;
}

/** The document's notes (absent while empty and not shown on load). */
export function notesOf(doc: GDocument | null | undefined): Notes {
  const n = (doc as (GDocument & { notes?: Partial<Notes> | null }) | null | undefined)?.notes;
  return { text: typeof n?.text === "string" ? n.text : "", show_on_load: n?.show_on_load === true };
}

/**
 * Whether opening a document should show its notes: the project / opened file changed (or this page just loaded it),
 * the notes say "show on load" and are not empty. `prevKey` null is the first document this page sees.
 */
export function showNotesOnLoad(prevKey: string | null | undefined, key: string, notes: Notes): boolean {
  return prevKey !== key && notes.show_on_load && notes.text.trim().length > 0;
}

/**
 * The revision a file was opened at, or null: opening a project, a template, a backup or a file as a new project
 * starts a fresh timeline whose only step loads it (`doc.load`, `doc.import_lbrn2` or `doc.new` plus an import).
 */
export function openedRevision(t: { steps: readonly { commands: string; revision: number | null }[] }): number | null {
  if (t.steps.length !== 1) return null;
  const s = t.steps[0];
  return /(^|[^\w.])doc\.(load|import_lbrn2|new)($|[^\w])/.test(s.commands) ? s.revision : null;
}

// --- pages --------------------------------------------------------------------------------------------------------

export interface PageRow {
  /** null for a document without pages (it is one page). */
  id: string | null;
  name: string;
  objects: number;
  active: boolean;
}

interface DocPages {
  pages?: { id: string; name: string; objects?: unknown[] }[];
  active_page?: string | null;
}

/** The pages in order (as page.list reports them); a document without pages is one page. */
export function pagesOf(doc: GDocument | null | undefined): PageRow[] {
  if (!doc) return [];
  const d = doc as GDocument & DocPages;
  const pages = Array.isArray(d.pages) ? d.pages : [];
  if (pages.length === 0) return [{ id: null, name: "Page 1", objects: doc.objects.length, active: true }];
  const activeIdx = Math.max(0, pages.findIndex((p) => p.id === d.active_page));
  return pages.map((p, i) => ({
    id: p.id,
    name: p.name,
    objects: i === activeIdx ? doc.objects.length : Array.isArray(p.objects) ? p.objects.length : 0,
    active: i === activeIdx,
  }));
}

/** Index after moving page `from` one step `dir` (null when it cannot move). */
export function pageMoveTarget(count: number, from: number, dir: -1 | 1): number | null {
  const to = from + dir;
  return from < 0 || from >= count || to < 0 || to >= count ? null : to;
}

/** A unique "Page n" name for a new page. */
export function nextPageName(rows: readonly PageRow[]): string {
  const names = new Set(rows.map((r) => r.name.toLowerCase()));
  for (let n = rows.length + 1; ; n++) if (!names.has(`page ${n}`)) return `Page ${n}`;
}

// --- images and masks ---------------------------------------------------------------------------------------------

type DocMasks = GDocument & { image_masks?: Record<string, string> | null };

export function isImage(o: DocObject | undefined): boolean {
  return o?.geometry.kind === "image";
}

/** The mask (object or group id) of image `id`, or null. */
export function maskOf(doc: GDocument | null | undefined, id: string): string | null {
  const m = (doc as DocMasks | null | undefined)?.image_masks;
  const v = m && typeof m === "object" ? m[id] : undefined;
  return typeof v === "string" && v ? v : null;
}

/** Objects of a mask id: the object itself, or every object inside the group (nested groups too). */
export function maskMembers(doc: GDocument, mask: string): DocObject[] {
  const direct = doc.objects.find((o) => o.id === mask);
  if (direct) return [direct];
  const groups = doc.groups ?? [];
  const inside = new Set<string>([mask]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const g of groups) {
      if (g.parent && inside.has(g.parent) && !inside.has(g.id)) {
        inside.add(g.id);
        grew = true;
      }
    }
  }
  return doc.objects.filter((o) => o.group && inside.has(o.group));
}

/** Image id -> the mask's outline as SVG path data in field mm (for clipping the canvas image, even-odd). */
export function maskClips(doc: GDocument | null | undefined): Map<string, string> {
  const out = new Map<string, string>();
  const m = (doc as DocMasks | null | undefined)?.image_masks;
  if (!doc || !m) return out;
  for (const [img, mask] of Object.entries(m)) {
    if (!doc.objects.some((o) => o.id === img && o.geometry.kind === "image")) continue;
    const members = maskMembers(doc, mask).filter((o) => o.geometry.kind !== "image");
    const d = members.map((o) => toPathData(geometryCmds(o.geometry), o.transform)).join(" ");
    if (d.trim()) out.set(img, d);
  }
  return out;
}

/**
 * Apply Mask to Image needs exactly one image and one other unit (a closed shape or a group). `units` are the
 * selection's units (groups as one id), `objectOf` looks up an object id. Returns the pair, or null.
 */
export function maskPair(units: readonly string[], objectOf: (id: string) => DocObject | undefined): { image: string; mask: string } | null {
  if (units.length !== 2) return null;
  const [a, b] = units;
  const ia = isImage(objectOf(a));
  const ib = isImage(objectOf(b));
  if (ia === ib) return null;
  return ia ? { image: a, mask: b } : { image: b, mask: a };
}

/** The selected images (object ids). */
export function selectedImages(doc: GDocument | null | undefined, ids: readonly string[]): DocObject[] {
  if (!doc) return [];
  const set = new Set(ids);
  return doc.objects.filter((o) => set.has(o.id) && o.geometry.kind === "image");
}

/** Selected objects that Convert to Bitmap takes (vectors). */
export function selectedVectors(doc: GDocument | null | undefined, ids: readonly string[]): DocObject[] {
  if (!doc) return [];
  const set = new Set(ids);
  return doc.objects.filter((o) => set.has(o.id) && o.geometry.kind !== "image");
}

/** A download file name for an image object: its name plus the extension (characters file systems refuse replaced). */
export function imageFileName(name: string, ext: string, suffix = ""): string {
  const clean = `${name}${suffix}`.replace(/[\\/:*?"<>|\u0000-\u001f]+/g, "_").trim() || "Image";
  return `${clean}.${ext}`;
}

// --- Trace Image --------------------------------------------------------------------------------------------------

export interface TraceOptions {
  cutoff: number;
  threshold: number;
  ignoreLessThan: number;
  smoothness: number;
  optimize: number;
  centerline: boolean;
  sketch: boolean;
  transparency: boolean;
  deleteImage: boolean;
  /** Field rectangle `[minX, minY, maxX, maxY]`, or null for the whole image. */
  boundary: [number, number, number, number] | null;
}

/** LightBurn's defaults (Cutoff 0, Threshold 128, Optimize 0.2) and the engine's (ignore 2, smoothness 1). */
export const DEFAULT_TRACE: TraceOptions = {
  cutoff: 0,
  threshold: 128,
  ignoreLessThan: 2,
  smoothness: 1,
  optimize: 0.2,
  centerline: false,
  sketch: false,
  transparency: false,
  deleteImage: false,
  boundary: null,
};

/** A problem with the options (the engine refuses these too), or null. */
export function traceProblem(o: TraceOptions): string | null {
  const byte = (v: number) => Number.isInteger(v) && v >= 0 && v <= 255;
  if (!byte(o.cutoff) || !byte(o.threshold)) return "Cutoff and threshold must be whole numbers from 0 to 255.";
  if (o.cutoff > o.threshold) return "Cutoff must not be above the threshold.";
  if (!Number.isInteger(o.ignoreLessThan) || o.ignoreLessThan < 0) return "Ignore less than must be a whole number of pixels.";
  if (!(o.smoothness >= 0 && o.smoothness <= 1.33)) return "Smoothness must be from 0 to 1.33.";
  if (!(o.optimize >= 0 && o.optimize <= 100)) return "Optimize must be 0 or more.";
  return null;
}

/** Parameters of image.trace / image.trace_preview (only what differs from the engine's defaults, plus the flags). */
export function traceParams(id: string, o: TraceOptions, preview: boolean): Record<string, unknown> {
  const p: Record<string, unknown> = { id };
  if (o.cutoff !== 0) p.cutoff = o.cutoff;
  if (o.threshold !== 128) p.threshold = o.threshold;
  if (o.ignoreLessThan !== 2) p.ignore_less_than = o.ignoreLessThan;
  if (o.smoothness !== 1) p.smoothness = o.smoothness;
  if (o.optimize !== 0.2) p.optimize = o.optimize;
  if (o.centerline) p.centerline = true;
  if (o.sketch) p.sketch = true;
  if (o.transparency) p.transparency = true;
  if (o.boundary) p.boundary = { min: [o.boundary[0], o.boundary[1]], max: [o.boundary[2], o.boundary[3]] };
  if (!preview && o.deleteImage) p.delete_image = true;
  return p;
}

/** The end points of every segment of SVG path data (LightBurn's Show Points), at most `max`. */
export function pathPoints(d: string, max = 5000): [number, number][] {
  const out: [number, number][] = [];
  const re = /([MLCQZmlcqz])([^MLCQZmlcqz]*)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(d)) && out.length < max) {
    const nums = (m[2].match(/-?(?:\d+\.?\d*|\.\d+)(?:e[-+]?\d+)?/gi) ?? []).map(Number);
    const c = m[1].toUpperCase();
    const step = c === "C" ? 6 : c === "Q" ? 4 : 2;
    if (c === "Z") continue;
    for (let i = step - 2; i + 1 < nums.length && out.length < max; i += step) out.push([nums[i], nums[i + 1]]);
  }
  return out;
}

// --- Convert to Bitmap --------------------------------------------------------------------------------------------

export interface BitmapOptions {
  dpi: number;
  render: "cut_settings" | "fill_all" | "outlines";
  gray: number;
  lineWidthMm: number | null;
  keepOriginal: boolean;
}

export const DEFAULT_BITMAP: BitmapOptions = { dpi: 254, render: "cut_settings", gray: 128, lineWidthMm: null, keepOriginal: false };

export function bitmapParams(ids: string[], o: BitmapOptions): Record<string, unknown> {
  const p: Record<string, unknown> = { ids, render: o.render };
  if (o.dpi !== 254) p.dpi = o.dpi;
  if (o.gray !== 128) p.gray = o.gray;
  if (o.lineWidthMm != null && o.lineWidthMm > 0) p.line_width_mm = o.lineWidthMm;
  if (o.keepOriginal) p.keep_original = true;
  return p;
}

/** Bitmap size in pixels for a box of `w` x `h` mm at `dpi` (to warn about huge results). */
export function bitmapPixels(w: number, h: number, dpi: number): [number, number] {
  return [Math.max(1, Math.ceil((w / 25.4) * dpi)), Math.max(1, Math.ceil((h / 25.4) * dpi))];
}

// --- 3D Sliced ----------------------------------------------------------------------------------------------------

export const SLICE3D = "slice3d";

export function isSlice3d(dither: unknown): boolean {
  return dither === SLICE3D || dither === "3dslice";
}

export interface Slice3dState {
  /** Number of slices = the layer's main passes. */
  slices: number;
  /** Index into `sub_layers` of the cleanup pass, or null. */
  cleanup: number | null;
  /** Clean after this many passes. */
  everyN: number;
  cleanWholeImage: boolean;
}

export function slice3dState(layer: Layer): Slice3dState {
  const subs: CutSettings[] = layer.sub_layers ?? [];
  const k = subs.findIndex((s) => !!s.cleanup);
  const img = (layer.settings as CutSettings & { image?: { clean_whole_image?: boolean } }).image;
  return {
    slices: layer.settings.passes,
    cleanup: k >= 0 ? k : null,
    everyN: k >= 0 ? (subs[k].cleanup?.every_n_passes ?? 1) : Math.max(1, layer.settings.passes),
    cleanWholeImage: img?.clean_whole_image === true,
  };
}

/** Depth reached by `slices` passes of `depthPerPassMm` each (for the hint), rounded to 0.001 mm. */
export function sliceDepth(slices: number, depthPerPassMm: number): number {
  return Math.round(slices * depthPerPassMm * 1000) / 1000;
}

// --- misc ---------------------------------------------------------------------------------------------------------

/** The images an image tool works on: the selected ones (in selection order). */
export function firstImage(doc: GDocument | null | undefined, ids: readonly string[]): DocObject | null {
  return selectedImages(doc, ids)[0] ?? null;
}
