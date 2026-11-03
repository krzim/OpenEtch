// Autosave with rotation, backups on save and the tab's recovery copy, as the engine keeps them on disk
// (crates/engine/src/userfiles.rs): an autosave of the active document every `autosave_minutes` while it has
// unsaved changes, the newest `autosave_keep` kept per project; the file a save replaces copied to the backups first
// (newest `backups_keep` kept). Files are named `<flat project name>.<unix seconds>.openetch`.
//
// The recovery copy is the design-only build's own: the page's document lives in this worker, so a reload or a wasm
// crash would lose it. A few seconds after each change the active document is written to the tab's recovery record
// (keyed by the session id the page keeps in sessionStorage); a reload or a restarted worker reads it back.

import type { Recovered } from "./contract.ts";
import { displayName, flatName, PROJECT_EXT, projectNameProblem, splitStamped, sameNameIgnoringCase, nameProblem } from "./names.ts";
import { DEFAULT_SETTINGS, S, type FileSettings, type ProjectMeta, type RecoveryMeta, type ShellContext, type StoredMeta } from "./records.ts";
import { HttpError, replyJson, text } from "./reply.ts";
import type { CoreReply } from "./contract.ts";

/** webcore's error answer as an HttpError (its `error` and `code`). */
export function coreError(r: CoreReply): HttpError {
  const j = replyJson(r);
  const msg = typeof j?.error === "string" ? j.error : text(r.body) || `status ${r.status}`;
  return new HttpError(r.status, msg, typeof j?.code === "string" ? j.code : undefined);
}

/** The active document now: its project status, its id and its `.openetch` bytes, read together. */
export interface Snapshot {
  name: string | null;
  displayName: string | null;
  revision: number;
  dirty: boolean;
  documentId: string;
  content: Uint8Array;
}

export async function projectStatus(ctx: ShellContext): Promise<{ name: string | null; displayName: string | null; revision: number; dirty: boolean; documentId: string }> {
  const p = replyJson(await ctx.core("GET", "/api/project")) ?? {};
  const d = replyJson(await ctx.core("GET", "/api/documents")) ?? {};
  return {
    name: typeof p.name === "string" ? p.name : null,
    displayName: typeof p.display_name === "string" ? p.display_name : null,
    revision: typeof p.revision === "number" ? p.revision : 0,
    dirty: p.dirty === true,
    documentId: typeof d.active === "string" ? d.active : "default",
  };
}

export async function snapshot(ctx: ShellContext): Promise<Snapshot> {
  const st = await projectStatus(ctx);
  const dl = await ctx.core("GET", "/api/document/download", "format=openetch");
  if (dl.status !== 200) throw coreError(dl);
  return { ...st, content: dl.body };
}

export async function fileSettings(ctx: ShellContext): Promise<FileSettings> {
  const s = await ctx.store.get<Partial<FileSettings>>(S.settings, "files");
  return { ...DEFAULT_SETTINGS, ...(s ?? {}) };
}

/** `<stem>.<unix>.openetch` copies in `meta`, newest first; `project` keeps only that stem (ignoring case). */
export async function stored(ctx: ShellContext, meta: string, project?: string | null): Promise<StoredMeta[]> {
  const rows = (await ctx.store.entries<StoredMeta>(meta)).map(([, v]) => v);
  return rows
    .filter((r) => project === undefined || project === null || sameNameIgnoringCase(project, r.project))
    .sort((a, b) => b.saved_unix_s - a.saved_unix_s || (a.file < b.file ? 1 : a.file > b.file ? -1 : 0));
}

/** A free `<stem>.<unix>.openetch` name, never older than the newest copy of `stem`. */
export async function stamped(ctx: ShellContext, meta: string, stem: string): Promise<{ file: string; unix: number }> {
  const all = await stored(ctx, meta, stem);
  const newest = all.length ? all[0].saved_unix_s + 1 : 0;
  const taken = new Set((await ctx.store.keys(meta)));
  let t = Math.max(Math.floor(ctx.now() / 1000), newest);
  for (;;) {
    const file = `${stem}.${t}.${PROJECT_EXT}`;
    if (!taken.has(file)) return { file, unix: t };
    t += 1;
  }
}

/** Write a copy of `content` as `stem` and keep the newest `keep` (one batch). */
async function writeCopy(ctx: ShellContext, meta: string, data: string, stem: string, content: Uint8Array, keep: number): Promise<StoredMeta> {
  const { file, unix } = await stamped(ctx, meta, stem);
  const row: StoredMeta = { file, project: stem, saved_unix_s: unix, size: content.byteLength };
  const old = (await stored(ctx, meta, stem)).slice(Math.max(0, keep - 1));
  await ctx.store.batch([
    { store: meta, key: file, value: row },
    { store: data, key: file, value: content },
    ...old.flatMap((o) => [
      { store: meta, key: o.file, value: null },
      { store: data, key: o.file, value: null },
    ]),
  ]);
  return row;
}

/**
 * Before project `name` is replaced: copy its stored bytes to the backups and keep the newest `keep`. A missing
 * project (first save) or `keep` 0 does nothing.
 */
export async function backupBeforeSave(ctx: ShellContext, name: string, keep: number): Promise<StoredMeta | null> {
  if (keep <= 0) return null;
  const old = await ctx.store.get<Uint8Array>(S.projectData, name);
  if (!old) return null;
  const row = await writeCopy(ctx, S.backups, S.backupData, flatName(name), old, keep);
  ctx.changed([S.backups]);
  return row;
}

/** The autosave of the active document (engine `autosave_now`). */
export class Autosaver {
  private readonly ctx: ShellContext;
  /** Revision of the last autosave (or -1). */
  private autosavedRevision = -1;
  /** When the interval last started (Unix ms). */
  private last: number;

  constructor(ctx: ShellContext) {
    this.ctx = ctx;
    this.last = ctx.now();
  }

  /** Called every 30 s: autosave when the interval has passed. Returns the file written, or null. */
  async tick(): Promise<string | null> {
    const s = await fileSettings(this.ctx);
    if (s.autosave_minutes === 0 || this.ctx.now() - this.last < s.autosave_minutes * 60_000) return null;
    const file = await this.autosaveNow(false);
    this.last = this.ctx.now();
    return file;
  }

  /** Write an autosave when the document changed since the last one (always with `force`). */
  async autosaveNow(force: boolean): Promise<string | null> {
    const s = await fileSettings(this.ctx);
    if (s.autosave_minutes === 0 && !force) return null;
    const st = await projectStatus(this.ctx);
    if (!force && (!st.dirty || this.autosavedRevision === st.revision)) return null;
    const dl = await this.ctx.core("GET", "/api/document/download", "format=openetch");
    if (dl.status !== 200) throw coreError(dl);
    const stem =
      st.name !== null && projectNameProblem(st.name) === null
        ? flatName(st.name)
        : st.displayName !== null && nameProblem(st.displayName) === null
          ? st.displayName
          : "Untitled";
    const row = await writeCopy(this.ctx, S.autosaves, S.autosaveData, stem, dl.body, s.autosave_keep);
    this.autosavedRevision = st.revision;
    this.ctx.changed([S.autosaves]);
    return row.file;
  }
}

/** Timers, injectable for tests. */
export interface Timers {
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(t: unknown): void;
}

const realTimers: Timers = {
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (t) => globalThis.clearTimeout(t as ReturnType<typeof setTimeout>),
};

/** Recovery records older than this are dropped at boot (tabs closed long ago). */
export const RECOVERY_MAX_AGE_MS = 14 * 24 * 3600_000;

/**
 * The tab's recovery copy. `noteChange()` after every change schedules a write `debounceMs` after the last change,
 * at most once per `minGapMs` (longer for slow writes); `writeNow()` is the page's flush (hidden or unloaded).
 */
export class RecoveryKeeper {
  private readonly ctx: ShellContext;
  private readonly run: (fn: () => Promise<void>) => Promise<void>;
  private readonly timers: Timers;
  private readonly debounceMs: number;
  private readonly minGapMs: number;
  private timer: unknown = null;
  private lastWrite = 0;
  private lastCost = 0;
  private pending = false;

  constructor(
    ctx: ShellContext,
    run: (fn: () => Promise<void>) => Promise<void>,
    opts: { timers?: Timers; debounceMs?: number; minGapMs?: number } = {},
  ) {
    this.ctx = ctx;
    this.run = run;
    this.timers = opts.timers ?? realTimers;
    this.debounceMs = opts.debounceMs ?? 2000;
    this.minGapMs = opts.minGapMs ?? 15_000;
  }

  /** The document (or which one is active) changed. */
  noteChange(): void {
    this.pending = true;
    if (this.timer !== null) this.timers.clearTimeout(this.timer);
    const gap = Math.max(this.minGapMs, this.lastCost * 20);
    const wait = Math.max(this.debounceMs, this.lastWrite + gap - this.ctx.now());
    this.timer = this.timers.setTimeout(() => {
      this.timer = null;
      void this.run(() => this.writeNow()).catch(() => undefined);
    }, wait);
  }

  /** Write the recovery record now if anything changed since the last one. */
  async writeNow(): Promise<void> {
    if (!this.pending) return;
    this.pending = false;
    if (this.timer !== null) {
      this.timers.clearTimeout(this.timer);
      this.timer = null;
    }
    const t0 = this.ctx.now();
    const st = await projectStatus(this.ctx);
    const known = this.ctx.known.get(st.documentId);
    const knownMs = known && st.name !== null && sameNameIgnoringCase(known.name, st.name) ? known.modified_ms : null;
    const id = this.ctx.sessionId;
    if (!st.dirty && st.name === null) {
      // A clean untitled document: nothing to recover.
      await this.ctx.store.batch([
        { store: S.recovery, key: id, value: null },
        { store: S.recoveryData, key: id, value: null },
      ]);
    } else if (!st.dirty) {
      const meta: RecoveryMeta = { saved_ms: t0, project: st.name, display: null, clean: true, known_ms: knownMs };
      await this.ctx.store.batch([
        { store: S.recovery, key: id, value: meta },
        { store: S.recoveryData, key: id, value: null },
      ]);
    } else {
      const dl = await this.ctx.core("GET", "/api/document/download", "format=openetch");
      if (dl.status !== 200) return;
      const meta: RecoveryMeta = { saved_ms: t0, project: st.name, display: st.displayName, clean: false, known_ms: knownMs };
      await this.ctx.store.batch([
        { store: S.recovery, key: id, value: meta },
        { store: S.recoveryData, key: id, value: dl.body },
      ]);
    }
    this.lastWrite = this.ctx.now();
    this.lastCost = this.lastWrite - t0;
  }

  /** Read this tab's recovery record back into webcore. Null when there was nothing to restore. */
  async recover(): Promise<Recovered | null> {
    const id = this.ctx.sessionId;
    const meta = await this.ctx.store.get<RecoveryMeta>(S.recovery, id);
    if (!meta) return null;
    if (meta.clean) {
      if (meta.project === null) return null;
      const row = await this.ctx.store.get<ProjectMeta>(S.projects, meta.project);
      const data = await this.ctx.store.get<Uint8Array>(S.projectData, meta.project);
      if (!row || !data) return null;
      const r = await this.ctx.core("POST", "/api/_shell/open_project", new URLSearchParams({ name: meta.project }).toString(), data);
      if (r.status !== 200) return null;
      await this.remember(meta.project, row.modified_ms);
      return { source: "project", savedMs: meta.saved_ms, name: meta.project };
    }
    const data = await this.ctx.store.get<Uint8Array>(S.recoveryData, id);
    if (!data) return null;
    // The document with its unsaved changes: still the project's (or named as before), and counted as changed.
    const q = new URLSearchParams();
    if (meta.project !== null) q.set("project", meta.project);
    else if (meta.display) q.set("display", meta.display);
    const r = await this.ctx.core("POST", "/api/_shell/recover", q.toString(), data);
    if (r.status !== 200 || replyJson(r)?.ok === false) return null;
    if (meta.project !== null && meta.known_ms !== null) await this.remember(meta.project, meta.known_ms);
    // Written again on the next change; until then this one stays (another crash before an edit restores it again).
    return { source: "recovery", savedMs: meta.saved_ms, name: meta.project ?? (meta.display ? displayName(meta.display) : null) };
  }

  private async remember(name: string, modifiedMs: number): Promise<void> {
    const st = await projectStatus(this.ctx);
    this.ctx.known.set(st.documentId, { name, modified_ms: modifiedMs });
  }

  /** Drop recovery records older than `maxAgeMs` (other tabs, long closed). */
  static async prune(ctx: ShellContext, maxAgeMs = RECOVERY_MAX_AGE_MS): Promise<void> {
    const old = (await ctx.store.entries<RecoveryMeta>(S.recovery)).filter(([k, m]) => k !== ctx.sessionId && ctx.now() - m.saved_ms > maxAgeMs);
    if (!old.length) return;
    await ctx.store.batch(old.flatMap(([k]) => [
      { store: S.recovery, key: k, value: null },
      { store: S.recoveryData, key: k, value: null },
    ]));
  }
}

/** The project file name an autosave or backup `file` belongs to (its stem). */
export function stemOf(file: string): string | null {
  return splitStamped(file, PROJECT_EXT)?.stem ?? null;
}
