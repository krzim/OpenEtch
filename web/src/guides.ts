// Manual guidelines (LightBurn's guides dragged out of the rulers): a vertical guide at x = v or a horizontal one at
// y = v, in field mm. They live in the web UI only, kept in this browser per project: the saved project's name, else
// the file the design was opened from, else one slot for the unsaved document (`guideScope`). Saving carries the
// guides to the project's slot (`carryGuides`) and New clears the unsaved slot, so projects never share guides.
// Guides stored by an older version under the document's name are adopted once by the first project that loads with
// that name (`loadScopedGuides`). Guides are snap targets for snapping.ts: the guide lines are outlines (nearest
// point, crossings with objects) and two crossing guides give an intersection point. Pure apart from the storage
// wrappers; tested in guides.test.ts.

import type { Vec2 } from "./api/types.ts";
import type { Box } from "./geometry.ts";
import { outlineOf, type SnapPart, type SnapPoint } from "./snapping.ts";

export interface Guide {
  id: string;
  /** "x": a vertical line at x = v; "y": a horizontal line at y = v. */
  axis: "x" | "y";
  v: number;
}

const KEY = "openetch.guides.";
const SCOPED_KEY = "openetch.guides@";

/** Storage key an older version kept a document's guides under (by document name, which projects could share). */
export function guidesKey(docName: string | null | undefined): string {
  return KEY + (docName || "Untitled");
}

/** The unsaved document's slot. */
export const UNSAVED_SCOPE = "unsaved";

/**
 * Which project the guides belong to: the saved project (names compare ignoring case, like project files), else the
 * file the design was opened from, else the unsaved document.
 */
export function guideScope(projectName: string | null | undefined, openedName: string | null | undefined): string {
  const p = projectName?.trim();
  if (p) return `project:${p.toLowerCase()}`;
  const f = openedName?.trim();
  if (f) return `file:${f}`;
  return UNSAVED_SCOPE;
}

/** Storage key of a scope's guides. */
export function scopedGuidesKey(scope: string): string {
  return SCOPED_KEY + scope;
}

/** The part of `Storage` the guide slots use (localStorage, or a map in tests). */
export interface GuideStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function write(store: GuideStore, key: string, guides: readonly Guide[]): void {
  if (guides.length) store.setItem(key, JSON.stringify(guides));
  else store.removeItem(key);
}

/**
 * A scope's guides. With none stored yet, guides an older version kept under the document's name (`docName`) are
 * moved into this scope, once.
 */
export function loadScopedGuides(store: GuideStore, scope: string, docName: string | null | undefined): Guide[] {
  const key = scopedGuidesKey(scope);
  const raw = store.getItem(key);
  if (raw !== null) return parseGuides(raw);
  const legacyKey = guidesKey(docName);
  const legacy = store.getItem(legacyKey);
  if (legacy === null) return [];
  const g = parseGuides(legacy);
  write(store, key, g);
  store.removeItem(legacyKey);
  return g;
}

export function saveScopedGuides(store: GuideStore, scope: string, guides: readonly Guide[]): void {
  write(store, scopedGuidesKey(scope), guides);
}

/**
 * After the document was saved as another project (Save, Save as) its guides go with it: `to` gets `from`'s guides
 * (also none, replacing what an overwritten project had). The unsaved slot is emptied; a project or file keeps its own.
 */
export function carryScopedGuides(store: GuideStore, from: string, to: string): void {
  if (from === to) return;
  write(store, scopedGuidesKey(to), parseGuides(store.getItem(scopedGuidesKey(from))));
  if (from === UNSAVED_SCOPE) store.removeItem(scopedGuidesKey(from));
}

// ---- this browser's storage, with change notices for the canvas ----

const changed = new Set<(scope: string) => void>();

/** Called with a scope whose stored guides were changed by `carryGuides` / `clearGuides`. */
export function onGuidesChanged(fn: (scope: string) => void): () => void {
  changed.add(fn);
  return () => changed.delete(fn);
}

function browserStore(): GuideStore | null {
  try {
    return localStorage;
  } catch {
    return null;
  }
}

/** `loadScopedGuides` on this browser's storage; nothing when storage is unavailable. */
export function loadGuidesFor(scope: string, docName: string | null | undefined): Guide[] {
  const s = browserStore();
  try {
    return s ? loadScopedGuides(s, scope, docName) : [];
  } catch {
    return [];
  }
}

export function saveGuidesFor(scope: string, guides: readonly Guide[]): void {
  const s = browserStore();
  try {
    if (s) saveScopedGuides(s, scope, guides);
  } catch {
    // Storage unavailable or full: guides last for this visit.
  }
}

/** `carryScopedGuides` on this browser's storage (after a save). */
export function carryGuides(from: string, to: string): void {
  const s = browserStore();
  try {
    if (s) carryScopedGuides(s, from, to);
  } catch {
    // Storage unavailable: nothing was kept to carry.
  }
  for (const fn of changed) fn(to);
}

/** Forget a scope's guides (New replaces the unsaved document). */
export function clearGuides(scope: string): void {
  saveGuidesFor(scope, []);
  for (const fn of changed) fn(scope);
}

/** Guides from stored JSON; anything malformed is dropped. */
export function parseGuides(raw: string | null): Guide[] {
  if (!raw) return [];
  try {
    const v: unknown = JSON.parse(raw);
    if (!Array.isArray(v)) return [];
    return v
      .filter((g): g is Guide => !!g && typeof g === "object" && (g.axis === "x" || g.axis === "y") && typeof g.v === "number" && Number.isFinite(g.v))
      .slice(0, 200)
      .map((g, i) => ({ id: typeof g.id === "string" && g.id ? g.id : `g${i}`, axis: g.axis, v: g.v }));
  } catch {
    return [];
  }
}

/** A new guide id not used by `guides`. */
export function newGuideId(guides: readonly Guide[]): string {
  let n = guides.length + 1;
  while (guides.some((g) => g.id === `g${n}`)) n++;
  return `g${n}`;
}

/** "Vertical guide X 12.5" / "Horizontal guide Y -3". */
export function guideLabel(g: Pick<Guide, "axis" | "v">): string {
  return `${g.axis === "x" ? "Vertical guide X" : "Horizontal guide Y"} ${Number(g.v.toFixed(3))}`;
}

/** The guide whose line passes within `tol` mm of `p`, nearest first. */
export function guideAt(guides: readonly Guide[], p: Vec2, tol: number): Guide | null {
  let best: { d: number; g: Guide } | null = null;
  for (const g of guides) {
    const d = Math.abs((g.axis === "x" ? p[0] : p[1]) - g.v);
    if (d <= tol && (!best || d < best.d)) best = { d, g };
  }
  return best ? best.g : null;
}

/**
 * Snap data of the guides over `extent` (the workspace, generously padded): each guide is an outline, and every
 * vertical/horizontal pair crosses at an intersection point.
 */
export function guideSnapPart(guides: readonly Guide[], extent: Box): SnapPart | null {
  if (!guides.length) return null;
  const polys = guides.map((g) => ({
    pts: (g.axis === "x"
      ? [
          [g.v, extent.y0],
          [g.v, extent.y1],
        ]
      : [
          [extent.x0, g.v],
          [extent.x1, g.v],
        ]) as Vec2[],
    closed: false,
  }));
  const points: SnapPoint[] = [];
  for (const a of guides) {
    if (a.axis !== "x") continue;
    for (const b of guides) if (b.axis === "y") points.push({ p: [a.v, b.v], kind: "intersection", what: "Intersection", owner: "guides", id: "guides" });
  }
  return { points, outline: outlineOf("guides", "guide", polys) };
}
