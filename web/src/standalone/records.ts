// What the worker shell keeps in the KeyValueStore (idb.ts), and the context its route modules share.
//
// Large things are split in two stores, a listing (small records, read whole for a list) and the bytes (read one at
// a time), written together in one batch: a project's row and its file never disagree.

import type { CoreReply } from "./contract.ts";
import type { KeyValueStore } from "./idb.ts";

export const S = {
  /** ProjectMeta by project path. */
  projects: "projects",
  projectData: "project-data",
  /** Explicitly created (maybe empty) project folders, by path. */
  folders: "folders",
  /** StoredMeta by file name `<flat name>.<unix>.openetch`. */
  autosaves: "autosaves",
  autosaveData: "autosave-data",
  backups: "backups",
  backupData: "backup-data",
  /** TemplateMeta by `<name>.<kind>`. */
  templates: "templates",
  templateData: "template-data",
  /** FontMeta by file name. */
  fonts: "fonts",
  fontData: "font-data",
  /** ThumbRecord by `<project path>|<width>`. */
  thumbs: "thumbs",
  /** "files": FileSettings. */
  settings: "settings",
  /** RecoveryMeta by session id (one tab). */
  recovery: "recovery",
  recoveryData: "recovery-data",
  /** File System Access handles (fsaccess.ts), by document key. */
  handles: "handles",
} as const;

/** webcore's own stores (prefs, devices, libraries, ...) are kept under this prefix. */
export const CORE_PREFIX = "core.";

/** Stores whose changes other tabs hear about (BroadcastChannel). */
export const SHARED_STORES: string[] = [S.projects, S.folders, S.autosaves, S.backups, S.templates, S.fonts, S.settings];

export interface ProjectMeta {
  name: string;
  folder: string;
  /** Last save, Unix ms; strictly increasing per project (the overwrite check compares it). */
  modified_ms: number;
  size: number;
}

/** An autosave or backup copy. */
export interface StoredMeta {
  file: string;
  /** The project's flat name (the file's stem). */
  project: string;
  saved_unix_s: number;
  size: number;
}

export interface TemplateMeta {
  name: string;
  kind: "openetch" | "lbt";
  modified_ms: number;
  size: number;
}

export interface FontMeta {
  /** The file name it was added as (the key). */
  name: string;
  /** The family webcore registered it as. */
  family: string;
  size: number;
  added_ms: number;
}

export interface ThumbRecord {
  /** The project's modified_ms the picture was made from. */
  modified_ms: number;
  /** The picture (PNG, or SVG when the core is built without resvg). */
  png: Uint8Array;
  /** Its media type; absent: image/png. */
  type?: string;
}

/** Autosave and backup settings (crates/engine/src/userfiles.rs `FileSettings`). */
export interface FileSettings {
  autosave_minutes: number;
  autosave_keep: number;
  backups_keep: number;
}

export const DEFAULT_SETTINGS: FileSettings = { autosave_minutes: 5, autosave_keep: 10, backups_keep: 5 };

/** A tab's recovery copy: the active document as it was, so a reload or a crash does not lose it. */
export interface RecoveryMeta {
  saved_ms: number;
  /** The project the document belongs to, if any. */
  project: string | null;
  /** Its display name while it is no project (an opened file's name). */
  display: string | null;
  /** True: the document was the saved project unchanged (no bytes kept; the project is reopened). */
  clean: boolean;
  /** The project's modified_ms this tab knew (the overwrite check after recovery). */
  known_ms: number | null;
}

/** Everything the route modules share (backend.ts builds it). */
export interface ShellContext {
  store: KeyValueStore;
  /** Call webcore: its store writes are persisted and its events posted before this resolves. */
  core(method: string, path: string, query?: string, body?: Uint8Array): Promise<CoreReply>;
  /** Unix ms. */
  now(): number;
  sessionId: string;
  /** Tell other tabs these shared stores changed. */
  changed(stores: string[]): void;
  /** Post an event (JSON text) to this tab's page. */
  emit(event: string): void;
  /** What this tab last opened or saved per document id: the project and its modified_ms then. */
  known: Map<string, { name: string; modified_ms: number }>;
}

/** The newest stamp for a record replacing one stamped `prev`: now, but never at or before `prev`. */
export function nextStamp(now: number, prev: number | undefined): number {
  return prev === undefined ? now : Math.max(now, prev + 1);
}
