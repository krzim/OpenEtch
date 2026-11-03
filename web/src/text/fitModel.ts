// Text auto-fit in the UI (crates/text/src/fit.rs): the fit area in the text's own frame, its outline in the field
// frame for the dashed frame on the canvas, the text.update fields that set or clear it, and "Fit to stock". Pure;
// tested in fitModel.test.ts.

import type { Invocation } from "../api/client.ts";
import type { Affine, DocObject, Stock, TextFit, TextGeometry, Vec2 } from "../api/types.ts";
import { TXT, type TextFields } from "./textModel.ts";

export type FitShape = "none" | "box" | "circle";
export type FitMode = TextFit["mode"];

/** The fit area's box in the text's own frame (a circle: its bounding square), as [x0, y0, x1, y1]. */
export function fitRect(g: Pick<TextGeometry, "align_h" | "align_v">, f: TextFit): [number, number, number, number] {
  const w = f.width_mm;
  const h = f.shape === "circle" ? f.width_mm : f.height_mm;
  if (f.shape === "circle") return [-w / 2, -h / 2, w / 2, h / 2];
  const x0 = g.align_h === "left" ? 0 : g.align_h === "middle" ? -w / 2 : -w;
  const y0 = g.align_v === "top" ? -h : g.align_v === "middle" ? -h / 2 : 0;
  return [x0, y0, x0 + w, y0 + h];
}

function apply(t: Affine, p: Vec2): Vec2 {
  return [t[0] * p[0] + t[2] * p[1] + t[4], t[1] * p[0] + t[3] * p[1] + t[5]];
}

/** The fit area's outline in the field frame (a box: 4 corners; a circle: 48 points), or null without a fit. */
export function fitOutline(o: DocObject & { geometry: TextGeometry }): Vec2[] | null {
  const f = o.geometry.fit;
  if (!f) return null;
  const [x0, y0, x1, y1] = fitRect(o.geometry, f);
  if (f.shape === "box") return ([[x0, y0], [x1, y0], [x1, y1], [x0, y1]] as Vec2[]).map((p) => apply(o.transform, p));
  const r = f.width_mm / 2;
  return Array.from({ length: 48 }, (_, i) => {
    const a = (i / 48) * 2 * Math.PI;
    return apply(o.transform, [r * Math.cos(a), r * Math.sin(a)]);
  });
}

/** The `fit` field of text.update for these choices (null when it would change nothing or a size is missing). */
export function fitFields(shape: FitShape, width: number | null, height: number | null, mode: FitMode): TextFields["fit"] | null {
  if (shape === "none") return { shape: "none" };
  if (width === null || !(width > 0)) return null;
  if (shape === "box") {
    if (height === null || !(height > 0)) return null;
    return { shape, width_mm: width, height_mm: height, mode };
  }
  return { shape, width_mm: width, mode: mode === "stretch" ? "contain" : mode };
}

/** Sizes to start a fit with: the text's own bounds rounded up (or 30 x 10 mm without them). */
export function startSize(bounds: { w: number; h: number } | null): { w: number; h: number } {
  const up = (v: number) => Math.max(1, Math.ceil(v));
  return bounds && bounds.w > 0 && bounds.h > 0 ? { w: up(bounds.w), h: up(bounds.h) } : { w: 30, h: 10 };
}

/** Share of the stock the text fills with Fit to stock. */
export const STOCK_FILL = 0.8;

/**
 * "Fit to stock": the text centred on the stock (middle / middle alignment) and fitted into 80 % of it: a circle
 * for a disc, a box for a rectangle (rotated with it when the text is not rotated). One batch, so one undo step.
 */
export function fitToStock(o: DocObject & { geometry: TextGeometry }, stock: Stock, mode: FitMode): Invocation[] {
  const sh = stock.shape;
  const fit: TextFields["fit"] =
    sh.kind === "circle"
      ? { shape: "circle", width_mm: round2(sh.diameter_mm * STOCK_FILL), mode: mode === "stretch" ? "contain" : mode }
      : { shape: "box", width_mm: round2(sh.width_mm * STOCK_FILL), height_mm: round2(sh.height_mm * STOCK_FILL), mode };
  const at: Vec2 = [o.transform[4], o.transform[5]];
  const dx = round4(stock.center[0] - at[0]);
  const dy = round4(stock.center[1] - at[1]);
  const out: Invocation[] = [{ id: TXT.update, params: { id: o.id, fit, align_h: "middle", align_v: "middle" } }];
  if (dx !== 0 || dy !== 0) out.push({ id: "object.transform", params: { ids: [o.id], translate: [dx, dy] } });
  return out;
}

const round2 = (v: number) => Math.round(v * 100) / 100;
const round4 = (v: number) => Math.round(v * 1e4) / 1e4;
