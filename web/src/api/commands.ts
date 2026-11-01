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

/**
 * Commands of newer engines. The UI checks for each before using it (`has`) and hides or explains the feature when
 * the engine lacks it, so these are not part of the startup check.
 */
export const OPT = {
  importDxf: "art.import_dxf",
  importImage: "art.import_image",
  duplicate: "object.duplicate",
  align: "object.align",
  distribute: "object.distribute",
  move: "object.move",
  groupCreate: "group.create",
  groupUngroup: "group.ungroup",
  stockMove: "stock.move",
  stockClear: "stock.clear",
  layerDelete: "layer.delete",
  docRename: "doc.rename",
  shapePath: "shape.path",
} as const;

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

/** Import DXF text (mm from `$INSUNITS`, else assumed mm with a warning). */
export function importDxf(content: string, name: string, layer: string | null): Invocation {
  const params: Record<string, unknown> = { content, name };
  if (layer) params.layer = layer;
  return { id: OPT.importDxf, params };
}

/** Import a bitmap (base64) as an image object centred on the field origin. */
export function importImage(data_base64: string, mime: string, name: string, layer: string | null): Invocation {
  const params: Record<string, unknown> = { data_base64, mime, name };
  if (layer) params.layer = layer;
  return { id: OPT.importImage, params };
}

/** Copy objects, offset by `offset` mm (default in place). The result's `created` lists the copies. */
export function duplicate(ids: string[], offset: Vec2 = [0, 0]): Invocation {
  return { id: OPT.duplicate, params: offset[0] === 0 && offset[1] === 0 ? { ids } : { ids, offset } };
}

/** What `object.align` lines the objects up with. */
export type AlignReference =
  | { kind: "last" }
  | { kind: "first" }
  | { kind: "selection" }
  | { kind: "object"; id: string }
  | { kind: "stock" }
  | { kind: "field" };

/** Move objects so their x/y keypoint lands on the reference's same keypoint. Omitted axes do not move. */
export function align(
  ids: string[],
  p: { reference?: AlignReference; x?: "left" | "center" | "right"; y?: "top" | "center" | "bottom"; mode?: "group" | "each" },
): Invocation {
  return { id: OPT.align, params: { ids, ...p } };
}

/** Even out the spacing of objects along an axis. */
export function distribute(ids: string[], axis: "x" | "y", by: "centers" | "gaps"): Invocation {
  return { id: OPT.distribute, params: { ids, axis, by } };
}

/** Group objects (or groups); the result is the new group id. */
export function groupCreate(ids: string[], name?: string): Invocation {
  return { id: OPT.groupCreate, params: name ? { ids, name } : { ids } };
}

/** Remove one level of grouping. */
export function groupUngroup(group: string): Invocation {
  return { id: OPT.groupUngroup, params: { group } };
}

/** Move or rotate the stock: `translate` by [dx, dy], `rotate_by_deg` relative. */
export function stockMove(p: { center?: Vec2; translate?: Vec2; rotation_deg?: number; rotate_by_deg?: number }): Invocation {
  return { id: OPT.stockMove, params: { ...p } };
}

/** Remove the stock. */
export function stockClear(): Invocation {
  return { id: OPT.stockClear, params: {} };
}

/** Delete a layer; a layer that still has objects needs `reassignTo`, the layer they move to. */
export function layerDelete(id: string, reassignTo?: string): Invocation {
  return { id: OPT.layerDelete, params: reassignTo ? { id, reassign_to: reassignTo } : { id } };
}

/** Rename the document. */
export function docRename(name: string): Invocation {
  return { id: OPT.docRename, params: { name } };
}

/** Add a path given as SVG path data in field mm (used to paste cut objects). */
export function shapePath(svg_path_data: string, layer: string, name: string): Invocation {
  return { id: OPT.shapePath, params: { svg_path_data, layer, name } };
}
