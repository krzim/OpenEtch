// Pure logic for the File menu: project names, file formats and names, the engine's import/export answers and their
// warnings, the project list, and whether the project has unsaved changes. No React, no fetch, so `npm test` covers it.

import { fieldSourceText, parseFieldSource, type FieldSource } from "./lightburn.ts";
import type { GDocument, ProjectEntry, ProjectInfo } from "./types.ts";

/** File formats the engine reads and writes: the native project and LightBurn's. */
export type FileFormat = "openetch" | "lbrn2";

/** Formats `POST /api/document/upload` takes: the two project formats plus large artwork (always merged). */
export type UploadFormat = FileFormat | "svg" | "dxf" | "image";

/** How `doc.import_lbrn2` treats the current document. */
export type ImportMode = "replace" | "merge";

/** The engine's limit for `POST /api/document/upload`. */
export const MAX_UPLOAD_BYTES = 50 * 1024 * 1024;

/** Longest project name, in characters. */
export const MAX_PROJECT_NAME = 100;

const NAME_CHARS = /^[\p{L}\p{N} _.()-]+$/u;

/**
 * Check a project name against the engine's rule: 1-100 letters, digits, spaces and `- _ . ( )`, no leading dot, no
 * "..". Returns the problem as a sentence, or null when the name is fine. Callers trim first.
 */
export function validateProjectName(name: string): string | null {
  const n = [...name].length;
  if (n === 0) return "Enter a name.";
  if (n > MAX_PROJECT_NAME) return `Use at most ${MAX_PROJECT_NAME} characters.`;
  if (!NAME_CHARS.test(name)) return "Use only letters, digits, spaces and - _ . ( )";
  if (name.startsWith(".")) return "The name cannot start with a dot.";
  if (name.includes("..")) return 'The name cannot contain "..".';
  return null;
}

/** A project name folded for comparison: Windows and macOS file names ignore case, so "Tags" and "TAGS" clash. */
function nameKey(name: string): string {
  return name.normalize("NFC").toLowerCase();
}

/**
 * The existing project that saving as `name` would replace, compared case-insensitively, or null. The current
 * project under its own exact name is not a clash (Save overwrites it on purpose).
 */
export function projectClash(list: readonly ProjectEntry[] | null | undefined, name: string, current: string | null): string | null {
  if (!list || !name) return null;
  const key = nameKey(name);
  const hit = list.find((p) => nameKey(p.name) === key && !(p.name === name && name === current));
  return hit ? hit.name : null;
}

/**
 * The message for a failed upload or download of `what` (a file name): the engine's own `error` text (e.g. "field_mm
 * 'x' is not a number" for a 400, "the file is larger than 50 MB" for a 413) rather than the method/path/status
 * prefix of the HTTP error. Anything else is returned unchanged.
 */
export function fileError(e: unknown, what: string): unknown {
  if (e instanceof Error) {
    const detail = (e as { detail?: unknown }).detail;
    if (typeof detail === "string" && detail) return new Error(`${what}: ${detail}`);
  }
  return e;
}

/** Whether `e` is the engine's 409 answer to saving over an existing project without `overwrite`. */
export function isSaveConflict(e: unknown): boolean {
  return e instanceof Error && (e as { status?: unknown }).status === 409;
}

/** Turn a file name or document name into a valid project name, or "" when nothing usable is left. */
export function suggestProjectName(raw: string): string {
  let s = raw.replace(/[^\p{L}\p{N} _.()-]+/gu, " ");
  s = s.replace(/\.{2,}/g, ".").replace(/\s+/g, " ").trim();
  s = s.replace(/^[.\s]+/, "");
  s = [...s].slice(0, MAX_PROJECT_NAME).join("").trim();
  return validateProjectName(s) === null ? s : "";
}

/** The format of a picked file, from its extension, or null for anything else. */
export function fileFormat(filename: string): FileFormat | null {
  const f = filename.toLowerCase();
  if (f.endsWith(".openetch")) return "openetch";
  if (f.endsWith(".lbrn2")) return "lbrn2";
  return null;
}

/** File name without its directory and last extension. */
export function fileStem(filename: string): string {
  const base = filename.split(/[\\/]/).pop() ?? filename;
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(0, dot) : base;
}

/** The file name in a Content-Disposition header (`filename*=UTF-8''...` preferred), or null. */
export function contentDispositionName(header: string | null | undefined): string | null {
  if (!header) return null;
  const star = /filename\*\s*=\s*([^']*)'[^']*'([^;]+)/i.exec(header);
  if (star) {
    try {
      const v = decodeURIComponent(star[2].trim().replace(/^"|"$/g, ""));
      if (v) return v;
    } catch {
      // Malformed percent-encoding: fall back to the plain parameter.
    }
  }
  const quoted = /filename\s*=\s*"((?:[^"\\]|\\.)*)"/i.exec(header);
  if (quoted) return quoted[1].replace(/\\(.)/g, "$1") || null;
  const bare = /filename\s*=\s*([^;\s]+)/i.exec(header);
  return bare ? bare[1] : null;
}

/** A safe download file name: `base` without characters file systems refuse, plus the format's extension. */
export function downloadName(base: string | null | undefined, format: FileFormat): string {
  const clean = (base ?? "").replace(/[\\/:*?"<>|\u0000-\u001f]+/g, "_").trim().replace(/^\.+/, "");
  return `${clean || "Untitled"}.${format}`;
}

/**
 * Unwrap an engine command answer: `{ok: true, result}` gives `result`, a bare result is returned as is, and
 * `{ok: false, error}` throws an Error with the engine's message.
 */
export function commandResult(res: unknown): unknown {
  if (res && typeof res === "object" && !Array.isArray(res)) {
    const o = res as { ok?: unknown; error?: unknown; result?: unknown };
    if (o.ok === false) throw new Error(typeof o.error === "string" && o.error ? o.error : "the engine refused the file");
    if (o.ok === true && "result" in o) return o.result;
  }
  return res;
}

/** The `warnings` strings of an import or export result. */
export function warningList(result: unknown): string[] {
  if (!result || typeof result !== "object") return [];
  const w = (result as { warnings?: unknown }).warnings;
  return Array.isArray(w) ? w.filter((x): x is string => typeof x === "string" && x.length > 0) : [];
}

/** What a `doc.import_lbrn2` (or `doc.load`) result reports, for the toast and the warnings panel. */
export interface ImportOutcome {
  objects: number;
  createdLayers: number;
  reusedLayers: number;
  warnings: string[];
  fieldMm: number | null;
  /** Where the engine took the field size from (`field_source`), when it says. */
  fieldSource: FieldSource | null;
}

export function importOutcome(result: unknown): ImportOutcome {
  const r = result && typeof result === "object" ? (result as Record<string, unknown>) : {};
  const count = (v: unknown) => (Array.isArray(v) ? v.length : 0);
  return {
    objects: count(r.created),
    createdLayers: count(r.created_layers ?? r.layers),
    reusedLayers: count(r.reused_layers),
    warnings: warningList(r),
    fieldMm: typeof r.field_mm === "number" && Number.isFinite(r.field_mm) ? r.field_mm : null,
    fieldSource: parseFieldSource(r.field_source),
  };
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

/** One-line toast for a finished LightBurn import. */
export function importMessage(o: ImportOutcome, mode: ImportMode): string {
  const layers = o.reusedLayers > 0 ? `${plural(o.createdLayers, "new layer")}, ${o.reusedLayers} reused` : plural(o.createdLayers, "layer");
  // The source only when it says something the user may not expect (a LightBurn device); "the size you entered" is noise.
  const src = o.fieldSource && (o.fieldSource.kind === "file-device" || o.fieldSource.kind === "default-device") ? ` ${fieldSourceText(o.fieldSource)}` : "";
  const field = o.fieldMm !== null ? ` (field ${Number(o.fieldMm.toFixed(3))} mm${src})` : "";
  const what = mode === "merge" ? "Merged" : "Imported";
  const warn = o.warnings.length ? ` ${plural(o.warnings.length, "warning")}.` : "";
  return `${what} ${plural(o.objects, "object")} on ${layers}${field}.${warn}`;
}

/** Identical warnings folded into one row with a count, in first-seen order. */
export function groupWarnings(list: string[]): { text: string; count: number }[] {
  const out: { text: string; count: number }[] = [];
  const at = new Map<string, number>();
  for (const w of list) {
    const i = at.get(w);
    if (i === undefined) {
      at.set(w, out.length);
      out.push({ text: w, count: 1 });
    } else {
      out[i].count += 1;
    }
  }
  return out;
}

/** GET /api/projects as a list (an array, or `{projects: [...]}`), valid rows only, newest first. */
export function projectList(res: unknown): ProjectEntry[] {
  const raw = Array.isArray(res) ? res : res && typeof res === "object" ? (res as { projects?: unknown }).projects : null;
  if (!Array.isArray(raw)) return [];
  const rows = raw.filter((p): p is ProjectEntry => !!p && typeof p === "object" && typeof (p as { name?: unknown }).name === "string");
  const t = (p: ProjectEntry) => (typeof p.modified_unix_s === "number" ? p.modified_unix_s : 0);
  return rows
    .map((p, i) => ({ p, i }))
    .sort((a, b) => t(b.p) - t(a.p) || a.p.name.localeCompare(b.p.name) || a.i - b.i)
    .map((x) => x.p);
}

/**
 * Whether the document has unsaved changes. Uses the engine's `dirty`, except when the document has moved on since
 * that answer (an edit raced the refetch): then it compares the document revision with `saved_revision`. Null when the
 * engine reports no project state at all.
 */
export function projectDirty(project: ProjectInfo | null, docRevision: number | null): boolean | null {
  if (!project) return null;
  if (docRevision !== null && typeof project.revision === "number" && docRevision !== project.revision) {
    return project.saved_revision === null || project.saved_revision === undefined || project.saved_revision !== docRevision;
  }
  return !!project.dirty;
}

/**
 * The project the document is saved as, for the top bar and Save. `detachedFrom` is the project name the engine
 * reported when this UI replaced the document (New, upload, LightBurn import): while the engine still reports that
 * name, the document is not that project any more, so Save must ask for a name instead of overwriting it.
 */
export function currentProjectName(project: ProjectInfo | null, detachedFrom: string | null | undefined): string | null {
  const name = project?.name ?? null;
  if (detachedFrom !== undefined && name === detachedFrom) return null;
  return name;
}

/** Ask before replacing the document: when it has unsaved changes, or (engine without project state) any content. */
export function needsConfirm(dirty: boolean | null, doc: GDocument | null): boolean {
  if (dirty !== null) return dirty;
  return !!doc && (doc.objects.length > 0 || doc.stock !== null);
}

/** File size for the project list. */
export function formatBytes(n: number | undefined): string {
  if (typeof n !== "number" || !Number.isFinite(n) || n < 0) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** Modification time for the project list: relative when recent, else the local date. */
export function formatModified(unixS: number | undefined, nowMs: number = Date.now()): string {
  if (typeof unixS !== "number" || !Number.isFinite(unixS) || unixS <= 0) return "";
  const ago = Math.max(0, nowMs / 1000 - unixS);
  if (ago < 60) return "just now";
  if (ago < 3600) return `${Math.floor(ago / 60)} min ago`;
  if (ago < 86400) return `${Math.floor(ago / 3600)} h ago`;
  return new Date(unixS * 1000).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/** A usable field size, or null. */
export function validField(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) && v > 0 ? v : null;
}

/**
 * Field sizes offered as one-tap choices in the LightBurn import and export dialogs: the size the LightBurn file was
 * opened at (export only), the document's, the machine's, 110, 150.
 */
export function fieldChoices(docField: number | null, machineField: number | null, importedField: number | null = null): number[] {
  const out: number[] = [];
  for (const v of [importedField, docField, machineField, 110, 150]) {
    if (v !== null && !out.some((x) => Math.abs(x - v) < 1e-9)) out.push(v);
  }
  return out;
}

/**
 * Field size Export to LightBurn starts with: the size the LightBurn file was opened at (so an open and export round
 * trip puts everything back where LightBurn had it), else the document's, else the machine's, else `fallback`.
 * LightBurn files do not record their field size, and a different size shifts everything by half the difference.
 */
export function exportFieldDefault(importedField: number | null, docField: number | null, machineField: number | null, fallback: number): number {
  return validField(importedField) ?? validField(docField) ?? validField(machineField) ?? fallback;
}

/** Where Export to LightBurn's starting field size comes from (same order as `exportFieldDefault`). */
export type ExportFieldOrigin = "opened" | "document" | "machine" | "default";

export function exportFieldOrigin(importedField: number | null, docField: number | null, machineField: number | null): ExportFieldOrigin {
  if (validField(importedField) !== null) return "opened";
  if (validField(docField) !== null) return "document";
  if (validField(machineField) !== null) return "machine";
  return "default";
}

/** Plain words for an `ExportFieldOrigin`, to follow "Field size 153.8 mm — ". */
export function exportFieldOriginText(o: ExportFieldOrigin): string {
  switch (o) {
    case "opened":
      return "the size this LightBurn file was opened with";
    case "document":
      return "the field size of your current design";
    case "machine":
      return "the field size of this laser";
    case "default":
      return "the usual field size";
  }
}

/**
 * The name exports and downloads are given: the saved project's, else the name of the file this page opened (Open
 * replaces the design, so "Export to LightBurn" after opening "Tags.lbrn2" gives "Tags.lbrn2"), else the document's.
 */
export function exportBaseName(projectName: string | null, openedName: string | null, docName: string | null | undefined): string | null {
  return projectName ?? (openedName && openedName.trim() ? openedName : null) ?? docName ?? null;
}

/** Document `extras` key where the engine remembers the field size of the last merging LightBurn import. */
export const LBRN_IMPORT_FIELD_KEY = "lbrn2._field_mm";

/**
 * The field size the engine's `doc.export_lbrn2` uses by default for `doc`: the one the last merging LightBurn import
 * was read with (kept in the document's extras, so it survives reloads and save/open), else null. A replacing import
 * sets the document's own `field_mm` instead.
 */
export function rememberedImportField(doc: Pick<GDocument, "extras"> | null | undefined): number | null {
  const raw = doc?.extras?.[LBRN_IMPORT_FIELD_KEY];
  return typeof raw === "string" && raw.trim() !== "" && Number(raw) <= 2000 ? validField(Number(raw)) : null;
}

/**
 * The LightBurn field size `doc`'s art was read at, as Export to LightBurn offers it first (the engine's own default):
 * a merged file's (remembered in the document), else `openedAt`, the size this page opened a LightBurn file at. A
 * replacing import sets the document's `field_mm` to that size, so `openedAt` only counts while the two still agree:
 * once the document was replaced or changed elsewhere (another tab, an agent) the remembered size is stale.
 */
export function importFieldOf(doc: Pick<GDocument, "extras" | "field_mm"> | null | undefined, openedAt: number | null): number | null {
  return rememberedImportField(doc) ?? (sameField(validField(openedAt), validField(doc?.field_mm)) ? openedAt : null);
}

/** Whether two field sizes are the same to the micrometre. */
export function sameField(a: number | null, b: number | null): boolean {
  return a !== null && b !== null && Math.abs(a - b) < 1e-3;
}
