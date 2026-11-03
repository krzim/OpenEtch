// Font favourites and hidden fonts (LightBurn 2.1's Font Manager), kept by the engine (GET/PUT /api/fonts/prefs,
// crates/engine/src/fontprefs.rs) so the PC, the iPad and the phone share one list, and the font picker's grouping:
// favourites first, then the engine's fonts by kind without the hidden ones (a switch shows them again). The model is
// pure and tested in fontPrefs.test.ts; the store below loads the lists once per page and saves every change.

import { ApiError, UI_ORIGIN, authToken } from "../api/client.ts";
import type { FontKind } from "../api/types.ts";
import { FONT_KINDS, sameFont, type FontInfo } from "./textModel.ts";

export interface FontRef {
  family: string;
  kind: FontKind;
}

export interface FontPrefs {
  favourites: FontRef[];
  hidden: FontRef[];
}

export const NO_FONT_PREFS: FontPrefs = { favourites: [], hidden: [] };

/** The engine's answer, checked. */
export function parseFontPrefs(raw: unknown): FontPrefs {
  const list = (v: unknown): FontRef[] =>
    Array.isArray(v)
      ? v.flatMap((f) => {
          if (!f || typeof f !== "object") return [];
          const x = f as Record<string, unknown>;
          const kind = (FONT_KINDS as string[]).includes(x.kind as string) ? (x.kind as FontKind) : "system";
          return typeof x.family === "string" && x.family.trim() ? [{ family: x.family, kind }] : [];
        })
      : [];
  const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  return { favourites: list(o.favourites), hidden: list(o.hidden) };
}

export const isFavourite = (p: FontPrefs, f: FontRef): boolean => p.favourites.some((x) => sameFont(x, f));
export const isHidden = (p: FontPrefs, f: FontRef): boolean => p.hidden.some((x) => sameFont(x, f));

/** Star or unstar `f` (a favourite is never hidden). */
export function toggleFavourite(p: FontPrefs, f: FontRef): FontPrefs {
  const ref = { family: f.family, kind: f.kind };
  return isFavourite(p, f)
    ? { ...p, favourites: p.favourites.filter((x) => !sameFont(x, f)) }
    : { favourites: [...p.favourites, ref], hidden: p.hidden.filter((x) => !sameFont(x, f)) };
}

/** Hide or show `f` (hiding also unstars it). */
export function toggleHidden(p: FontPrefs, f: FontRef): FontPrefs {
  const ref = { family: f.family, kind: f.kind };
  return isHidden(p, f)
    ? { ...p, hidden: p.hidden.filter((x) => !sameFont(x, f)) }
    : { favourites: p.favourites.filter((x) => !sameFont(x, f)), hidden: [...p.hidden, ref] };
}

export type PickerGroupId = "favourites" | FontKind;

export interface PickerGroup {
  id: PickerGroupId;
  fonts: FontInfo[];
}

/**
 * The picker's groups for `query`: favourites the engine has (in the order they were starred), then each kind in
 * FONT_KINDS order sorted by name, without hidden fonts unless `showHidden`. `hiddenCount` is how many matching
 * fonts are hidden. The font in use (`keep`) is listed even when hidden, so the picker never hides the selection.
 */
export function pickerGroups(
  list: readonly FontInfo[],
  query: string,
  prefs: FontPrefs,
  showHidden: boolean,
  keep?: FontRef | null,
): { groups: PickerGroup[]; hiddenCount: number } {
  const q = query.trim().toLowerCase();
  const match = (f: FontInfo) => !q || f.family.toLowerCase().includes(q);
  const kept = (f: FontInfo) => !!keep && sameFont(f, keep);
  const hidden = list.filter((f) => match(f) && isHidden(prefs, f));
  const shown = (f: FontInfo) => match(f) && (showHidden || kept(f) || !isHidden(prefs, f));
  const favs = prefs.favourites.flatMap((r) => list.filter((f) => sameFont(f, r) && match(f)).slice(0, 1));
  const groups: PickerGroup[] = favs.length ? [{ id: "favourites", fonts: favs }] : [];
  for (const kind of FONT_KINDS) {
    const fonts = list.filter((f) => f.kind === kind && shown(f)).sort((a, b) => a.family.localeCompare(b.family));
    if (fonts.length) groups.push({ id: kind, fonts });
  }
  return { groups, hiddenCount: hidden.length };
}

// ---- the engine's lists (one store per page) --------------------------------------------------------------------

let current: FontPrefs = NO_FONT_PREFS;
let loaded: Promise<void> | null = null;
const listeners = new Set<() => void>();

function headers(json: boolean): Record<string, string> {
  const h: Record<string, string> = { Accept: "application/json" };
  if (json) h["Content-Type"] = "application/json";
  const t = authToken();
  if (t) h.Authorization = `Bearer ${t}`;
  return h;
}

async function call(method: "GET" | "PUT", body?: unknown): Promise<FontPrefs> {
  let res: Response;
  try {
    res = await fetch("/api/fonts/prefs", { method, headers: headers(body !== undefined), body: body === undefined ? undefined : JSON.stringify(body) });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  const text = await res.text();
  if (!res.ok) {
    let msg = text || res.statusText;
    try {
      const o = JSON.parse(text) as { error?: unknown };
      if (typeof o.error === "string") msg = o.error;
    } catch {
      // Not JSON.
    }
    throw new ApiError(`${method} /api/fonts/prefs: ${res.status} ${msg}`, res.status, { detail: msg });
  }
  return parseFontPrefs(JSON.parse(text || "{}"));
}

export function fontPrefs(): FontPrefs {
  return current;
}

/** Load the lists once (an engine without them keeps the empty lists). */
export function loadFontPrefs(): Promise<void> {
  loaded ??= call("GET")
    .then((p) => {
      current = p;
      listeners.forEach((f) => f());
    })
    .catch(() => undefined);
  return loaded;
}

/** Store `next` (shown at once; put back when the engine refuses it). */
export async function saveFontPrefs(next: FontPrefs): Promise<void> {
  const before = current;
  current = next;
  listeners.forEach((f) => f());
  try {
    current = await call("PUT", { favourites: next.favourites, hidden: next.hidden, origin: UI_ORIGIN });
  } catch (e) {
    current = before;
    throw e;
  } finally {
    listeners.forEach((f) => f());
  }
}

export function subscribeFontPrefs(f: () => void): () => void {
  listeners.add(f);
  return () => listeners.delete(f);
}
