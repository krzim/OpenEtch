// Builders for the geometry commands behind the editing UI (node editing, path tools, booleans, generators, arrays,
// draw order, locking, two-point transforms). Shapes match the input structs in crates/commands/src/builtin
// (boolean.rs, offset.rs, path_tools.rs, node.rs, generate.rs, arrays.rs, order.rs, object.rs), which refuse unknown
// keys, so optional keys are only sent when set. These are newer commands: the UI checks `has(id)` before offering
// each one, so they are not part of the startup check in commands.ts.

import type { Invocation } from "./client.ts";
import type { Vec2 } from "./types.ts";

export const GEO = {
  boolean: "path.boolean",
  weld: "path.weld",
  cutShapes: "path.cut_shapes",
  offset: "path.offset",
  breakApart: "path.break_apart",
  autoJoin: "path.auto_join",
  close: "path.close",
  deleteDuplicates: "path.delete_duplicates",
  convert: "path.convert",
  optimize: "path.optimize",
  fillet: "path.fillet",
  trim: "path.trim",
  reverse: "path.reverse",
  nodeList: "node.list",
  nodeMove: "node.move",
  nodeInsert: "node.insert",
  nodeDelete: "node.delete",
  nodeBreak: "node.break",
  nodeJoin: "node.join",
  nodeSetType: "node.set_type",
  nodeSegmentType: "node.segment_type",
  polygon: "shape.polygon",
  line: "shape.line",
  circle: "shape.circle",
  path: "shape.path",
  circularArray: "art.circular_array",
  alongPath: "art.along_path",
  nest: "art.nest",
  order: "object.order",
  lock: "object.lock",
  makeSame: "object.make_same",
  rotateTo: "object.rotate_to",
  scaleTo: "object.scale_to",
  transform: "object.transform",
  duplicate: "object.duplicate",
} as const;

export type BooleanOp = "union" | "subtract" | "intersect" | "xor";

/** Union / subtract (the last id is cut from the others and deleted) / intersect / xor of closed shapes. */
export function pathBoolean(ids: string[], op: BooleanOp): Invocation {
  return { id: GEO.boolean, params: { ids, op } };
}

/** Merge every given closed shape (and each one's own overlaps) into one outline. */
export function weld(ids: string[]): Invocation {
  return { id: GEO.weld, params: { ids } };
}

/** Cut the other shapes where the last id (a closed, ungrouped object, deleted by the cut) crosses them. */
export function cutShapes(ids: string[]): Invocation {
  return { id: GEO.cutShapes, params: { ids } };
}

export type OffsetJoin = "round" | "miter" | "bevel";
export type OffsetDirection = "outward" | "inward" | "both";

export function offset(
  ids: string[],
  p: { distance_mm: number; join?: OffsetJoin; miter_limit?: number; keep_original?: boolean; combine?: boolean; direction?: OffsetDirection },
): Invocation {
  return { id: GEO.offset, params: { ids, ...defined(p) } };
}

export type BreakMode = "auto" | "segments" | "subpaths" | "islands";

export function breakApart(ids: string[], mode?: BreakMode): Invocation {
  return { id: GEO.breakApart, params: mode ? { ids, mode } : { ids } };
}

/** Join open paths whose ends are within `tolerance_mm` (engine default 0.05, LightBurn's). */
export function autoJoin(ids: string[], tolerance_mm?: number): Invocation {
  return { id: GEO.autoJoin, params: tolerance_mm === undefined ? { ids } : { ids, tolerance_mm } };
}

/** Close open paths; with a tolerance only those whose ends are that close. */
export function closePaths(ids: string[], p: { tolerance_mm?: number; mode?: "line" | "move_ends" } = {}): Invocation {
  return { id: GEO.close, params: { ids, ...defined(p) } };
}

/** Remove duplicate shapes among `ids` (the whole document when `ids` is null). */
export function deleteDuplicates(ids: string[] | null, segments = false): Invocation {
  const params: Record<string, unknown> = {};
  if (ids && ids.length) params.ids = ids;
  if (segments) params.segments = true;
  return { id: GEO.deleteDuplicates, params };
}

export function convertToPath(ids: string[]): Invocation {
  return { id: GEO.convert, params: { ids } };
}

/**
 * Optimize Selected Shapes: fit to lines within `tolerance_mm`, to arcs within `arcs_mm`, smooth within `smooth_mm`
 * (each optional, at least one; older engines take only `tolerance_mm`).
 */
export function optimize(ids: string[], p: number | { tolerance_mm?: number; arcs_mm?: number; smooth_mm?: number }): Invocation {
  return { id: GEO.optimize, params: typeof p === "number" ? { ids, tolerance_mm: p } : { ids, ...defined(p) } };
}

/** Round corners: all of them, or node indices from node.list (one object only). Negative radius: concave bite. */
export function fillet(ids: string[], radius_mm: number, corners?: number[]): Invocation {
  return { id: GEO.fillet, params: corners && corners.length ? { ids, radius_mm, corners } : { ids, radius_mm } };
}

/** Remove the piece of object `id` nearest `at` between the nearest crossings. */
export function trim(id: string, at: Vec2, pick_mm?: number): Invocation {
  return { id: GEO.trim, params: pick_mm === undefined ? { id, at } : { id, at, pick_mm } };
}

export function reverse(ids: string[]): Invocation {
  return { id: GEO.reverse, params: { ids } };
}

// --- nodes ---------------------------------------------------------------------------------------------------------

export function nodeList(id: string): Invocation {
  return { id: GEO.nodeList, params: { id } };
}

/** Move node `index` (its handles move along), or with `handle` one of its handles, to `to` (field mm). */
export function nodeMove(id: string, index: number, to: Vec2, handle?: "in" | "out"): Invocation {
  return { id: GEO.nodeMove, params: handle ? { id, index, to, handle } : { id, index, to } };
}

export function nodeInsert(id: string, segment: number, t: number): Invocation {
  return { id: GEO.nodeInsert, params: { id, segment, t } };
}

export function nodeDelete(id: string, indices: number[]): Invocation {
  return { id: GEO.nodeDelete, params: { id, indices } };
}

export function nodeBreak(id: string, index: number): Invocation {
  return { id: GEO.nodeBreak, params: { id, index } };
}

export function nodeJoin(id: string, a: number, b: number): Invocation {
  return { id: GEO.nodeJoin, params: { id, a, b } };
}

export function nodeSetType(id: string, indices: number[], type: "corner" | "smooth" | "symmetric"): Invocation {
  return { id: GEO.nodeSetType, params: { id, indices, type } };
}

export function nodeSegmentType(id: string, segment: number, type: "line" | "curve"): Invocation {
  return { id: GEO.nodeSegmentType, params: { id, segment, type } };
}

// --- generators ----------------------------------------------------------------------------------------------------

export function polygon(p: {
  sides: number;
  radius_mm: number;
  center: Vec2;
  rotation_deg?: number;
  star_inner_ratio?: number;
  corner_radius_mm?: number;
  layer?: string | null;
  name?: string;
  /** Keep it a parametric polygon (Shape Properties can change it); engines with `shape.update`. */
  parametric?: boolean;
}): Invocation {
  const { layer, ...rest } = p;
  const params: Record<string, unknown> = defined(rest);
  if (layer) params.layer = layer;
  if (params.corner_radius_mm === 0) delete params.corner_radius_mm;
  if (params.rotation_deg === 0) delete params.rotation_deg;
  if (!params.parametric) delete params.parametric;
  return { id: GEO.polygon, params };
}

export function line(points: Vec2[], closed: boolean, layer?: string | null, name?: string): Invocation {
  const params: Record<string, unknown> = { points };
  if (closed) params.closed = true;
  if (layer) params.layer = layer;
  if (name) params.name = name;
  return { id: GEO.line, params };
}

export function pathShape(svg_path_data: string, layer?: string | null, name?: string): Invocation {
  const params: Record<string, unknown> = { svg_path_data };
  if (layer) params.layer = layer;
  if (name) params.name = name;
  return { id: GEO.path, params };
}

// --- arrays and nesting ----------------------------------------------------------------------------------------------

export function circularArray(ids: string[], p: { count: number; center: Vec2; angle_deg?: number; rotate_copies?: boolean }): Invocation {
  const params: Record<string, unknown> = { ids, count: p.count, center: p.center };
  if (p.angle_deg !== undefined && p.angle_deg !== 360) params.angle_deg = p.angle_deg;
  if (p.rotate_copies === false) params.rotate_copies = false;
  return { id: GEO.circularArray, params };
}

export function alongPath(ids: string[], path_id: string, p: { count?: number; spacing_mm?: number; rotate_to_path?: boolean }): Invocation {
  const params: Record<string, unknown> = { ids, path_id };
  if (p.count !== undefined) params.count = p.count;
  else if (p.spacing_mm !== undefined) params.spacing_mm = p.spacing_mm;
  if (p.rotate_to_path === false) params.rotate_to_path = false;
  return { id: GEO.alongPath, params };
}

export type NestRegion = "stock" | "field" | { min: Vec2; max: Vec2 } | { object: string };

export function nest(
  ids: string[],
  region: NestRegion,
  p: { spacing_mm?: number; edge_mm?: number; rotations?: number[]; mode?: "boxes" | "shapes"; resolution_mm?: number } = {},
): Invocation {
  return { id: GEO.nest, params: { ids, region, ...defined(p) } };
}

// --- arrange -------------------------------------------------------------------------------------------------------

export type OrderTo = "front" | "back" | "forward" | "backward";

export function order(ids: string[], to: OrderTo): Invocation {
  return { id: GEO.order, params: { ids, to } };
}

export function lock(ids: string[], locked: boolean): Invocation {
  return { id: GEO.lock, params: { ids, locked } };
}

export function makeSame(ids: string[], dimension: "width" | "height" | "both", keep_aspect = false): Invocation {
  return { id: GEO.makeSame, params: keep_aspect && dimension !== "both" ? { ids, dimension, keep_aspect } : { ids, dimension } };
}

/** Rotate about `pivot` so the direction pivot→from turns to pivot→to. */
export function rotateTo(ids: string[], pivot: Vec2, from: Vec2, to: Vec2): Invocation {
  return { id: GEO.rotateTo, params: { ids, pivot, from, to } };
}

/** Scale uniformly about `pivot` by |to - pivot| / |from - pivot| (and turn with `rotate`). */
export function scaleTo(ids: string[], pivot: Vec2, from: Vec2, to: Vec2, rotate = false): Invocation {
  return { id: GEO.scaleTo, params: rotate ? { ids, pivot, from, to, rotate } : { ids, pivot, from, to } };
}

/** object.transform with the newer parts (shear, mirror across a line) and scale. */
export function transformEx(
  ids: string[],
  t: { scale?: number | Vec2; shear_deg?: Vec2; mirror_line?: { a: Vec2; b: Vec2 }; rotate_deg?: number; about?: "center" | "origin" | Vec2; keep_copy?: boolean },
): Invocation {
  return { id: GEO.transform, params: { ids, ...defined(t) } };
}

// --- results -------------------------------------------------------------------------------------------------------

function strings(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

/**
 * The objects an edit leaves to select (LightBurn selects the result): created and affected ids, without deleted ones.
 * A batch answer (`{results: [...]}`) is read result by result.
 */
export function resultIds(r: unknown): string[] {
  if (!r || typeof r !== "object") return [];
  const o = r as Record<string, unknown>;
  if (Array.isArray(o.results)) {
    const all = o.results.flatMap((x) => resultIds(x));
    return [...new Set(all)];
  }
  const deleted = new Set(strings(o.deleted));
  const out: string[] = [];
  for (const id of [...strings(o.created), ...strings(o.affected)]) if (!deleted.has(id) && !out.includes(id)) out.push(id);
  return out;
}

/** `p` without undefined keys (absent means "default"; the commands refuse unknown keys). */
function defined<T extends object>(p: T): Record<string, unknown> {
  return Object.fromEntries(Object.entries(p).filter(([, v]) => v !== undefined));
}
