// Command builders and HTTP calls of the design-engine UI (src/design/): the commands in crates/commands/src/builtin/
// design.rs, construct.rs and art_library.rs, and the Art Library routes of crates/engine/src/artlib.rs (module docs).
// The commands refuse unknown keys, so optional keys are only sent when set. Same auth and error handling as
// api/client.ts (bearer token, ApiError with the engine's `error` text).

import { ApiError, authToken, UI_ORIGIN, type Invocation } from "../api/client.ts";
import type { Vec2 } from "../api/types.ts";
import type { PageAnchor } from "./model.ts";

export const DCMD = {
  shapeUpdate: "shape.update",
  extend: "node.extend",
  warp: "path.warp",
  tangentCandidates: "art.tangent_circle_candidates",
  tangentCircle: "art.tangent_circle",
  rubberBand: "art.rubber_band",
  toolpathToPath: "art.toolpath_to_path",
  selectOpen: "select.open_shapes",
  selectOpenOnFill: "select.open_on_fill",
  selectContained: "select.contained",
  pagePosition: "object.move_to_page_position",
  distribute: "object.distribute",
  guideAdd: "guide.add",
  guideMove: "guide.move",
  guideDelete: "guide.delete",
  curveTolerance: "doc.set_curve_tolerance",
  artInsert: "art.insert",
  array: "art.array",
} as const;

export function shapeUpdate(id: string, patch: Record<string, number | null>): Invocation {
  return { id: DCMD.shapeUpdate, params: { id, ...patch } };
}

/** Extend the open end nearest `at` straight on to the nearest line (Edit Nodes "E"). */
export function extend(id: string, at: Vec2, pick_mm: number): Invocation {
  return { id: DCMD.extend, params: { id, at, pick_mm } };
}

export function tangentCandidates(ids: string[]): Invocation {
  return { id: DCMD.tangentCandidates, params: { ids } };
}

export function tangentCircle(ids: string[], pick: number[] | "all", layer?: string | null): Invocation {
  const params: Record<string, unknown> = { ids, pick };
  if (layer) params.layer = layer;
  return { id: DCMD.tangentCircle, params };
}

export function rubberBand(ids: string[], layer?: string | null): Invocation {
  return { id: DCMD.rubberBand, params: layer ? { ids, layer } : { ids } };
}

export function toolpathToPath(ids: string[] | null, layer?: string | null): Invocation {
  const params: Record<string, unknown> = {};
  if (ids && ids.length) params.ids = ids;
  if (layer) params.layer = layer;
  return { id: DCMD.toolpathToPath, params };
}

export function selectQuery(id: typeof DCMD.selectOpen | typeof DCMD.selectOpenOnFill | typeof DCMD.selectContained, ids: string[] | null): Invocation {
  return { id, params: ids && ids.length ? { ids } : {} };
}

export function pagePosition(ids: string[], anchor: PageAnchor, margin_mm: number, reference: "workspace" | "stock"): Invocation {
  const params: Record<string, unknown> = { ids, anchor };
  if (margin_mm) params.margin_mm = margin_mm;
  if (reference !== "workspace") params.reference = reference;
  return { id: DCMD.pagePosition, params };
}

/** Move Horizontally / Vertically Together: pack the selection `spacing_mm` apart along an axis. */
export function distributeSpacing(ids: string[], axis: "x" | "y", spacing_mm: number): Invocation {
  return { id: DCMD.distribute, params: { ids, axis, by: "spacing", spacing_mm } };
}

export function guideAdd(axis: "x" | "y", position_mm: number): Invocation {
  return { id: DCMD.guideAdd, params: { axis, position_mm } };
}

export function guideMove(id: string, position_mm: number, axis?: "x" | "y"): Invocation {
  return { id: DCMD.guideMove, params: axis ? { id, position_mm, axis } : { id, position_mm } };
}

/** Delete guides by id, or every guide when `ids` is null. */
export function guideDelete(ids: string[] | null): Invocation {
  return { id: DCMD.guideDelete, params: ids ? { ids } : {} };
}

export function curveTolerance(tolerance_mm: number | null): Invocation {
  return { id: DCMD.curveTolerance, params: { tolerance_mm } };
}

export function warp(params: Record<string, unknown>): Invocation {
  return { id: DCMD.warp, params };
}

// ---- Art Library HTTP -------------------------------------------------------------------------------------------------

export interface ArtLibraryRow {
  name: string;
  items: number;
  modified_unix_s?: number;
  size?: number;
}

export interface ArtItem {
  id: string;
  name: string;
  created_unix_s?: number;
  size_mm?: [number, number];
  objects?: number;
  /** URL of the PNG thumbnail (needs the Authorization header). */
  thumbnail?: string;
}

function headers(accept: string, contentType?: string): Record<string, string> {
  const h: Record<string, string> = { Accept: accept };
  if (contentType) h["Content-Type"] = contentType;
  const token = authToken();
  if (token) h.Authorization = `Bearer ${token}`;
  return h;
}

async function call<T = unknown>(method: string, path: string, body?: unknown, raw?: { body: BodyInit; type: string }): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, {
      method,
      headers: headers("application/json", raw ? raw.type : body === undefined ? undefined : "application/json"),
      body: raw ? raw.body : body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  const text = await res.text();
  let data: unknown = undefined;
  try {
    data = text ? JSON.parse(text) : undefined;
  } catch {
    data = text;
  }
  const obj = data && typeof data === "object" ? (data as { ok?: unknown; error?: unknown; code?: unknown }) : null;
  if (!res.ok || obj?.ok === false) {
    const msg = obj && "error" in obj ? String(obj.error) : text || res.statusText;
    throw new ApiError(msg, res.status, { code: typeof obj?.code === "string" ? obj.code : undefined, detail: msg });
  }
  return data as T;
}

const lib = (name: string) => `/api/art/libraries/${encodeURIComponent(name)}`;

export const artApi = {
  /** `formats` lists the library files the engine loads (engines before .lbart support leave it out). */
  list: () => call<{ libraries?: ArtLibraryRow[]; formats?: string[] }>("GET", "/api/art/libraries"),
  create: (name: string) => call("POST", "/api/art/libraries", { name }),
  load: (file: File, overwrite = false) =>
    call<{ name?: string; items?: number; warnings?: string[] }>(
      "POST",
      `/api/art/libraries/load?name=${encodeURIComponent(file.name)}${overwrite ? "&overwrite=true" : ""}`,
      undefined,
      { body: file, type: /\.lbart$/i.test(file.name) ? "application/octet-stream" : "application/json" },
    ),
  view: (name: string) => call<{ name: string; items?: ArtItem[] }>("GET", lib(name)),
  rename: (name: string, to: string) => call("POST", `${lib(name)}/rename`, { to }),
  remove: (name: string) => call("DELETE", lib(name)),
  addSelection: (name: string, ids: string[], itemName?: string) => call<{ item?: ArtItem }>("POST", `${lib(name)}/items`, itemName ? { ids, name: itemName } : { ids }),
  importFile: (name: string, file: File, format: string) =>
    call<{ item?: ArtItem; items?: ArtItem[]; warnings?: string[] }>("POST", `${lib(name)}/import?format=${encodeURIComponent(format)}&name=${encodeURIComponent(file.name)}`, undefined, {
      body: file,
      type: file.type || "application/octet-stream",
    }),
  renameItem: (name: string, id: string, to: string) => call("PUT", `${lib(name)}/items/${encodeURIComponent(id)}`, { name: to }),
  deleteItem: (name: string, id: string) => call("DELETE", `${lib(name)}/items/${encodeURIComponent(id)}`),
  copyItem: (name: string, id: string, to: string, move: boolean) => call("POST", `${lib(name)}/items/${encodeURIComponent(id)}/copy`, move ? { to, move } : { to }),
  insert: (name: string, id: string, center: Vec2 | null, layer?: string | null) => {
    const body: Record<string, unknown> = { origin: UI_ORIGIN };
    if (center) body.center = center;
    if (layer) body.layer = layer;
    return call<{ result?: unknown; revision?: number }>("POST", `${lib(name)}/items/${encodeURIComponent(id)}/insert`, body);
  },
  /** A thumbnail as an object URL (fetched with the Authorization header). */
  thumbnail: async (url: string): Promise<string> => {
    const res = await fetch(url, { headers: headers("image/png") });
    if (!res.ok) throw new ApiError(`thumbnail: ${res.status}`, res.status);
    return URL.createObjectURL(await res.blob());
  },
  /** The library file: OpenEtch's `.oeart`, or LightBurn's `.lbart`. */
  download: async (name: string, format: "oeart" | "lbart" = "oeart"): Promise<Blob> => {
    const res = await fetch(`${lib(name)}/download${format === "lbart" ? "?format=lbart" : ""}`, { headers: headers("*/*") });
    if (!res.ok) throw new ApiError(`download: ${res.status} ${await res.text()}`, res.status);
    return res.blob();
  },
};
