// Text, variable text and barcodes in the UI (LightBurn's Text tool, Text Options, Variable Text window and Create Bar
// Code): command builders for the engine's text commands (crates/commands/src/builtin/text.rs, ARCHITECTURE.md
// "Text"), font lists and the missing-font check, the defaults for new text, variable-text code help, and the
// placement maths of the on-canvas text editor. Pure; tested in textModel.test.ts.

import type { Invocation } from "../api/client.ts";
import type { DocObject, FontKind, GDocument, TextAlignH, TextAlignV, TextGeometry, VarMode, VariableState, Vec2 } from "../api/types.ts";

/** Command ids (see ARCHITECTURE.md "Text"). */
export const TXT = {
  shapeText: "shape.text",
  update: "text.update",
  applyPath: "text.apply_path",
  detachPath: "text.detach_path",
  convert: "text.convert_to_path",
  refresh: "text.refresh",
  setSerial: "var.set_serial",
  advance: "var.advance",
  loadCsv: "var.load_csv",
  clearCsv: "var.clear_csv",
  previewRow: "var.preview_row",
  barcode: "shape.barcode",
  fontList: "font.list",
} as const;

export type TextObject = DocObject & { geometry: TextGeometry };

export function isText(o: DocObject | null | undefined): o is TextObject {
  return !!o && o.geometry.kind === "text";
}

/** What the text shows now (the evaluated variable text, else the text itself). */
export function shownText(g: TextGeometry): string {
  return g.shown ?? g.text;
}

// ---- fonts -----------------------------------------------------------------------------------------------------------

export interface FontInfo {
  family: string;
  kind: FontKind;
  bold: boolean;
  italic: boolean;
}

export const FONT_KINDS: FontKind[] = ["system", "stroke", "shx", "jsf"];

/** `font.list`'s answer, checked. */
export function parseFontList(r: unknown): FontInfo[] {
  if (!Array.isArray(r)) return [];
  const out: FontInfo[] = [];
  for (const f of r) {
    if (!f || typeof f !== "object") continue;
    const x = f as Record<string, unknown>;
    if (typeof x.family !== "string" || !FONT_KINDS.includes(x.kind as FontKind)) continue;
    out.push({ family: x.family, kind: x.kind as FontKind, bold: x.bold === true, italic: x.italic === true });
  }
  return out;
}

export const sameFont = (a: { family: string; kind: FontKind }, b: { family: string; kind: FontKind }): boolean =>
  a.kind === b.kind && a.family.toLowerCase() === b.family.toLowerCase();

/** The font is not among the engine's fonts (false while the list is unknown). */
export function fontMissing(font: { family: string; kind: FontKind }, list: readonly FontInfo[] | null): boolean {
  if (!list) return false;
  return !list.some((f) => sameFont(f, font));
}

/** The engine's entry for `font`, if it has one. */
export function fontInfo(font: { family: string; kind: FontKind }, list: readonly FontInfo[] | null): FontInfo | null {
  return list?.find((f) => sameFont(f, font)) ?? null;
}

/** Fonts matching `query` (any part of the name, ignoring case), grouped by kind in FONT_KINDS order. */
export function fontGroups(list: readonly FontInfo[], query: string): { kind: FontKind; fonts: FontInfo[] }[] {
  const q = query.trim().toLowerCase();
  return FONT_KINDS.map((kind) => ({
    kind,
    fonts: list.filter((f) => f.kind === kind && (!q || f.family.toLowerCase().includes(q))).sort((a, b) => a.family.localeCompare(b.family)),
  })).filter((g) => g.fonts.length > 0);
}

/** CSS font-family for showing text in `font` in the browser (installed fonts render as themselves locally). */
export function cssFontFamily(font: { family: string; kind: FontKind }): string {
  const generic = font.kind === "system" ? "sans-serif" : "ui-sans-serif, sans-serif";
  if (font.kind !== "system") return generic;
  return `"${font.family.replace(/["\\]/g, "")}", ${generic}`;
}

// ---- text settings ---------------------------------------------------------------------------------------------------

export type TextMode = "normal" | VarMode;

/** Settings `text.update` and `shape.text` take (every field optional; ARCHITECTURE.md "Text"). */
export interface TextFields {
  text?: string;
  font?: { family: string; kind: FontKind };
  height_mm?: number;
  h_spacing?: number;
  v_spacing?: number;
  align_h?: TextAlignH;
  align_v?: TextAlignV;
  bold?: boolean;
  italic?: boolean;
  upper?: boolean;
  welded?: boolean;
  bend_deg?: number;
  distort?: boolean;
  max_width_mm?: number;
  squeeze?: boolean;
  ignore_empty_vars?: boolean;
  variable?: { mode: TextMode; offset?: number };
  path_offset_mm?: number;
}

/** What new text starts with (LightBurn's Text Options while the Text tool is active); kept in this browser. */
export interface TextDefaults {
  font: { family: string; kind: FontKind } | null;
  height_mm: number;
  h_spacing: number;
  v_spacing: number;
  align_h: TextAlignH;
  align_v: TextAlignV;
  bold: boolean;
  italic: boolean;
  upper: boolean;
  welded: boolean;
}

/** LightBurn's defaults: 5 mm, middle / middle, welded; the engine picks the font (Arial, else Hershey Sans). */
export const DEFAULT_TEXT: TextDefaults = {
  font: null,
  height_mm: 5,
  h_spacing: 0,
  v_spacing: 0,
  align_h: "middle",
  align_v: "middle",
  bold: false,
  italic: false,
  upper: false,
  welded: true,
};

export const TEXT_DEFAULTS_KEY = "openetch.textDefaults";

const ALIGN_H: TextAlignH[] = ["left", "middle", "right"];
const ALIGN_V: TextAlignV[] = ["top", "middle", "bottom"];

/** Stored defaults, checked field by field (anything odd falls back to DEFAULT_TEXT). */
export function parseTextDefaults(raw: unknown): TextDefaults {
  const d: TextDefaults = { ...DEFAULT_TEXT };
  if (!raw || typeof raw !== "object") return d;
  const x = raw as Record<string, unknown>;
  const f = x.font as Record<string, unknown> | null | undefined;
  if (f && typeof f.family === "string" && f.family.trim() && FONT_KINDS.includes(f.kind as FontKind)) d.font = { family: f.family, kind: f.kind as FontKind };
  const num = (v: unknown, lo: number, hi: number) => typeof v === "number" && Number.isFinite(v) && v >= lo && v <= hi;
  if (num(x.height_mm, 0.001, 10000) && (x.height_mm as number) > 0) d.height_mm = x.height_mm as number;
  if (num(x.h_spacing, -100, 1000)) d.h_spacing = x.h_spacing as number;
  if (num(x.v_spacing, -100, 1000)) d.v_spacing = x.v_spacing as number;
  if (ALIGN_H.includes(x.align_h as TextAlignH)) d.align_h = x.align_h as TextAlignH;
  if (ALIGN_V.includes(x.align_v as TextAlignV)) d.align_v = x.align_v as TextAlignV;
  for (const k of ["bold", "italic", "upper", "welded"] as const) if (typeof x[k] === "boolean") d[k] = x[k] as boolean;
  return d;
}

export function loadTextDefaults(): TextDefaults {
  try {
    return parseTextDefaults(JSON.parse(globalThis.localStorage?.getItem(TEXT_DEFAULTS_KEY) ?? "null"));
  } catch {
    return { ...DEFAULT_TEXT };
  }
}

export function saveTextDefaults(d: TextDefaults): void {
  try {
    globalThis.localStorage?.setItem(TEXT_DEFAULTS_KEY, JSON.stringify(d));
  } catch {
    // Storage unavailable: the defaults last for this visit.
  }
}

/** The settings of an existing text, as defaults for the next one (LightBurn keeps the last used options). */
export function defaultsFrom(g: TextGeometry): TextDefaults {
  return {
    font: { family: g.font.family, kind: g.font.kind },
    height_mm: g.height_mm,
    h_spacing: g.h_spacing,
    v_spacing: g.v_spacing,
    align_h: g.align_h,
    align_v: g.align_v,
    bold: g.bold,
    italic: g.italic,
    upper: g.upper,
    welded: g.welded,
  };
}

/** The fields of `TextDefaults` that change, as text.update fields. */
export function defaultsToFields(d: Partial<TextDefaults>): TextFields {
  const f: TextFields = {};
  for (const [k, v] of Object.entries(d)) {
    if (v === undefined || (k === "font" && v === null)) continue;
    (f as Record<string, unknown>)[k] = v;
  }
  return f;
}

/** `shape.text`: new text `text` with its alignment point at `center`. */
export function shapeText(text: string, center: Vec2, d: TextDefaults, layer: string | null): Invocation {
  const params: Record<string, unknown> = {
    text,
    center: [round4(center[0]), round4(center[1])],
    height_mm: d.height_mm,
    h_spacing: d.h_spacing,
    v_spacing: d.v_spacing,
    align_h: d.align_h,
    align_v: d.align_v,
    bold: d.bold,
    italic: d.italic,
    upper: d.upper,
    welded: d.welded,
  };
  if (d.font) params.font = d.font;
  if (layer) params.layer = layer;
  return { id: TXT.shapeText, params };
}

export function textUpdate(id: string, fields: TextFields): Invocation {
  return { id: TXT.update, params: { id, ...fields } };
}

export const applyPath = (id: string, path: string, offset_mm = 0): Invocation => ({ id: TXT.applyPath, params: { id, path, offset_mm } });
export const detachPath = (id: string): Invocation => ({ id: TXT.detachPath, params: { id } });
export const convertText = (ids: string[]): Invocation => ({ id: TXT.convert, params: { ids } });
export const refreshText = (ids?: string[]): Invocation => ({ id: TXT.refresh, params: ids ? { ids } : {} });
export const fontList = (): Invocation => ({ id: TXT.fontList, params: {} });

export function textMode(g: TextGeometry): TextMode {
  return g.variable?.mode ?? "normal";
}

/** Text Mode list in LightBurn's words. */
export const TEXT_MODES: { id: TextMode; key: "text.mode.normal" | "text.mode.dateTime" | "text.mode.serial" | "text.mode.csv" | "text.mode.cutSetting" }[] = [
  { id: "normal", key: "text.mode.normal" },
  { id: "date_time", key: "text.mode.dateTime" },
  { id: "serial", key: "text.mode.serial" },
  { id: "csv", key: "text.mode.csv" },
  { id: "cut_setting", key: "text.mode.cutSetting" },
];

/**
 * LightBurn's Variable Text Formatting codes per mode (what the engine implements: crates/text/src/vars.rs), as
 * [code, meaning] rows plus an example format.
 */
export const VAR_HELP: Record<VarMode, { rows: [string, string][]; example: string; note: string }> = {
  serial: {
    rows: [
      ["d", "decimal digit (the count of d is the width)"],
      ["h / H", "hexadecimal digit, lower / upper case"],
      ["0", "pad with leading zeros (0ddddd)"],
      ["'text'", "copied as it is"],
    ],
    example: "'SN-'0dddd",
    note: "One run of d, h or H; a shorter run shows the last digits. Mixed or split runs show \"bad serial format\".",
  },
  date_time: {
    rows: [
      ["d dd ddd dddd", "day, 2-digit day, short / long weekday"],
      ["M MM MMM MMMM", "month, 2-digit month, short / long name"],
      ["yy yyyy", "2- / 4-digit year"],
      ["h hh / H HH", "hour (12 h with AP) / 24 h"],
      ["m mm s ss", "minutes, seconds"],
      ["z zzz", "milliseconds"],
      ["AP ap", "AM/PM, am/pm"],
      ["w W", "ISO week, ISO year/week"],
      ["t", "time zone (UTC offset)"],
      ["'text'", "copied as it is ('' is one quote)"],
    ],
    example: "yyyy-MM-dd HH:mm",
    note: "Evaluated when a run is requested; the canvas shows the time of the last edit.",
  },
  csv: {
    rows: [
      ["%0, %1, …", "column 0, 1, … of the current row"],
      ["%%", "a percent sign"],
    ],
    example: "%0 %1",
    note: "The row is Current plus this object's Offset (row 0 is the first line of the file).",
  },
  cut_setting: {
    rows: [
      ["s S", "speed, with units"],
      ["p P / m M", "max / min power, with %"],
      ["n", "number of passes"],
      ["k K", "frequency, with kHz"],
      ["q Q", "Q-pulse width, with ns"],
      ["i I / d D", "interval, with units / DPI"],
      ["z Z", "Z offset, with units"],
      ["L", "laser (device) name"],
      ["C##", "use layer ## for the rest (C03p)"],
      ["'text'", "copied as it is"],
    ],
    example: "'P'p 'S'S",
    note: "Values come from the text's own layer unless C## names another.",
  },
};

// ---- variable text counter ------------------------------------------------------------------------------------------

export const DEFAULT_VARIABLES: VariableState = { start: 0, end: 999, current: 0, advance_by: 1, auto_advance: false, csv: null };

export function variablesOf(doc: GDocument | null): VariableState {
  return { ...DEFAULT_VARIABLES, ...(doc?.variables ?? {}) };
}

export interface SerialFields {
  start?: number;
  end?: number;
  current?: number;
  advance_by?: number;
  auto_advance?: boolean;
  reset?: boolean;
}

/** Why these counter settings would be refused, or null. */
export function serialProblem(v: VariableState, f: SerialFields): string | null {
  const n = { ...v, ...f };
  for (const k of ["start", "end", "current", "advance_by"] as const) {
    if (!Number.isSafeInteger(n[k])) return `${k.replace("_", " ")} must be a whole number`;
  }
  if (n.start > n.end) return `Start (${n.start}) is after End (${n.end})`;
  return null;
}

export const setSerial = (f: SerialFields): Invocation => ({ id: TXT.setSerial, params: { ...f } });
export const advance = (steps: number): Invocation => ({ id: TXT.advance, params: { steps } });
export const loadCsv = (content: string, name: string): Invocation => ({ id: TXT.loadCsv, params: { content, name } });
export const clearCsv = (): Invocation => ({ id: TXT.clearCsv, params: {} });
export const previewRow = (value?: number): Invocation => ({ id: TXT.previewRow, params: value === undefined ? {} : { value } });

/** The engine refuses CSV files over 10 MB. */
export const MAX_CSV_BYTES = 10_000_000;

export interface RowPreview {
  value: number;
  items: { id: string; shown: string }[];
}

export function parseRowPreview(r: unknown): RowPreview | null {
  if (!r || typeof r !== "object") return null;
  const x = r as Record<string, unknown>;
  if (typeof x.value !== "number" || !Array.isArray(x.items)) return null;
  const items = x.items
    .filter((i): i is { id: string; shown: string } => !!i && typeof i.id === "string" && typeof i.shown === "string")
    .map((i) => ({ id: i.id, shown: i.shown }));
  return { value: x.value, items };
}

/** Whether any text or barcode of the document uses variable text. */
export function usesVariables(doc: GDocument | null): boolean {
  return !!doc?.objects.some((o) => (isText(o) && !!o.geometry.variable) || !!barcodeParams(o)?.variable);
}

// ---- barcodes --------------------------------------------------------------------------------------------------------

export const BARCODE_KEY = "openetch.barcode";

export interface BarcodeParams {
  kind: string;
  data: string;
  module_mm?: number | null;
  height_mm?: number | null;
  quiet_zone?: number | null;
  invert?: boolean;
  error_correction?: string | null;
  force_square?: boolean | null;
  show_text?: boolean;
  variable?: { mode: TextMode; offset?: number } | null;
}

export interface BarcodeKind {
  id: string;
  label: string;
  twoD: boolean;
  /** Error correction choices (value, label), for the kinds that have them. */
  ec?: [string, string][];
}

/** The 14 kinds the engine can encode, in LightBurn's naming. */
export const BARCODE_KINDS: BarcodeKind[] = [
  { id: "qr", label: "QR Code", twoD: true, ec: [["L", "L (7 %)"], ["M", "M (15 %)"], ["Q", "Q (25 %)"], ["H", "H (30 %)"]] },
  { id: "data_matrix", label: "Data Matrix", twoD: true },
  { id: "aztec", label: "Aztec", twoD: true, ec: [["23", "23 %"], ["33", "33 %"], ["50", "50 %"]] },
  { id: "pdf417", label: "PDF417", twoD: true, ec: "012345678".split("").map((n): [string, string] => [n, `Level ${n}`]) },
  { id: "code128", label: "Code 128", twoD: false },
  { id: "code39", label: "Code 39", twoD: false },
  { id: "code93", label: "Code 93", twoD: false },
  { id: "codabar", label: "Codabar", twoD: false },
  { id: "ean8", label: "EAN-8", twoD: false },
  { id: "ean13", label: "EAN-13", twoD: false },
  { id: "upc_a", label: "UPC-A", twoD: false },
  { id: "upc_e", label: "UPC-E", twoD: false },
  { id: "itf", label: "ITF", twoD: false },
  { id: "telepen", label: "Telepen", twoD: false },
];

/**
 * LightBurn's names of the barcode types the engine may add later (DES-055: the 2 of 5 variants, ISBN, VIN, Plessey,
 * PDF417 Compact / Micro, Micro QR, rMQR, Chinese Sensible, Grid Matrix, AprilTag, ArUco). A kind the engine offers
 * that is in neither list is shown under its id.
 */
const HAN_XIN_EC: [string, string][] = [["1", "L1 (~8 %)"], ["2", "L2 (~15 %)"], ["3", "L3 (~23 %)"], ["4", "L4 (~30 %)"]];
const GRID_MATRIX_EC: [string, string][] = [["1", "L1 (~10 %)"], ["2", "L2 (~20 %)"], ["3", "L3 (~30 %)"], ["4", "L4 (~40 %)"], ["5", "L5 (~50 %)"]];

const MORE_KIND_LABELS: Record<string, { label: string; twoD: boolean; ec?: [string, string][] }> = {
  iata_2of5: { label: "2 of 5 IATA", twoD: false },
  code2of5_iata: { label: "2 of 5 IATA", twoD: false },
  industrial_2of5: { label: "2 of 5 Industrial", twoD: false },
  code2of5_industrial: { label: "2 of 5 Industrial", twoD: false },
  isbn: { label: "ISBN", twoD: false },
  vin: { label: "VIN", twoD: false },
  plessey: { label: "Plessey", twoD: false },
  pdf417_compact: { label: "PDF417 Compact", twoD: true },
  micro_pdf417: { label: "PDF417 Micro", twoD: true },
  pdf417_micro: { label: "PDF417 Micro", twoD: true },
  micro_qr: { label: "QR Micro", twoD: true },
  qr_micro: { label: "QR Micro", twoD: true },
  rmqr: { label: "QR Micro Rectangular", twoD: true },
  chinese_sensible: { label: "Chinese Sensible Code", twoD: true, ec: HAN_XIN_EC },
  han_xin: { label: "Chinese Sensible Code", twoD: true, ec: HAN_XIN_EC },
  grid_matrix: { label: "Grid Matrix", twoD: true, ec: GRID_MATRIX_EC },
  apriltag_16h5: { label: "AprilTag 16h5", twoD: true },
  apriltag_25h9: { label: "AprilTag 25h9", twoD: true },
  apriltag_36h10: { label: "AprilTag 36h10", twoD: true },
  apriltag_36h11: { label: "AprilTag 36h11", twoD: true },
  // The ArUco library's ARUCO_MIP_36h12, which LightBurn lists as "ArUco MIP 36h11".
  aruco_mip_36h12: { label: "ArUco MIP 36h11", twoD: true },
  aruco_mip_36h11: { label: "ArUco MIP 36h11", twoD: true },
  aruco_7x7: { label: "ArUco OpenCV 7x7", twoD: true },
  aruco_opencv_7x7: { label: "ArUco OpenCV 7x7", twoD: true },
};

/**
 * The kinds the engine's `shape.barcode` takes, read from its input schema: the `kind` property's enum when it has
 * one, else the names its description lists ("qr, data_matrix, ..."). Known kinds keep this file's order and labels;
 * more (types the engine added) follow, labelled by LightBurn's name when known. Falls back to BARCODE_KINDS.
 */
export function barcodeKindsFrom(inputSchema: unknown): BarcodeKind[] {
  const props = (inputSchema && typeof inputSchema === "object" ? (inputSchema as { properties?: Record<string, unknown> }).properties : undefined) ?? {};
  const k = (props.kind && typeof props.kind === "object" ? props.kind : {}) as { enum?: unknown; description?: unknown };
  let ids: string[] = [];
  if (Array.isArray(k.enum)) ids = k.enum.filter((x): x is string => typeof x === "string");
  else if (typeof k.description === "string") {
    const list = k.description.split(/[.:;]/)[0] ?? "";
    ids = list
      .split(",")
      .map((x) => x.trim())
      .filter((x) => /^[a-z][a-z0-9_]*$/.test(x));
  }
  if (ids.length < 2) return BARCODE_KINDS;
  const known = BARCODE_KINDS.filter((b) => ids.includes(b.id));
  const more = ids
    .filter((id) => !BARCODE_KINDS.some((b) => b.id === id))
    .map((id): BarcodeKind => {
      const m = MORE_KIND_LABELS[id];
      const twoD = m?.twoD ?? /qr|matrix|aztec|pdf417|tag|aruco|sensible|maxi|dot/.test(id);
      return m?.ec ? { id, label: m.label, twoD, ec: m.ec } : { id, label: m?.label ?? id, twoD };
    });
  return [...known, ...more];
}

let engineKinds: BarcodeKind[] | null = null;

/** Remember the engine's kinds (from GET /api/commands), for `barcodeKind` and the dialog. */
export function setEngineBarcodeKinds(kinds: BarcodeKind[] | null): void {
  engineKinds = kinds;
}

/** The kinds to offer: the engine's when known, else the built-in list. */
export function barcodeKinds(): BarcodeKind[] {
  return engineKinds ?? BARCODE_KINDS;
}

export function barcodeKind(id: string): BarcodeKind | undefined {
  return barcodeKinds().find((k) => k.id === id) ?? BARCODE_KINDS.find((k) => k.id === id);
}

/** A barcode object's kept parameters (`extras["openetch.barcode"]`), or null for other objects. */
export function barcodeParams(o: DocObject | null | undefined): BarcodeParams | null {
  const raw = o?.extras?.[BARCODE_KEY];
  if (!raw) return null;
  try {
    const p = JSON.parse(raw) as Record<string, unknown>;
    if (!p || typeof p.kind !== "string" || typeof p.data !== "string") return null;
    return p as unknown as BarcodeParams;
  } catch {
    return null;
  }
}

const BARCODE_FIELDS = ["kind", "data", "module_mm", "height_mm", "quiet_zone", "invert", "error_correction", "force_square", "show_text", "variable"] as const;

/** Only the fields shape.barcode takes (it refuses unknown ones), without nulls and Normal mode. */
export function cleanBarcode(p: BarcodeParams): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const k of BARCODE_FIELDS) {
    const v = p[k];
    if (v === null || v === undefined) continue;
    if (k === "variable" && (v as { mode: TextMode }).mode === "normal") continue;
    out[k] = v;
  }
  return out;
}

/** shape.barcode: a new barcode at `center` on `layer`. */
export function barcodeCreate(p: BarcodeParams, center: Vec2, layer: string | null): Invocation {
  const params: Record<string, unknown> = { ...cleanBarcode(p), center: [round4(center[0]), round4(center[1])] };
  if (layer) params.layer = layer;
  return { id: TXT.barcode, params };
}

/**
 * shape.barcode with `id`: redraw barcode `id` in place with `p` (its layer, name and placement stay; leaving `layer`
 * out keeps the object's current layer).
 */
export function barcodeRedraw(id: string, p: BarcodeParams): Invocation {
  return { id: TXT.barcode, params: { ...cleanBarcode(p), id } };
}

// ---- selection helpers ------------------------------------------------------------------------------------------------

/** A text and a vector shape selected together (LightBurn's Apply Path to Text), else null. */
export function applyPathPair(doc: GDocument | null, ids: readonly string[]): { text: string; path: string } | null {
  if (!doc || ids.length !== 2) return null;
  const objs = ids.map((id) => doc.objects.find((o) => o.id === id));
  if (objs.some((o) => !o)) return null;
  const t = objs.find(isText);
  const p = objs.find((o) => o && o !== t && o.geometry.kind !== "image" && o.geometry.kind !== "text");
  return t && p ? { text: t.id, path: p.id } : null;
}

// ---- on-canvas editor placement -------------------------------------------------------------------------------------

/**
 * Where the editor box goes relative to the text's alignment point (the object's local origin), as CSS translate
 * percentages: Align X left puts the box's left edge there, middle centres it, right its right edge; Align Y top puts
 * its top there, middle centres it, bottom its bottom.
 */
export function editorTranslate(h: TextAlignH, v: TextAlignV): [number, number] {
  return [h === "left" ? 0 : h === "middle" ? -50 : -100, v === "top" ? 0 : v === "middle" ? -50 : -100];
}

/** CSS text-align for Align X. */
export const cssTextAlign = (h: TextAlignH): "left" | "center" | "right" => (h === "left" ? "left" : h === "middle" ? "center" : "right");

/** Field point of an object's local origin (a text's alignment point). */
export function originOf(o: DocObject): Vec2 {
  return [o.transform[4], o.transform[5]];
}

/** Rotation of an object's x axis in degrees, counter-clockwise (field frame, +Y up). */
export function rotationOf(o: DocObject): number {
  const [a, b] = o.transform;
  return (Math.atan2(b, a) * 180) / Math.PI;
}

/** Uniform scale of an object's transform (a scaled text shows its em that much bigger). */
export function scaleOf(o: DocObject): number {
  const [a, b, c, d] = o.transform;
  return Math.sqrt(Math.abs(a * d - b * c)) || 1;
}

/** Lines of an editor value with Windows line ends folded, so the engine gets `\n`. */
export function normalizeText(s: string): string {
  return s.replace(/\r\n?/g, "\n");
}

/** Whether a text value has something to show (the engine refuses blank text). */
export function hasInk(s: string): boolean {
  return s.trim().length > 0;
}

function round4(v: number): number {
  return Math.round(v * 1e4) / 1e4;
}
