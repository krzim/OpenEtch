// User fonts in the design-only build. The bundled Hershey fonts are always there; a TTF/OTF the person adds (the
// Font picker's "Add font…", or the Local Font Access API on Chromium) is stored in IndexedDB and handed to webcore
// (`POST /api/_shell/font`, text::registry::add_font_data) at every boot, so `font.list` and text layout see it.
//
// Routes (design-only; the engine reads installed fonts itself and has none of these):
//
// | Method | Path | Body / result |
// |---|---|---|
// | GET | `/api/fonts/user` | `{fonts: [{name, family, size, added_unix_s}]}` |
// | POST | `/api/fonts/user?name=<file name>` | the font file (≤ 30 MB) → `{ok, name, family}`; the same name replaces it |
// | DELETE | `/api/fonts/user/{name}` | `{ok}`; the font is gone after the next reload (webcore cannot unload one) |

import { S, type FontMeta, type ShellContext } from "./records.ts";
import { badRequest, json, notFound, parseQuery, replyJson, type ShellReply, type ShellRequest } from "./reply.ts";
import { coreError } from "./autosave.ts";

export const MAX_FONT_BYTES = 30 * 1024 * 1024;
const FONT_FILE = /^[^/\\:\u0000-\u001f]{1,200}\.(ttf|otf|ttc)$/i;

/** What the first bytes say: an OpenType/TrueType font or collection, a WOFF (not taken), or something else. */
export function fontKind(b: Uint8Array): "font" | "woff" | null {
  if (b.length < 12) return null;
  const tag = String.fromCharCode(b[0], b[1], b[2], b[3]);
  if (tag === "OTTO" || tag === "true" || tag === "ttcf" || (b[0] === 0 && b[1] === 1 && b[2] === 0 && b[3] === 0)) return "font";
  if (tag === "wOFF" || tag === "wOF2") return "woff";
  return null;
}

/** Hand every stored font to webcore (boot). A font webcore refuses now is skipped, not fatal. */
export async function loadFonts(ctx: ShellContext): Promise<string[]> {
  const loaded: string[] = [];
  for (const [name] of await ctx.store.entries<FontMeta>(S.fonts)) {
    const data = await ctx.store.get<Uint8Array>(S.fontData, name);
    if (!data) continue;
    const r = await ctx.core("POST", "/api/_shell/font", new URLSearchParams({ name }).toString(), data);
    if (r.status === 200) loaded.push(name);
  }
  return loaded;
}

/** One font another tab added: load it here too. */
export async function loadFont(ctx: ShellContext, name: string): Promise<void> {
  const data = await ctx.store.get<Uint8Array>(S.fontData, name);
  if (data) await ctx.core("POST", "/api/_shell/font", new URLSearchParams({ name }).toString(), data);
}

/** The font routes, or null when `req` is not one. */
export async function fontRoute(ctx: ShellContext, req: ShellRequest): Promise<ShellReply | null> {
  const base = "/api/fonts/user";
  if (req.path === base && req.method === "GET") {
    parseQuery(req.query, []);
    const fonts = (await ctx.store.entries<FontMeta>(S.fonts)).map(([, m]) => ({ name: m.name, family: m.family, size: m.size, added_unix_s: Math.floor(m.added_ms / 1000) }));
    return json({ fonts });
  }
  if (req.path === base && req.method === "POST") {
    const q = parseQuery(req.query, ["name"]);
    const name = q.get("name");
    if (!name) throw badRequest("name is required");
    if (!FONT_FILE.test(name)) throw badRequest(`bad font file name ${JSON.stringify(name)} (a .ttf, .otf or .ttc file)`);
    if (req.body.byteLength === 0) throw badRequest("the upload is empty");
    if (req.body.byteLength > MAX_FONT_BYTES) throw badRequest(`the font is larger than ${MAX_FONT_BYTES / (1024 * 1024)} MB`);
    const kind = fontKind(req.body);
    if (kind === "woff") throw badRequest(`${name} is a web font (WOFF); add the .ttf or .otf file instead`);
    if (kind === null) throw badRequest(`${name} is not a TrueType or OpenType font`);
    const r = await ctx.core("POST", "/api/_shell/font", new URLSearchParams({ name }).toString(), req.body);
    if (r.status !== 200) throw coreError(r);
    const family = String(replyJson(r)?.family ?? name.replace(/\.[^.]+$/, ""));
    const meta: FontMeta = { name, family, size: req.body.byteLength, added_ms: ctx.now() };
    await ctx.store.batch([
      { store: S.fonts, key: name, value: meta },
      { store: S.fontData, key: name, value: req.body },
    ]);
    ctx.changed([S.fonts]);
    return json({ ok: true, name, family });
  }
  if (req.path.startsWith(`${base}/`) && req.method === "DELETE") {
    parseQuery(req.query, []);
    const name = decodeURIComponent(req.path.slice(base.length + 1));
    if (!(await ctx.store.get(S.fonts, name))) throw notFound(`no font named ${JSON.stringify(name)}`);
    await ctx.store.batch([
      { store: S.fonts, key: name, value: null },
      { store: S.fontData, key: name, value: null },
    ]);
    ctx.changed([S.fonts]);
    return json({ ok: true });
  }
  return null;
}

// --- Page side: the Local Font Access API (Chromium desktop) ------------------------------------------------------

interface LocalFontData {
  family: string;
  fullName: string;
  postscriptName: string;
  style: string;
  blob(): Promise<Blob>;
}

/** Whether this browser can list the computer's installed fonts (Chromium desktop, on an explicit button). */
export function localFontsSupported(): boolean {
  return typeof (globalThis as { queryLocalFonts?: unknown }).queryLocalFonts === "function";
}

/**
 * The installed fonts of `families` (all when empty), from the Local Font Access API: the browser asks the person
 * first. Each comes as a file to POST to `/api/fonts/user`. Call it from a click.
 */
export async function localFonts(families: string[] = []): Promise<{ name: string; family: string; blob: Blob }[]> {
  const q = (globalThis as { queryLocalFonts?: () => Promise<LocalFontData[]> }).queryLocalFonts;
  if (!q) return [];
  const want = new Set(families.map((f) => f.toLowerCase()));
  const out: { name: string; family: string; blob: Blob }[] = [];
  for (const f of await q()) {
    if (want.size && !want.has(f.family.toLowerCase())) continue;
    const blob = await f.blob();
    const head = new Uint8Array(await blob.slice(0, 4).arrayBuffer());
    const ext = String.fromCharCode(...head) === "OTTO" ? "otf" : "ttf";
    out.push({ name: `${f.postscriptName || f.fullName}.${ext}`, family: f.family, blob });
  }
  return out;
}
