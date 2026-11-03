// Saving to and opening real files in the design-only build (page side). Chromium on a desktop has the File System
// Access API: Save As picks a file once (`showSaveFilePicker`), and Save writes to that file again, its handle kept
// in IndexedDB so it survives a reload (the browser asks for permission again after a reload; that needs the click
// of Save). Everywhere else (Firefox, Safari on the Mac, iPad and iPhone) a save is a download (`<a download>`) and
// opening is `<input type=file>`.
//
// iOS and iPadOS Safari grey out files whose extension the `accept` list names but the system does not know
// (`.openetch`, `.lbrn2`), so there the picker takes any file and the extension is checked after picking.

import type { KeyValueStore } from "./idb.ts";
import { S } from "./records.ts";

/** A file type for the pickers. */
export interface FileKind {
  description: string;
  /** With the dot: [".openetch"]. */
  extensions: string[];
  mime: string;
}

export const OPENETCH_FILE: FileKind = { description: "OpenEtch project", extensions: [".openetch"], mime: "application/json" };
export const LBRN2_FILE: FileKind = { description: "LightBurn project", extensions: [".lbrn2"], mime: "application/xml" };

export type SaveOutcome = "saved" | "downloaded" | "cancelled";

/** The API's parts used here (lib.dom does not declare them all yet). */
interface Writable {
  write(data: Blob | BufferSource): Promise<void>;
  close(): Promise<void>;
}
interface FileHandle {
  kind: "file";
  name: string;
  createWritable(): Promise<Writable>;
  getFile(): Promise<File>;
  queryPermission?(o: { mode: "readwrite" }): Promise<PermissionState>;
  requestPermission?(o: { mode: "readwrite" }): Promise<PermissionState>;
}
interface PickerWindow {
  showSaveFilePicker?(o: { suggestedName?: string; types?: { description: string; accept: Record<string, string[]> }[] }): Promise<FileHandle>;
  showOpenFilePicker?(o: { multiple?: boolean; types?: { description: string; accept: Record<string, string[]> }[] }): Promise<FileHandle[]>;
}

function win(): PickerWindow & typeof globalThis {
  return globalThis as PickerWindow & typeof globalThis;
}

/** iPhone, iPad (also iPadOS asking for the desktop site) and iPod. */
export function isAppleTouch(nav: { userAgent: string; maxTouchPoints?: number; platform?: string } | undefined = globalThis.navigator): boolean {
  if (!nav) return false;
  if (/iPhone|iPad|iPod/.test(nav.userAgent)) return true;
  return /Macintosh/.test(nav.userAgent) && (nav.maxTouchPoints ?? 0) > 1;
}

/** Whether Save can write to a file the person picked (Chromium desktop). */
export function canPickFiles(): boolean {
  return typeof win().showSaveFilePicker === "function" && !isAppleTouch();
}

function types(kinds: FileKind[]) {
  return kinds.map((k) => ({ description: k.description, accept: { [k.mime]: k.extensions } }));
}

function isAbort(e: unknown): boolean {
  return (e as { name?: unknown } | null)?.name === "AbortError";
}

/** A download of `blob` as `filename` (a temporary link; Safari on iOS shows its download sheet). */
export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.rel = "noopener";
  a.style.display = "none";
  document.body.appendChild(a);
  a.click();
  a.remove();
  // Safari reads the blob after click() returns: revoke later.
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/**
 * Save `blob`. With `key` (the document's or project's), Save reuses the file picked last time for it; `saveAs`
 * always asks for a file. Without the File System Access API it downloads. Call it from a click.
 */
export async function saveFile(
  blob: Blob,
  suggestedName: string,
  opts: { kind?: FileKind; key?: string; saveAs?: boolean; store?: KeyValueStore | null } = {},
): Promise<SaveOutcome> {
  const w = win();
  if (!canPickFiles() || !w.showSaveFilePicker) {
    downloadBlob(blob, suggestedName);
    return "downloaded";
  }
  const store = opts.store ?? null;
  let handle: FileHandle | null = null;
  if (!opts.saveAs && opts.key && store) {
    handle = (await store.get<FileHandle>(S.handles, opts.key).catch(() => undefined)) ?? null;
    if (handle && !(await writable(handle))) handle = null;
  }
  if (!handle) {
    try {
      handle = await w.showSaveFilePicker({ suggestedName, types: opts.kind ? types([opts.kind]) : undefined });
    } catch (e) {
      if (isAbort(e)) return "cancelled";
      // A picker refused here (a sandboxed frame): fall back to a download.
      downloadBlob(blob, suggestedName);
      return "downloaded";
    }
  }
  const out = await handle.createWritable();
  await out.write(blob);
  await out.close();
  if (opts.key && store) await store.batch([{ store: S.handles, key: opts.key, value: handle }]).catch(() => undefined);
  return "saved";
}

/** Whether we may write to `h` now (asks again after a reload; needs a click). */
async function writable(h: FileHandle): Promise<boolean> {
  try {
    const opts = { mode: "readwrite" as const };
    if (!h.queryPermission) return true;
    if ((await h.queryPermission(opts)) === "granted") return true;
    return (await h.requestPermission?.(opts)) === "granted";
  } catch {
    return false;
  }
}

/** Forget the file Save writes to for `key` (Save As to a new file, a closed document). */
export async function forgetFile(store: KeyValueStore, key: string): Promise<void> {
  await store.batch([{ store: S.handles, key, value: null }]).catch(() => undefined);
}

/** Whether `name` ends with one of `kinds`' extensions (any case). */
export function hasExtension(name: string, kinds: FileKind[]): boolean {
  const n = name.toLowerCase();
  return kinds.some((k) => k.extensions.some((e) => n.endsWith(e.toLowerCase())));
}

/**
 * Ask for files to open. Resolves to the files picked (empty when cancelled). On iOS/iPadOS every file is offered
 * and the caller checks the extension (`hasExtension`). Call it from a click.
 */
export async function openFiles(kinds: FileKind[], opts: { multiple?: boolean } = {}): Promise<File[]> {
  const w = win();
  if (w.showOpenFilePicker && !isAppleTouch()) {
    try {
      const hs = await w.showOpenFilePicker({ multiple: !!opts.multiple, types: kinds.length ? types(kinds) : undefined });
      return Promise.all(hs.map((h) => h.getFile()));
    } catch (e) {
      if (isAbort(e)) return [];
      // Fall through to the input.
    }
  }
  return pickWithInput(kinds, !!opts.multiple);
}

function pickWithInput(kinds: FileKind[], multiple: boolean): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = multiple;
    if (!isAppleTouch() && kinds.length) input.accept = kinds.flatMap((k) => [...k.extensions, k.mime]).join(",");
    input.style.display = "none";
    let done = false;
    const finish = (files: File[]) => {
      if (done) return;
      done = true;
      input.remove();
      window.removeEventListener("focus", onFocus);
      resolve(files);
    };
    input.addEventListener("change", () => finish(input.files ? [...input.files] : []));
    // Chrome 113+, Safari 16.4+, Firefox 91+.
    input.addEventListener("cancel", () => finish([]));
    // Older browsers: the window gets focus back when the picker closes; `change` comes first when files were picked.
    const onFocus = () => setTimeout(() => finish(input.files ? [...input.files] : []), 1000);
    window.addEventListener("focus", onFocus, { once: true });
    document.body.appendChild(input);
    input.click();
  });
}
