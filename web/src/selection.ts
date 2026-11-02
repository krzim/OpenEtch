// UI-only selection state and the LightBurn selection rules. Selection is a view concern, not document state, so it
// lives in the client. Pure functions, no DOM, so `npm test` covers them.
//
// LightBurn rules (docs.lightburnsoftware.com Reference/Selection, GetStarted/SelectingMovingSizing):
// - click selects; Shift adds, Ctrl toggles, Ctrl+Shift removes; the same modifiers apply to box selection;
// - a box dragged left to right encloses (selects what is entirely inside), right to left crosses (also selects what
//   its edges touch); with no modifier the result replaces the selection;
// - Esc or a click on empty space clears; Ctrl+A selects all, Ctrl+Shift+I inverts, Tab / Shift+Tab cycle;
// - a group selects as one unit: clicking a member selects the whole top-level group;
// - align uses the last object added to the selection as its reference (the first in draw order among those added
//   together), and Ctrl+click on an already selected object twice makes it the reference.

import type { DocObject, GDocument, Vec2 } from "./api/types.ts";
import {
  boxArea,
  boxInside,
  boxesIntersect,
  distToPolylines,
  cmdsBounds,
  flattenCmds,
  geometryCmds,
  geometryKey,
  insideEvenOdd,
  polylinesTouchBox,
  type Box,
  type Polyline,
} from "./geometry.ts";

export interface Selection {
  /** Selected object ids, in the order they were added (group members are listed individually). */
  objects: string[];
  /** Layer shown in the properties panel (the selected object's layer, or a layer clicked in the browser). */
  layer: string | null;
  /** The stock is selected. */
  stock: boolean;
  /** Ids added by the latest selection action (the align reference comes from these). */
  lastAdded?: string[];
  /** Align reference picked explicitly (Ctrl+click a selected object twice). */
  reference?: string | null;
}

export const EMPTY_SELECTION: Selection = { objects: [], layer: null, stock: false };

/** What a click or box does to the selection. */
export type SelectOp = "replace" | "add" | "toggle" | "remove";

/** LightBurn's modifiers: Shift adds, Ctrl (Cmd on macOS) toggles, both remove. Alt has no selection meaning. */
export function selectOpFor(m: { shift: boolean; ctrl: boolean }): SelectOp {
  if (m.shift && m.ctrl) return "remove";
  if (m.ctrl) return "toggle";
  if (m.shift) return "add";
  return "replace";
}

/** Ids of layers that are shown. Objects on hidden layers cannot be selected. */
export function visibleLayerIds(doc: GDocument): Set<string> {
  return new Set(doc.layers.filter((l) => l.visible).map((l) => l.id));
}

function isVisible(doc: GDocument, o: DocObject, visible: Set<string> = visibleLayerIds(doc)): boolean {
  // Objects whose layer is unknown stay selectable so nothing is unreachable.
  return visible.has(o.layer) || !doc.layers.some((l) => l.id === o.layer);
}

/** Objects by id and group parents per document (documents are immutable values), so group lookups are O(depth). */
const lookups = new WeakMap<GDocument, { byId: Map<string, DocObject>; parents: Map<string, string | null> }>();

function lookupOf(doc: GDocument) {
  let l = lookups.get(doc);
  if (!l) {
    const byId = new Map<string, DocObject>();
    // The first object with an id wins, as Array.find did.
    for (const o of doc.objects) if (!byId.has(o.id)) byId.set(o.id, o);
    l = { byId, parents: new Map((doc.groups ?? []).map((x) => [x.id, x.parent ?? null])) };
    lookups.set(doc, l);
  }
  return l;
}

/** The outermost group an object belongs to, or null. Cycles in `parent` are cut. */
export function topGroup(doc: GDocument, objectId: string): string | null {
  const { byId, parents } = lookupOf(doc);
  const o = byId.get(objectId);
  let g: string | null = o?.group ?? null;
  if (!g) return null;
  const seen = new Set<string>();
  for (;;) {
    const p: string | null = parents.get(g) ?? null;
    if (!p || seen.has(p)) return g;
    seen.add(g);
    g = p;
  }
}

/** Selection unit of an object: its top-level group id, or its own id. */
export function unitOf(doc: GDocument, objectId: string): string {
  return topGroup(doc, objectId) ?? objectId;
}

/** Object ids in a unit (all members of a top-level group, or the object itself), in draw order. */
export function unitMembers(doc: GDocument, unit: string): string[] {
  const direct = doc.objects.find((o) => o.id === unit);
  if (direct && !topGroup(doc, unit)) return [unit];
  return doc.objects.filter((o) => topGroup(doc, o.id) === unit).map((o) => o.id);
}

/** Each object id widened to its whole top-level group, duplicates dropped, order kept. */
export function expandToUnits(doc: GDocument, ids: readonly string[]): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const id of ids) {
    for (const m of unitMembers(doc, unitOf(doc, id))) {
      if (!seen.has(m)) {
        seen.add(m);
        out.push(m);
      }
    }
  }
  return out;
}

/** The units (group ids or loose object ids) covered by `ids`, in first-seen order. */
export function unitsOf(doc: GDocument, ids: readonly string[]): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const id of ids) {
    const u = unitOf(doc, id);
    if (!seen.has(u)) {
      seen.add(u);
      out.push(u);
    }
  }
  return out;
}

/** Whether any of `ids` is a group member. */
export function anyGrouped(doc: GDocument, ids: readonly string[]): boolean {
  return ids.some((id) => topGroup(doc, id) !== null);
}

function layerOf(doc: GDocument, ids: readonly string[], fallback: string | null): string | null {
  const last = ids[ids.length - 1];
  return doc.objects.find((o) => o.id === last)?.layer ?? fallback;
}

/**
 * Apply a selection operation to the objects hit by a click or box (`hit`, object ids). Each hit is widened to its
 * whole group first. Toggle flips each unit on its own.
 */
export function applySelect(sel: Selection, doc: GDocument, hit: readonly string[], op: SelectOp): Selection {
  const units = unitsOf(doc, hit);
  const membersOf = (u: string) => unitMembers(doc, u);
  let objects: string[];
  let added: string[] = [];
  switch (op) {
    case "replace":
      objects = units.flatMap(membersOf);
      added = objects;
      break;
    case "add": {
      objects = [...sel.objects];
      for (const m of units.flatMap(membersOf)) {
        if (!objects.includes(m)) {
          objects.push(m);
          added.push(m);
        }
      }
      break;
    }
    case "remove": {
      const drop = new Set(units.flatMap(membersOf));
      objects = sel.objects.filter((id) => !drop.has(id));
      break;
    }
    case "toggle": {
      objects = [...sel.objects];
      for (const u of units) {
        const ms = membersOf(u);
        if (ms.every((m) => objects.includes(m))) objects = objects.filter((id) => !ms.includes(id));
        else
          for (const m of ms) {
            if (!objects.includes(m)) {
              objects.push(m);
              added.push(m);
            }
          }
      }
      break;
    }
  }
  const reference = sel.reference && objects.includes(sel.reference) ? sel.reference : null;
  return {
    objects,
    layer: objects.length ? layerOf(doc, added.length ? added : objects, sel.layer) : op === "replace" ? null : sel.layer,
    stock: false,
    lastAdded: added.length ? added : (sel.lastAdded ?? []).filter((id) => objects.includes(id)),
    reference,
  };
}

/**
 * Selection after clicking an object. `op` comes from the modifiers. Ctrl+click on an object that is already the
 * only thing just added keeps it selected and makes it the align reference (LightBurn: "Ctrl+click twice").
 */
export function clickObject(sel: Selection, doc: GDocument, id: string, op: SelectOp | boolean): Selection {
  const o: SelectOp = typeof op === "boolean" ? (op ? "toggle" : "replace") : op;
  return applySelect(sel, doc, [id], o);
}

/** Make `id` (already selected) the align reference. */
export function setReference(sel: Selection, id: string): Selection {
  return sel.objects.includes(id) ? { ...sel, reference: id } : sel;
}

/**
 * Drop ids that no longer exist, or whose layer is hidden, after a document refetch. Same object when unchanged.
 * When the selected objects moved to another layer (a palette swatch, the Layer field, an agent), the selection's
 * layer follows them, so Cuts / Layers and Properties highlight the layer they are on now.
 */
export function pruneSelection(sel: Selection, doc: GDocument | null): Selection {
  if (!doc) return sel;
  const visible = visibleLayerIds(doc);
  const byId = new Map(doc.objects.map((o) => [o.id, o]));
  const objects = sel.objects.filter((id) => {
    const o = byId.get(id);
    return !!o && isVisible(doc, o, visible);
  });
  let layer = sel.layer && doc.layers.some((l) => l.id === sel.layer) ? sel.layer : null;
  if (objects.length && !objects.some((id) => byId.get(id)?.layer === layer)) {
    const added = (sel.lastAdded ?? []).filter((id) => objects.includes(id));
    layer = layerOf(doc, added.length ? added : objects, layer);
  }
  const stock = sel.stock && doc.stock !== null;
  if (objects.length === sel.objects.length && layer === sel.layer && stock === sel.stock) return sel;
  return {
    objects,
    layer,
    stock,
    lastAdded: (sel.lastAdded ?? []).filter((id) => objects.includes(id)),
    reference: sel.reference && objects.includes(sel.reference) ? sel.reference : null,
  };
}

/** Select every visible object (Ctrl+A). */
export function selectAll(doc: GDocument): Selection {
  const visible = visibleLayerIds(doc);
  const objects = doc.objects.filter((o) => isVisible(doc, o, visible)).map((o) => o.id);
  return { objects, layer: layerOf(doc, objects, null), stock: false, lastAdded: objects, reference: null };
}

/** Select the visible objects that are not selected (Ctrl+Shift+I). Groups stay whole. */
export function invertSelection(sel: Selection, doc: GDocument): Selection {
  const visible = visibleLayerIds(doc);
  const chosen = new Set(sel.objects);
  const objects = expandToUnits(
    doc,
    doc.objects.filter((o) => isVisible(doc, o, visible) && !chosen.has(o.id)).map((o) => o.id),
  ).filter((id) => !chosen.has(id));
  return { objects, layer: layerOf(doc, objects, sel.layer), stock: false, lastAdded: objects, reference: null };
}

/** Select every visible object on a layer (Shift+click a layer row). */
export function selectLayerObjects(doc: GDocument, layer: string): Selection {
  const objects = expandToUnits(doc, doc.objects.filter((o) => o.layer === layer).map((o) => o.id));
  return { objects, layer, stock: false, lastAdded: objects, reference: null };
}

/** Tab / Shift+Tab: select the next or previous unit in creation order, wrapping around. */
export function cycleSelection(sel: Selection, doc: GDocument, dir: 1 | -1): Selection {
  const visible = visibleLayerIds(doc);
  const units = unitsOf(doc, doc.objects.filter((o) => isVisible(doc, o, visible)).map((o) => o.id));
  if (units.length === 0) return sel;
  const cur = sel.objects.length ? unitOf(doc, sel.objects[sel.objects.length - 1]) : null;
  const i = cur === null ? -1 : units.indexOf(cur);
  const next = i < 0 ? (dir === 1 ? 0 : units.length - 1) : (i + dir + units.length) % units.length;
  return applySelect(sel, doc, unitMembers(doc, units[next]), "replace");
}

/** The object to align to: the explicit reference, else the first in draw order among the objects added last. */
export function alignReference(sel: Selection, doc: GDocument): string | null {
  if (sel.reference && sel.objects.includes(sel.reference)) return sel.reference;
  const pool = (sel.lastAdded ?? []).filter((id) => sel.objects.includes(id));
  const from = new Set(pool.length ? pool : sel.objects);
  return doc.objects.find((o) => from.has(o.id))?.id ?? null;
}

// ---- hit testing --------------------------------------------------------------------------------------------------

/** Flattened outline of one object, for hit testing and box selection. */
export interface ShapeEntry {
  id: string;
  polys: Polyline[];
  box: Box;
  /** Has at least one closed subpath, so clicking inside it selects it. */
  closed: boolean;
  area: number;
}

/** Entries by object id, reused while an object's geometry and transform are unchanged (geometry.ts geometryKey). */
const shapeCache = new Map<string, { key: string; t: readonly number[]; entry: ShapeEntry | null }>();

/** Hit-test data for the visible objects in draw order. Build once per document. */
export function buildShapeIndex(doc: GDocument): ShapeEntry[] {
  const visible = visibleLayerIds(doc);
  const out: ShapeEntry[] = [];
  const seen = new Set<string>();
  for (const o of doc.objects) {
    if (!isVisible(doc, o, visible)) continue;
    seen.add(o.id);
    const key = geometryKey(o.geometry);
    const t = o.transform;
    let c = shapeCache.get(o.id);
    if (!c || c.key !== key || c.t.some((v, i) => v !== t[i])) {
      const cmds = geometryCmds(o.geometry);
      const box = cmdsBounds(cmds, t);
      const polys = box ? flattenCmds(cmds, t) : [];
      c = { key, t: [...t], entry: box ? { id: o.id, polys, box, closed: polys.some((p) => p.closed), area: boxArea(box) } : null };
      shapeCache.set(o.id, c);
    }
    if (c.entry) out.push(c.entry);
  }
  for (const id of shapeCache.keys()) if (!seen.has(id)) shapeCache.delete(id);
  return out;
}

/**
 * The object under field point `p`: the topmost whose line is within `tolMm`, else the smallest closed shape that
 * contains the point (even-odd, so a hole does not select its ring). Null when nothing is hit. With `nearest` (touch
 * and pen, whose tolerances are large) the outline nearest the point wins instead of the topmost one, so a finger on
 * one of two close lines picks that line (mouse clicks keep LightBurn's topmost rule).
 */
export function hitTest(index: readonly ShapeEntry[], p: Vec2, tolMm: number, nearest = false): string | null {
  const near: Box = { x0: p[0] - tolMm, y0: p[1] - tolMm, x1: p[0] + tolMm, y1: p[1] + tolMm };
  if (nearest) {
    // Touch and pen (large tolerances): the outline nearest the point wins; equal distances go to the topmost.
    let bestOutline: string | null = null;
    let bestD = Infinity;
    for (let i = index.length - 1; i >= 0; i--) {
      const s = index[i];
      if (!boxesIntersect(s.box, near)) continue;
      const d = distToPolylines(p, s.polys);
      if (d <= tolMm && d < bestD) {
        bestD = d;
        bestOutline = s.id;
      }
    }
    if (bestOutline) return bestOutline;
  } else {
    for (let i = index.length - 1; i >= 0; i--) {
      const s = index[i];
      if (boxesIntersect(s.box, near) && distToPolylines(p, s.polys) <= tolMm) return s.id;
    }
  }
  let best: ShapeEntry | null = null;
  for (const s of index) {
    if (!s.closed || !boxesIntersect(s.box, near)) continue;
    if (insideEvenOdd(p, s.polys) && (!best || s.area <= best.area)) best = s;
  }
  return best?.id ?? null;
}

/**
 * Objects selected by a box: `crossing` (dragged right to left) also takes objects the box's edges touch; enclosing
 * takes only objects entirely inside. A group counts as one unit: enclosing needs every member inside, crossing any.
 */
export function boxHits(doc: GDocument, index: readonly ShapeEntry[], box: Box, crossing: boolean): string[] {
  const byUnit = new Map<string, ShapeEntry[]>();
  for (const s of index) {
    const u = unitOf(doc, s.id);
    const list = byUnit.get(u);
    if (list) list.push(s);
    else byUnit.set(u, [s]);
  }
  const out: string[] = [];
  for (const [, members] of byUnit) {
    const hit = crossing
      ? members.some((s) => boxInside(s.box, box) || (boxesIntersect(s.box, box) && polylinesTouchBox(s.polys, box)))
      : members.every((s) => boxInside(s.box, box));
    if (hit) out.push(...members.map((s) => s.id));
  }
  return out;
}
