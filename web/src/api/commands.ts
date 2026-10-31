// Builders for the document commands the UI issues. Every id and parameter shape the UI depends on lives here and
// nowhere else, matching the input structs in crates/commands/src/builtin (which use `deny_unknown_fields`, so only
// documented keys may be sent). The UI checks these ids against GET /api/commands at startup and reports any missing.

import type { Invocation } from "./client.ts";
import type { CutSettings, Hatch, StockShape, Vec2 } from "./types.ts";

/** Command ids used by the UI. */
export const CMD = {
  stockDefine: "stock.define",
  shapeRect: "shape.rect",
  shapeEllipse: "shape.ellipse",
  importSvg: "art.import_svg",
  place: "art.place",
  array: "art.array",
  remove: "object.delete",
  transform: "object.transform",
  setLayer: "object.set_layer",
  rename: "object.rename",
  layerAdd: "layer.add",
  layerUpdate: "layer.update",
  docNew: "doc.new",
  importLbrn2: "doc.import_lbrn2",
  exportLbrn2: "doc.export_lbrn2",
} as const;

/** Every id in [`CMD`], for the startup check. */
export const REQUIRED_COMMANDS: string[] = Object.values(CMD);

/** Define (or replace) the stock. */
export function stockDefine(p: {
  name: string;
  shape: StockShape;
  thickness_mm: number;
  material: string | null;
  center: Vec2;
  rotation_deg: number;
}): Invocation {
  return { id: CMD.stockDefine, params: { ...p } };
}

/** Add a rectangle centred at `center` on `layer`. */
export function shapeRect(layer: string, center: Vec2, width_mm: number, height_mm: number, corner_radius_mm = 0): Invocation {
  return { id: CMD.shapeRect, params: { layer, center, width_mm, height_mm, corner_radius_mm } };
}

/** Add an ellipse centred at `center` on `layer`. */
export function shapeEllipse(layer: string, center: Vec2, rx_mm: number, ry_mm: number): Invocation {
  return { id: CMD.shapeEllipse, params: { layer, center, rx_mm, ry_mm } };
}

/** Import SVG text as path objects. `layer` is the default layer for imported art. */
export function importSvg(svg: string, name: string, layer: string | null): Invocation {
  const params: Record<string, unknown> = { svg, name };
  if (layer) params.layer = layer;
  return { id: CMD.importSvg, params };
}

/** Nine-point anchor on the stock. */
export type Anchor =
  | "top_left"
  | "top"
  | "top_right"
  | "left"
  | "center"
  | "right"
  | "bottom_left"
  | "bottom"
  | "bottom_right";

/** Sizing for `art.place`, tagged by `mode` exactly as the command accepts it. */
export type Fit =
  | { mode: "none" }
  | { mode: "contain"; margin_mm: number }
  | { mode: "width"; width_mm: number }
  | { mode: "height"; height_mm: number }
  | { mode: "exact"; width_mm: number; height_mm: number; keep_aspect?: boolean };

/** Place objects on the stock at an anchor (stock frame), then offset by `offset` mm along the stock's axes. */
export function place(ids: string[], anchor: Anchor, fit: Fit, offset: Vec2 = [0, 0]): Invocation {
  const params: Record<string, unknown> = { ids, anchor, fit };
  if (offset[0] !== 0 || offset[1] !== 0) params.offset_mm = offset;
  return { id: CMD.place, params };
}

/** Grid array: `cols` x `rows` cells (including the original) at `pitch` mm, centre to centre. */
export function arrayGrid(ids: string[], cols: number, rows: number, pitch: Vec2): Invocation {
  return { id: CMD.array, params: { ids, cols, rows, pitch_x_mm: pitch[0], pitch_y_mm: pitch[1] } };
}

/** Delete objects. */
export function remove(ids: string[]): Invocation {
  return { id: CMD.remove, params: { ids } };
}

/**
 * Transform objects together in the field frame: scale, mirror and rotate about `about` (default: the selection's
 * bounds centre), then translate. Omitted parts are identity. Scale factors must be > 0.
 */
export function transform(
  ids: string[],
  t: { translate?: Vec2; rotate_deg?: number; scale?: number | Vec2; mirror?: "x" | "y"; about?: "center" | "origin" | Vec2 },
): Invocation {
  return { id: CMD.transform, params: { ids, ...t } };
}

/** Move objects to another layer. */
export function setLayer(ids: string[], layer: string): Invocation {
  return { id: CMD.setLayer, params: { ids, layer } };
}

/** Rename an object. */
export function rename(id: string, name: string): Invocation {
  return { id: CMD.rename, params: { id, name } };
}

/** Add a layer; `color` omitted picks the next palette colour. */
export function layerAdd(name: string, color?: string): Invocation {
  return { id: CMD.layerAdd, params: color ? { name, color } : { name } };
}

/** Partial cut settings, with partial hatch, as `layer.update` accepts them. */
export type CutSettingsPatch = Partial<Omit<CutSettings, "hatch">> & { hatch?: Partial<Hatch> };

/** Update layer properties. Only the given fields change. */
export function layerUpdate(
  id: string,
  patch: { name?: string; color?: string; visible?: boolean; output?: boolean; settings?: CutSettingsPatch },
): Invocation {
  return { id: CMD.layerUpdate, params: { id, ...patch } };
}

/** Replace the document with an empty one (keeps the field size). Undoable. */
export function docNew(name?: string): Invocation {
  return { id: CMD.docNew, params: name ? { name } : {} };
}

/**
 * Import a LightBurn .lbrn2 project given as text. `field_mm` is the galvo field the LightBurn project was laid out
 * for (LightBurn files do not record it); omitted, the engine uses the document's.
 */
export function importLbrn2(content: string, mode: "replace" | "merge", field_mm?: number): Invocation {
  const params: Record<string, unknown> = { content, mode };
  if (field_mm !== undefined) params.field_mm = field_mm;
  return { id: CMD.importLbrn2, params };
}

/** Convert the document to a LightBurn project: the result is `{content, warnings, field_mm}`. Read-only. */
export function exportLbrn2(field_mm?: number): Invocation {
  return { id: CMD.exportLbrn2, params: field_mm !== undefined ? { field_mm } : {} };
}
