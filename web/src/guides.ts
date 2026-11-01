// Manual guidelines (LightBurn's guides dragged out of the rulers): a vertical guide at x = v or a horizontal one at
// y = v, in field mm. They live in the web UI only, per document (browser storage keyed by the document name), and
// are snap targets for snapping.ts: the guide lines are outlines (nearest point, crossings with objects) and two
// crossing guides give an intersection point. Pure apart from the storage helpers; tested in guides.test.ts.

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

/** Storage key for a document's guides. */
export function guidesKey(docName: string | null | undefined): string {
  return KEY + (docName || "Untitled");
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

export function loadGuides(key: string): Guide[] {
  try {
    return parseGuides(localStorage.getItem(key));
  } catch {
    return [];
  }
}

export function saveGuides(key: string, guides: readonly Guide[]): void {
  try {
    if (guides.length) localStorage.setItem(key, JSON.stringify(guides));
    else localStorage.removeItem(key);
  } catch {
    // Storage unavailable (private mode): guides last for this visit.
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
