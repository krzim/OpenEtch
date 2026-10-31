// UI-only selection state. Selection is a view concern, not document state, so it lives in the client.

import type { GDocument } from "./api/types.ts";

export interface Selection {
  /** Selected object ids, in click order. */
  objects: string[];
  /** Layer shown in the properties panel (the selected object's layer, or a layer clicked in the browser). */
  layer: string | null;
  /** The stock is selected. */
  stock: boolean;
}

export const EMPTY_SELECTION: Selection = { objects: [], layer: null, stock: false };

/** Drop ids that no longer exist after a document refetch. Returns the same object when nothing changed. */
export function pruneSelection(sel: Selection, doc: GDocument | null): Selection {
  if (!doc) return sel;
  const objects = sel.objects.filter((id) => doc.objects.some((o) => o.id === id));
  const layer = sel.layer && doc.layers.some((l) => l.id === sel.layer) ? sel.layer : null;
  const stock = sel.stock && doc.stock !== null;
  if (objects.length === sel.objects.length && layer === sel.layer && stock === sel.stock) return sel;
  return { objects, layer, stock };
}

/** Selection after clicking an object; `additive` (shift) toggles it in a multi-selection. */
export function clickObject(sel: Selection, doc: GDocument, id: string, additive: boolean): Selection {
  const layer = doc.objects.find((o) => o.id === id)?.layer ?? sel.layer;
  if (!additive) return { objects: [id], layer, stock: false };
  const has = sel.objects.includes(id);
  const objects = has ? sel.objects.filter((x) => x !== id) : [...sel.objects, id];
  return { objects, layer: objects.length ? layer : sel.layer, stock: false };
}
