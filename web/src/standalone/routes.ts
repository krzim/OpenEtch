// The routes the worker shell answers itself from the KeyValueStore: projects (save, open, list, folders,
// thumbnails), autosaves and backups, file settings, templates, user fonts and storage. Same paths, bodies and JSON
// as the engine (crates/engine/src/api.rs, userfiles.rs, folders.rs, documents.rs); the bytes go to and come from
// webcore (`GET /api/document/download?format=openetch`, `POST /api/_shell/open_project`, `POST /api/document/upload`).
//
// Two tabs each have their own worker and document. A save compares the project's stored `modified_ms` with what
// this tab last opened or saved: when another tab saved it since, the save is refused with 409
// `code: "changed_elsewhere"` unless `overwrite: true` (the UI asks first). Other tabs hear of every change through
// the BroadcastChannel (backend.ts).

import { backupBeforeSave, coreError, fileSettings, projectStatus, snapshot, stemOf, stored } from "./autosave.ts";
import { fontRoute } from "./fonts.ts";
import { StorageFullError } from "./idb.ts";
import {
  flatName,
  folderOf,
  folderProblem,
  nameProblem,
  plainFileProblem,
  PROJECT_EXT,
  projectNameProblem,
  sameNameIgnoringCase,
} from "./names.ts";
import { DEFAULT_SETTINGS, nextStamp, S, type FileSettings, type ProjectMeta, type ShellContext, type TemplateMeta, type ThumbRecord } from "./records.ts";
import {
  badRequest,
  binary,
  conflict,
  errorReply,
  HttpError,
  json,
  notFound,
  optBool,
  parseBody,
  parseQuery,
  replyJson,
  str,
  text,
  type ShellReply,
  type ShellRequest,
} from "./reply.ts";
import type { CoreReply } from "./contract.ts";

/** Path prefixes answered here (or refused here); everything else goes to webcore. */
export const SHELL_PATHS = ["/api/projects", "/api/backups", "/api/files/settings", "/api/templates", "/api/fonts/user", "/api/storage"];

/** Whether the shell may answer `req` (POST /api/documents only when it opens a project). */
export function isShellPath(method: string, path: string): boolean {
  if (method === "POST" && path === "/api/documents") return true;
  return SHELL_PATHS.some((p) => path === p || path.startsWith(`${p}/`));
}

const ORIGIN = "origin";

/** Answer `req`, or null when webcore should. Errors become the engine's `{ok: false, error}` answers. */
export async function shellRoute(ctx: ShellContext, req: ShellRequest): Promise<ShellReply | null> {
  try {
    return await route(ctx, req);
  } catch (e) {
    if (e instanceof HttpError) return errorReply(e);
    if (e instanceof StorageFullError) return errorReply(new HttpError(507, e.message, "storage_full"));
    throw e;
  }
}

async function route(ctx: ShellContext, req: ShellRequest): Promise<ShellReply | null> {
  const { method: m, path: p } = req;
  if (p === "/api/projects" && m === "GET") return listProjects(ctx, req);
  if (p === "/api/projects/save" && m === "POST") return saveProject(ctx, req);
  if (p === "/api/projects/open" && m === "POST") return openProject(ctx, req);
  if (p === "/api/documents" && m === "POST") return openInNewDocument(ctx, req);
  if (p === "/api/projects/folders") {
    if (m === "GET") return folders(ctx, req);
    if (m === "POST") return createFolder(ctx, req);
  }
  if (p === "/api/projects/locations") {
    if (m === "GET") return json({ locations: [] });
    throw new HttpError(404, "project locations are not available in the design-only build: projects are kept in this browser", "design_only");
  }
  if (p.startsWith("/api/projects/") && p.endsWith("/thumbnail.png") && m === "GET") return thumbnail(ctx, req);
  if (p === "/api/backups" && m === "GET") return backups(ctx, req);
  if (p === "/api/backups/restore" && m === "POST") return restore(ctx, req);
  if (p === "/api/files/settings") {
    if (m === "GET") return json(await fileSettings(ctx));
    if (m === "PUT") return putSettings(ctx, req);
  }
  if (p === "/api/templates" && m === "GET") return templates(ctx, req);
  if (p === "/api/templates/save" && m === "POST") return saveTemplate(ctx, req);
  if (p === "/api/templates/upload" && m === "POST") return uploadTemplate(ctx, req);
  if (p === "/api/templates/open" && m === "POST") return openTemplate(ctx, req);
  if (p === "/api/storage" && m === "GET") return storage(ctx);
  const f = await fontRoute(ctx, req);
  if (f) return f;
  if (isShellPath(m, p)) throw new HttpError(404, "no such API route");
  return null;
}

/** webcore's answer passed on to the page as it is. */
function passOn(r: CoreReply): ShellReply {
  return { status: r.status, headers: r.headers, body: r.body };
}

function check(problem: string | null): void {
  if (problem) throw badRequest(problem);
}

// --- Projects -----------------------------------------------------------------------------------------------------

async function projectRows(ctx: ShellContext): Promise<ProjectMeta[]> {
  return (await ctx.store.entries<ProjectMeta>(S.projects)).map(([, v]) => v);
}

async function listProjects(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  parseQuery(req.query, []);
  const rows = (await projectRows(ctx))
    .map((r) => ({ name: r.name, folder: r.folder, modified_unix_s: Math.floor(r.modified_ms / 1000), size: r.size }))
    .sort((a, b) => cmp(a.name.toLowerCase(), b.name.toLowerCase()) || cmp(a.name, b.name));
  return json(rows);
}

function cmp(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** The stored projects `name` would replace: the exact name, else every one that differs only in case. */
async function replacedBy(ctx: ShellContext, name: string): Promise<ProjectMeta[]> {
  const rows = await projectRows(ctx);
  const exact = rows.find((r) => r.name === name);
  if (exact) return [exact];
  return rows.filter((r) => sameNameIgnoringCase(r.name, name)).sort((a, b) => cmp(a.name, b.name));
}

/** The stored project `name` refers to: the exact name, else the only one differing in case (several: 409). */
async function resolveProject(ctx: ShellContext, name: string): Promise<ProjectMeta> {
  check(projectNameProblem(name));
  const matches = await replacedBy(ctx, name);
  if (matches.length === 1) return matches[0];
  if (matches.length === 0) throw notFound(`no project named "${name}"`);
  throw conflict(`several projects match "${name}" ignoring case (${matches.map((x) => `"${x.name}"`).join(", ")}); use the exact name`);
}

async function saveProject(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const b = parseBody(req.body, ["name", "overwrite", ORIGIN], ["name"]);
  const name = str(b, "name");
  const overwriteArg = optBool(b, "overwrite");
  const overwrite = overwriteArg ?? true;
  check(projectNameProblem(name));
  const snap = await snapshot(ctx);
  const existing = await replacedBy(ctx, name);
  if (!overwrite) {
    const clash = existing.find((e) => e.name !== snap.name);
    if (clash) {
      const what = `a project named "${clash.name}"`;
      const casing = clash.name === name ? "" : " (names that differ only in case are the same file)";
      throw conflict(`${what} already exists${casing}; pass overwrite: true to replace it`);
    }
  }
  // Another tab saved this project after this tab opened or saved it: replacing it would lose that work.
  const known = ctx.known.get(snap.documentId);
  const target = existing[0];
  if (overwriteArg !== true && target && known && sameNameIgnoringCase(known.name, target.name) && known.modified_ms !== target.modified_ms) {
    throw conflict(
      `project "${target.name}" was saved in another tab or window after this one opened it; pass overwrite: true to replace it`,
      "changed_elsewhere",
    );
  }
  const settings = await fileSettings(ctx);
  if (target) await backupBeforeSave(ctx, target.name, settings.backups_keep);
  const modified = nextStamp(ctx.now(), target?.modified_ms);
  const row: ProjectMeta = { name, folder: folderOf(name), modified_ms: modified, size: snap.content.byteLength };
  const stale = existing.filter((e) => e.name !== name);
  const thumbs = (await ctx.store.keys(S.thumbs)).filter((k) => existing.some((e) => k.startsWith(`${e.name}|`)));
  await ctx.store.batch([
    ...stale.flatMap((e) => [
      { store: S.projects, key: e.name, value: null },
      { store: S.projectData, key: e.name, value: null },
    ]),
    ...thumbs.map((k) => ({ store: S.thumbs, key: k, value: null })),
    { store: S.projects, key: name, value: row },
    { store: S.projectData, key: name, value: snap.content },
  ]);
  ctx.changed([S.projects]);
  const marked = await ctx.core("POST", "/api/_shell/project_saved", "", jsonBytes({ name, revision: snap.revision, document_id: snap.documentId }));
  if (marked.status !== 200) throw coreError(marked);
  ctx.known.set(snap.documentId, { name, modified_ms: modified });
  return json({ ok: true, name, path: `${name}.${PROJECT_EXT}`, revision: snap.revision });
}

function jsonBytes(v: unknown): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(v));
}

/** Load stored project `row` into webcore (the active document, or a new one). */
async function loadProject(ctx: ShellContext, row: ProjectMeta, newDocument: boolean): Promise<CoreReply> {
  const data = await ctx.store.get<Uint8Array>(S.projectData, row.name);
  if (!data) throw notFound(`no project named "${row.name}"`);
  const q = new URLSearchParams({ name: row.name });
  if (newDocument) q.set("new_document", "1");
  const r = await ctx.core("POST", "/api/_shell/open_project", q.toString(), data);
  if (r.status === 200 && replyJson(r)?.ok !== false) {
    const st = await projectStatus(ctx);
    ctx.known.set(st.documentId, { name: row.name, modified_ms: row.modified_ms });
  }
  return r;
}

async function openProject(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const b = parseBody(req.body, ["name", "overwrite", ORIGIN], ["name"]);
  if (b.overwrite !== undefined) throw badRequest("overwrite is only for saving");
  const row = await resolveProject(ctx, str(b, "name"));
  return passOn(await loadProject(ctx, row, false));
}

/** `POST /api/documents {project}`: the project in a new document tab. Without `project` webcore answers. */
async function openInNewDocument(ctx: ShellContext, req: ShellRequest): Promise<ShellReply | null> {
  const t = text(req.body).trim();
  if (t === "") return null;
  let b: Record<string, unknown>;
  try {
    b = parseBody(req.body, ["project", "activate", ORIGIN]);
  } catch {
    return null;
  }
  if (b.project === undefined || b.project === null) return null;
  if (b.activate === false) throw badRequest("a project opens into the active document: leave activate out");
  const row = await resolveProject(ctx, str(b, "project"));
  return passOn(await loadProject(ctx, row, true));
}

async function folders(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  parseQuery(req.query, []);
  const set = new Set<string>(await ctx.store.keys(S.folders));
  for (const r of await projectRows(ctx)) if (r.folder) set.add(r.folder);
  // Every folder above one listed is a folder too.
  for (const f of [...set]) {
    const segs = f.split("/");
    for (let i = 1; i < segs.length; i++) set.add(segs.slice(0, i).join("/"));
  }
  const list = [...set].sort((a, b) => cmp(a.toLowerCase(), b.toLowerCase()) || cmp(a, b));
  return json({ folders: list, locations: [] });
}

async function createFolder(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const b = parseBody(req.body, ["name", ORIGIN], ["name"]);
  const name = str(b, "name");
  if (name === "") throw badRequest("invalid folder \"\": it is empty");
  check(folderProblem(name));
  const segs = name.split("/");
  const writes = segs.map((_, i) => ({ store: S.folders, key: segs.slice(0, i + 1).join("/"), value: { created_ms: ctx.now() } }));
  await ctx.store.batch(writes);
  ctx.changed([S.folders]);
  return json({ ok: true, name });
}

async function thumbnail(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const q = parseQuery(req.query, ["width"]);
  let width = 256;
  const w = q.get("width");
  if (w !== undefined) {
    width = /^\d+$/.test(w) ? Number(w) : NaN;
    if (!(width >= 32 && width <= 1024)) throw badRequest("width must be 32..=1024");
  }
  let name: string;
  try {
    name = decodeURIComponent(req.path.slice("/api/projects/".length, -"/thumbnail.png".length));
  } catch {
    throw badRequest("bad project name in the path");
  }
  const row = await resolveProject(ctx, name);
  const key = `${row.name}|${width}`;
  const cached = await ctx.store.get<ThumbRecord>(S.thumbs, key);
  if (cached && cached.modified_ms === row.modified_ms) return binary(cached.png, "image/png", [["cache-control", "no-cache"]]);
  const data = await ctx.store.get<Uint8Array>(S.projectData, row.name);
  if (!data) throw notFound(`no project named "${row.name}"`);
  const r = await ctx.core("POST", "/api/_shell/thumbnail", `width=${width}`, data);
  if (r.status !== 200) {
    const e = coreError(r);
    throw new HttpError(e.status, `project "${row.name}": ${e.message}`, e.code);
  }
  // A cache only: a full storage must not fail the picture.
  await ctx.store.batch([{ store: S.thumbs, key, value: { modified_ms: row.modified_ms, png: r.body } }]).catch(() => undefined);
  return binary(r.body, "image/png", [["cache-control", "no-cache"]]);
}

// --- Autosaves and backups ------------------------------------------------------------------------------------------

async function backups(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const q = parseQuery(req.query, ["project"]);
  const p = q.get("project");
  const project = p === undefined ? null : flatName(p);
  return json({ autosaves: await stored(ctx, S.autosaves, project), backups: await stored(ctx, S.backups, project) });
}

async function restore(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const b = parseBody(req.body, ["kind", "file", ORIGIN], ["kind", "file"]);
  const kind = str(b, "kind");
  const file = str(b, "file");
  const data = kind === "autosave" ? S.autosaveData : kind === "backup" ? S.backupData : null;
  if (data === null) throw badRequest(`unknown kind '${kind}' (autosave or backup)`);
  check(plainFileProblem(file, `.${PROJECT_EXT}`));
  const bytes = await ctx.store.get<Uint8Array>(data, file);
  if (!bytes) throw notFound(`no ${kind} ${file}`);
  // An unsaved document named after its project (the engine's restore: `display_name` = the file's stem).
  const q = new URLSearchParams({ format: "openetch" });
  const stem = stemOf(file);
  if (stem) q.set("name", stem);
  return passOn(await ctx.core("POST", "/api/document/upload", q.toString(), bytes));
}

async function putSettings(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const b = parseBody(req.body, ["autosave_minutes", "autosave_keep", "backups_keep", ORIGIN]);
  const cur: FileSettings = { ...(await fileSettings(ctx)) };
  for (const k of Object.keys(DEFAULT_SETTINGS) as (keyof FileSettings)[]) {
    if (b[k] === undefined) continue;
    const v = b[k];
    if (typeof v !== "number" || !Number.isInteger(v) || v < 0) throw badRequest(`invalid type: ${k} must be a whole number`);
    cur[k] = v;
  }
  if (cur.autosave_minutes > 24 * 60) throw badRequest("autosave_minutes must be 0..=1440");
  if (cur.autosave_keep < 1 || cur.autosave_keep > 50) throw badRequest("autosave_keep must be 1..=50");
  if (cur.backups_keep > 50) throw badRequest("backups_keep must be 0..=50");
  await ctx.store.batch([{ store: S.settings, key: "files", value: cur }]);
  ctx.changed([S.settings]);
  return json(cur);
}

// --- Templates ------------------------------------------------------------------------------------------------------

async function templateRows(ctx: ShellContext): Promise<TemplateMeta[]> {
  return (await ctx.store.entries<TemplateMeta>(S.templates)).map(([, v]) => v);
}

async function templates(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  parseQuery(req.query, []);
  const rows = (await templateRows(ctx))
    .map((r) => ({ name: r.name, kind: r.kind, modified_unix_s: Math.floor(r.modified_ms / 1000), size: r.size }))
    .sort((a, b) => cmp(a.name.toLowerCase(), b.name.toLowerCase()));
  return json(rows);
}

async function saveTemplate(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const b = parseBody(req.body, ["name", "overwrite", ORIGIN], ["name"]);
  const name = str(b, "name");
  check(nameProblem(name));
  const dl = await ctx.core("GET", "/api/document/download", "format=openetch");
  if (dl.status !== 200) throw coreError(dl);
  const same = (await templateRows(ctx)).filter((t) => t.kind === "openetch" && sameNameIgnoringCase(t.name, name));
  if (optBool(b, "overwrite") !== true && same.length) throw conflict(`a template named "${name}" exists; pass overwrite: true`);
  const prev = same.find((t) => t.name === name);
  const row: TemplateMeta = { name, kind: "openetch", modified_ms: nextStamp(ctx.now(), prev?.modified_ms), size: dl.body.byteLength };
  await ctx.store.batch([
    ...same.filter((t) => t.name !== name).flatMap((t) => [
      { store: S.templates, key: `${t.name}.openetch`, value: null },
      { store: S.templateData, key: `${t.name}.openetch`, value: null },
    ]),
    { store: S.templates, key: `${name}.openetch`, value: row },
    { store: S.templateData, key: `${name}.openetch`, value: dl.body },
  ]);
  ctx.changed([S.templates]);
  return json({ ok: true, name, path: `templates/${name}.${PROJECT_EXT}` });
}

async function uploadTemplate(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const q = parseQuery(req.query, ["name"]);
  const name = q.get("name");
  if (name === undefined) throw badRequest("name is required");
  check(nameProblem(name));
  let t: string;
  try {
    t = new TextDecoder("utf-8", { fatal: true }).decode(req.body);
  } catch {
    throw badRequest("a .lbt template must be UTF-8 XML");
  }
  const r = await ctx.core("POST", "/api/_shell/check_template", "", req.body);
  if (r.status === 404) {
    // A webcore without the check: the file must at least look like LightBurn's.
    if (!t.includes("<LightBurnProject")) throw badRequest("not a LightBurn template: no LightBurnProject element");
  } else if (r.status !== 200) {
    throw coreError(r);
  }
  const prev = (await ctx.store.get<TemplateMeta>(S.templates, `${name}.lbt`))?.modified_ms;
  const row: TemplateMeta = { name, kind: "lbt", modified_ms: nextStamp(ctx.now(), prev), size: req.body.byteLength };
  await ctx.store.batch([
    { store: S.templates, key: `${name}.lbt`, value: row },
    { store: S.templateData, key: `${name}.lbt`, value: req.body },
  ]);
  ctx.changed([S.templates]);
  return json({ ok: true, path: `templates/${name}.lbt` });
}

async function openTemplate(ctx: ShellContext, req: ShellRequest): Promise<ShellReply> {
  const b = parseBody(req.body, ["name", "overwrite", ORIGIN], ["name"]);
  const name = str(b, "name");
  check(nameProblem(name));
  const rows = await templateRows(ctx);
  const pick = (kind: TemplateMeta["kind"]) =>
    rows.find((t) => t.kind === kind && t.name === name) ?? rows.find((t) => t.kind === kind && sameNameIgnoringCase(t.name, name));
  const t = pick("openetch") ?? pick("lbt");
  if (!t) throw notFound(`no template named "${name}"`);
  const bytes = await ctx.store.get<Uint8Array>(S.templateData, `${t.name}.${t.kind}`);
  if (!bytes) throw notFound(`no template named "${name}"`);
  // A new, untitled document: the template is never the current project.
  const query = t.kind === "openetch" ? "format=openetch" : "format=lbrn2&mode=replace";
  return passOn(await ctx.core("POST", "/api/document/upload", query, bytes));
}

// --- Storage --------------------------------------------------------------------------------------------------------

async function storage(ctx: ShellContext): Promise<ShellReply> {
  const sm = (globalThis.navigator as { storage?: { estimate?: () => Promise<{ usage?: number; quota?: number }>; persisted?: () => Promise<boolean> } } | undefined)?.storage;
  let usage: number | null = null;
  let quota: number | null = null;
  let persisted: boolean | null = null;
  try {
    const e = await sm?.estimate?.();
    usage = e?.usage ?? null;
    quota = e?.quota ?? null;
  } catch {
    // Not offered here.
  }
  try {
    persisted = (await sm?.persisted?.()) ?? null;
  } catch {
    // Not offered here.
  }
  return json({ kind: ctx.store.kind, usage, quota, persisted });
}

