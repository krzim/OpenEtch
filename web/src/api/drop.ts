// Pure routing for files dropped on the window or picked with Import: which file goes to which engine import, which
// are refused and why, the drop overlay's label, the enter/leave counter, and the summary toast. No DOM and no fetch,
// so `npm test` covers it. The browser side (window listeners, reading files) is in components/FileDrop.tsx.
//
// Rules (LightBurn: drag files from the system file browser into the workspace; Open replaces, Import adds):
// - .svg, .dxf and bitmaps are imported into the current document (merged) on the active layer;
// - a .lbrn2 goes through the LightBurn dialog (Open = replace, Import = merge; a drop defaults to merge);
// - a .openetch opens (replaces) the document, behind the unsaved-changes guard, and only when dropped on its own;
// - everything else is refused with a reason, never silently.

/** Commands the drop router may use. */
export const IMPORT_COMMANDS = {
  svg: "art.import_svg",
  dxf: "art.import_dxf",
  image: "art.import_image",
} as const;

/** Bitmap extensions and the MIME type sent with them. */
export const IMAGE_TYPES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  bmp: "image/bmp",
  gif: "image/gif",
  webp: "image/webp",
};

/** Formats LightBurn imports that OpenEtch has no importer for yet. */
const NOT_YET: Record<string, string> = {
  ai: "Adobe Illustrator",
  pdf: "PDF",
  eps: "EPS",
  plt: "HPGL",
  hpgl: "HPGL",
  gc: "G-code",
  gcode: "G-code",
  nc: "G-code",
  tif: "TIFF",
  tiff: "TIFF",
  clb: "LightBurn library",
};

export type FileKind = "svg" | "dxf" | "image" | "lbrn2" | "openetch" | "lbrn-old" | "not-yet" | "unknown";

/** The extension of a file name, lower case, without the dot ("" when there is none). */
export function fileExt(name: string): string {
  const base = name.split(/[\\/]/).pop() ?? name;
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : "";
}

/** What kind of file `name` is, by extension (case-insensitive). */
export function classifyFile(name: string): FileKind {
  const ext = fileExt(name);
  if (ext === "svg") return "svg";
  if (ext === "dxf") return "dxf";
  if (ext in IMAGE_TYPES) return "image";
  if (ext === "lbrn2") return "lbrn2";
  if (ext === "openetch") return "openetch";
  if (ext === "lbrn" || ext === "lbt") return "lbrn-old";
  if (ext in NOT_YET) return "not-yet";
  return "unknown";
}

/** One direct import (no dialog): SVG, DXF or a bitmap. */
export interface DirectImport<F> {
  file: F;
  kind: "svg" | "dxf" | "image";
  command: string;
  /** For bitmaps. */
  mime?: string;
}

export interface DropPlan<F> {
  /** Imported straight away, in drop order. */
  imports: DirectImport<F>[];
  /** LightBurn projects, for the Open/Import dialog (merged when more than one file was dropped). */
  lbrn2: F[];
  /** A project to open (replace); only when it was the only file. */
  open: F | null;
  /** Files not imported, with the reason shown to the user. */
  refused: { name: string; reason: string }[];
}

export interface RouteOptions {
  /** Larger files are refused before they are read. */
  maxBytes: number;
  /** Whether the engine has a command id (from GET /api/commands); missing importers are refused with a message. */
  has: (command: string) => boolean;
  /** When set, every file is refused with this reason (a dialog is open, or a file task is running). */
  blocked?: string | null;
}

function mb(n: number): string {
  return `${Math.round(n / (1024 * 1024))} MB`;
}

/** Route dropped or picked files to their import paths. */
export function routeDroppedFiles<F extends { name: string; size: number }>(files: readonly F[], opts: RouteOptions): DropPlan<F> {
  const plan: DropPlan<F> = { imports: [], lbrn2: [], open: null, refused: [] };
  if (opts.blocked) {
    for (const f of files) plan.refused.push({ name: f.name, reason: opts.blocked });
    return plan;
  }
  const several = files.length > 1;
  for (const f of files) {
    const kind = classifyFile(f.name);
    const ext = fileExt(f.name);
    const refuse = (reason: string) => plan.refused.push({ name: f.name, reason });
    if (kind === "unknown") {
      refuse(ext ? `.${ext} files cannot be imported` : "not a file type OpenEtch can import");
      continue;
    }
    if (kind === "lbrn-old") {
      refuse("LightBurn .lbrn/.lbt files are not supported; save as .lbrn2");
      continue;
    }
    if (kind === "not-yet") {
      refuse(`${NOT_YET[ext]} (.${ext}) import is not supported yet`);
      continue;
    }
    if (f.size > opts.maxBytes) {
      refuse(`larger than ${mb(opts.maxBytes)}`);
      continue;
    }
    if (f.size === 0) {
      refuse("the file is empty");
      continue;
    }
    if (kind === "openetch") {
      if (several) refuse("drop a project file on its own to open it");
      else plan.open = f;
      continue;
    }
    if (kind === "lbrn2") {
      plan.lbrn2.push(f);
      continue;
    }
    const command = IMPORT_COMMANDS[kind];
    if (!opts.has(command)) {
      refuse(`${kind === "image" ? "image" : kind.toUpperCase()} import is not supported by this engine yet`);
      continue;
    }
    plan.imports.push(kind === "image" ? { file: f, kind, command, mime: IMAGE_TYPES[ext] } : { file: f, kind, command });
  }
  return plan;
}

/** Nothing in the plan will be imported. */
export function planIsEmpty(p: DropPlan<unknown>): boolean {
  return p.imports.length === 0 && p.lbrn2.length === 0 && p.open === null;
}

/**
 * Overlay text while files are dragged over the window. Browsers do not reveal file names before the drop, only the
 * number of items and (sometimes) their MIME types, so a single item that is not a known graphic may be a project.
 */
export function overlayLabel(items: readonly { type: string }[]): string {
  if (items.length > 1) return `Drop to import ${items.length} files`;
  const t = items[0]?.type ?? "";
  if (t.startsWith("image/") || t === "image/svg+xml" || /dxf/i.test(t)) return "Drop to import";
  return "Drop to open or import";
}

/** Enter/leave counter so the overlay does not flicker while the pointer crosses child elements. */
export function dragDepth(depth: number, event: "enter" | "leave" | "drop"): number {
  if (event === "enter") return depth + 1;
  if (event === "leave") return Math.max(0, depth - 1);
  return 0;
}

/** Whether a drag carries files (and so must never reach the browser's default "open the file" action). */
export function carriesFiles(types: readonly string[] | null | undefined): boolean {
  return !!types && Array.from(types).includes("Files");
}

/** The engine's `created` ids in an import result (several shapes may come from one file). */
export function createdIds(result: unknown): string[] {
  if (!result || typeof result !== "object") return [];
  const c = (result as { created?: unknown }).created;
  return Array.isArray(c) ? c.filter((x): x is string => typeof x === "string") : [];
}

/**
 * Largest file sent inline with `/api/execute` (the engine takes JSON bodies up to 32 MB; base64 makes bitmaps a third
 * larger). Larger files go up raw through `POST /api/document/upload?format=svg|dxf|image`.
 */
export function inlineLimit(kind: "svg" | "dxf" | "image"): number {
  return (kind === "image" ? 16 : 24) * 1024 * 1024;
}

/** Base64 of raw bytes, in chunks so large images do not overflow the argument limit of `String.fromCharCode`. */
export function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin);
}

/** File name without its extension, for object names. */
export function importName(name: string): string {
  const base = name.split(/[\\/]/).pop() ?? name;
  const dot = base.lastIndexOf(".");
  return (dot > 0 ? base.slice(0, dot) : base) || "Imported";
}

/** One-line result of a drop or Import: how many files and objects came in, and what was refused. */
export function dropSummary(s: { imported: string[]; objects: number; refused: { name: string; reason: string }[]; pending?: number }): {
  msg: string;
  kind: "info" | "error";
} {
  const parts: string[] = [];
  if (s.imported.length === 1) parts.push(`Imported ${s.imported[0]}${s.objects > 1 ? ` (${s.objects} objects)` : ""}.`);
  else if (s.imported.length > 1) parts.push(`Imported ${s.imported.length} files (${s.objects} object${s.objects === 1 ? "" : "s"}).`);
  if (s.pending) parts.push(`${s.pending} LightBurn file${s.pending === 1 ? "" : "s"} waiting in the import dialog.`);
  if (s.refused.length === 1) parts.push(`${s.refused[0].name}: ${s.refused[0].reason}.`);
  else if (s.refused.length > 1) parts.push(`Not imported: ${s.refused.map((r) => `${r.name} (${r.reason})`).join("; ")}.`);
  if (parts.length === 0) return { msg: "Nothing imported.", kind: "error" };
  const failed = s.imported.length === 0 && !s.pending;
  return { msg: parts.join(" "), kind: failed ? "error" : "info" };
}

/** `accept` for the Import picker: everything the router handles. */
export const IMPORT_ACCEPT = [".svg", ".dxf", ...Object.keys(IMAGE_TYPES).map((e) => `.${e}`), ".lbrn2", ".openetch"].join(",");
