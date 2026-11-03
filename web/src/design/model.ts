// Pure logic of the design-engine UI (src/design/): Warp / Deform handle maths (the same maps as crates/geom/src/
// warp.rs, for the live preview), the nine page positions, the grid array layout (crates/commands/src/builtin/
// array.rs, for the dialog's preview), the clipboard (Art Library fragments for the internal clipboard, the selection
// as SVG for other programs, and what a system paste holds), guides kept in the document (and moving the ones an
// older version kept in this browser), Art Library file formats, tangent-circle picking and Shape Properties
// parameters. No DOM; tested in design.test.ts.

import type { DocGuide, DocObject, GDocument, Geometry, Group, Vec2, VirtualArray } from "../api/types.ts";
import { flattenCmds, geometryCmds, objectBounds, toPathData, unionBoxes, type Box, type Polyline } from "../geometry.ts";
import { engineFormatOf, extOf } from "../files/fileFormats.ts";
import { topGroup } from "../selection.ts";
import type { Guide } from "../guides.ts";
import { tr } from "../workspace/tr.ts";

// ---- Warp and Deform ----------------------------------------------------------------------------------------------

/** "perspective": straight lines stay straight; "envelope": bilinear, edges stay straight; "grid": 4 x 4 Deform. */
export type WarpKind = "perspective" | "envelope" | "grid";

/**
 * The handles that change nothing: for perspective and envelope the box's corners [bottom_left, bottom_right,
 * top_right, top_left]; for the grid 16 handles, row by row from the bottom, left to right, at thirds.
 */
export function identityHandles(box: Box, kind: WarpKind): Vec2[] {
  if (kind !== "grid") {
    return [
      [box.x0, box.y0],
      [box.x1, box.y0],
      [box.x1, box.y1],
      [box.x0, box.y1],
    ];
  }
  const out: Vec2[] = [];
  const w = box.x1 - box.x0;
  const h = box.y1 - box.y0;
  for (let r = 0; r < 4; r++) for (let c = 0; c < 4; c++) out.push([box.x0 + (w * c) / 3, box.y0 + (h * r) / 3]);
  return out;
}

/** The homography taking the unit square's corners to `q` (Heckbert), row-major 3x3. */
export function squareToQuad(q: readonly Vec2[]): number[] {
  const [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = q;
  const sx = x0 - x1 + x2 - x3;
  const sy = y0 - y1 + y2 - y3;
  if (Math.abs(sx) < 1e-15 && Math.abs(sy) < 1e-15) return [x1 - x0, x3 - x0, x0, y1 - y0, y3 - y0, y0, 0, 0, 1];
  const [dx1, dx2, dy1, dy2] = [x1 - x2, x3 - x2, y1 - y2, y3 - y2];
  const den = dx1 * dy2 - dx2 * dy1;
  const g = (sx * dy2 - dx2 * sy) / den;
  const h = (dx1 * sy - sx * dy1) / den;
  return [x1 - x0 + g * x1, x3 - x0 + h * x3, x0, y1 - y0 + g * y1, y3 - y0 + h * y3, y0, g, h, 1];
}

function bern(t: number): [number, number, number, number] {
  const m = 1 - t;
  return [m * m * m, 3 * t * m * m, 3 * t * t * m, t * t * t];
}

/** Why a perspective cannot use these corners (they must form a convex quadrilateral in order), or null. */
export function perspectiveProblem(q: readonly Vec2[]): string | null {
  const cross = (i: number) => {
    const [a, b, c] = [q[i], q[(i + 1) % 4], q[(i + 2) % 4]];
    return (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
  };
  const s = [0, 1, 2, 3].map(cross);
  return s.every((v) => v > 1e-12) || s.every((v) => v < -1e-12) ? null : tr("dz.m.convex");
}

/** A point mapper for `kind` through `handles` fitted to `box` (the engine's `Mapper::map`), or null when unusable. */
export function warpMapper(box: Box, kind: WarpKind, handles: readonly Vec2[]): ((p: Vec2) => Vec2) | null {
  const w = box.x1 - box.x0;
  const h = box.y1 - box.y0;
  if (!(w > 0 && h > 0)) return null;
  if (!handles.every((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]))) return null;
  const uv = (p: Vec2): Vec2 => [(p[0] - box.x0) / w, (p[1] - box.y0) / h];
  if (kind === "perspective") {
    if (handles.length !== 4 || perspectiveProblem(handles)) return null;
    const m = squareToQuad(handles);
    return (p) => {
      const [u, v] = uv(p);
      const d = m[6] * u + m[7] * v + m[8];
      return [(m[0] * u + m[1] * v + m[2]) / d, (m[3] * u + m[4] * v + m[5]) / d];
    };
  }
  if (kind === "envelope") {
    if (handles.length !== 4) return null;
    const q = handles;
    return (p) => {
      const [u, v] = uv(p);
      const a = (1 - u) * (1 - v);
      const b = u * (1 - v);
      const c = u * v;
      const d = (1 - u) * v;
      return [a * q[0][0] + b * q[1][0] + c * q[2][0] + d * q[3][0], a * q[0][1] + b * q[1][1] + c * q[2][1] + d * q[3][1]];
    };
  }
  if (handles.length !== 16) return null;
  return (p) => {
    const [u, v] = uv(p);
    const bu = bern(u);
    const bv = bern(v);
    let x = 0;
    let y = 0;
    for (let r = 0; r < 4; r++)
      for (let c = 0; c < 4; c++) {
        const k = bv[r] * bu[c];
        x += handles[r * 4 + c][0] * k;
        y += handles[r * 4 + c][1] * k;
      }
    return [x, y];
  };
}

/** Polylines in the field frame densified so no piece is longer than `step` mm (bent lines show as bent). */
export function densify(polys: Polyline[], step: number): Polyline[] {
  return polys.map((pl) => {
    const pts: Vec2[] = [];
    const n = pl.pts.length;
    const segs = pl.closed ? n : n - 1;
    for (let i = 0; i < segs; i++) {
      const a = pl.pts[i];
      const b = pl.pts[(i + 1) % n];
      const k = Math.min(64, Math.max(1, Math.ceil(Math.hypot(b[0] - a[0], b[1] - a[1]) / step)));
      for (let j = 0; j < k; j++) pts.push([a[0] + ((b[0] - a[0]) * j) / k, a[1] + ((b[1] - a[1]) * j) / k]);
    }
    if (!pl.closed && n) pts.push(pl.pts[n - 1]);
    return { pts, closed: pl.closed };
  });
}

/** The selection's outlines warped (for the preview), as SVG path data in the field frame; images show their frame. */
export function warpPreview(objects: readonly DocObject[], box: Box, kind: WarpKind, handles: readonly Vec2[]): string | null {
  const map = warpMapper(box, kind, handles);
  if (!map) return null;
  const step = Math.max(box.x1 - box.x0, box.y1 - box.y0) / 48;
  let d = "";
  let budget = 40000;
  for (const o of objects) {
    for (const pl of densify(flattenCmds(geometryCmds(o.geometry), o.transform, 8), step)) {
      if (budget <= 0) return d;
      budget -= pl.pts.length;
      const q = pl.pts.map(map);
      if (q.length < 2) continue;
      d += `M${q.map((p) => `${r4(p[0])},${r4(p[1])}`).join("L")}${pl.closed ? "Z" : ""}`;
    }
  }
  return d;
}

/**
 * The handle that mirrors `index` across the box's vertical centre line ("h": LightBurn's Shift, horizontal symmetry)
 * or its horizontal centre line ("v": Alt, vertical symmetry); null for none (it is its own mirror).
 */
export function symmetricHandle(kind: WarpKind, index: number, axis: "h" | "v"): number | null {
  if (kind !== "grid") {
    // [bl, br, tr, tl]
    const h = [1, 0, 3, 2];
    const v = [3, 2, 1, 0];
    return (axis === "h" ? h : v)[index] ?? null;
  }
  const r = Math.floor(index / 4);
  const c = index % 4;
  const j = axis === "h" ? r * 4 + (3 - c) : (3 - r) * 4 + c;
  return j === index ? null : j;
}

/**
 * Move handle `index` to `to`; with symmetry the mirrored handle moves the mirrored way (about the box's centre lines,
 * from where it started), so the warp stays symmetric.
 */
export function moveHandle(
  handles: readonly Vec2[],
  base: readonly Vec2[],
  kind: WarpKind,
  index: number,
  to: Vec2,
  sym: { h: boolean; v: boolean },
): Vec2[] {
  const out = handles.map((p) => [p[0], p[1]] as Vec2);
  const d: Vec2 = [to[0] - base[index][0], to[1] - base[index][1]];
  out[index] = to;
  const move = (j: number | null, sx: number, sy: number) => {
    if (j === null) return;
    out[j] = [base[j][0] + d[0] * sx, base[j][1] + d[1] * sy];
  };
  if (sym.h) move(symmetricHandle(kind, index, "h"), -1, 1);
  if (sym.v) move(symmetricHandle(kind, index, "v"), 1, -1);
  if (sym.h && sym.v) {
    const a = symmetricHandle(kind, index, "h");
    move(a === null ? null : symmetricHandle(kind, a, "v"), -1, -1);
  }
  return out;
}

/** The `path.warp` parameters for the handles. */
export function warpParams(ids: string[], kind: WarpKind, handles: readonly Vec2[]): Record<string, unknown> {
  const r = (p: Vec2): Vec2 => [r4(p[0]), r4(p[1])];
  if (kind === "grid") return { ids, mode: "grid", grid: [0, 1, 2, 3].map((row) => handles.slice(row * 4, row * 4 + 4).map(r)) };
  return { ids, mode: kind, corners: handles.map(r) };
}

/** Whether every handle is still where it started. */
export function handlesUnchanged(a: readonly Vec2[], b: readonly Vec2[]): boolean {
  return a.length === b.length && a.every((p, i) => Math.abs(p[0] - b[i][0]) < 1e-9 && Math.abs(p[1] - b[i][1]) < 1e-9);
}

// ---- page positions -------------------------------------------------------------------------------------------------

export type PageAnchor = "top_left" | "top" | "top_right" | "left" | "center" | "right" | "bottom_left" | "bottom" | "bottom_right";

/** The nine positions in reading order (top row first), as the Move Selected Objects window shows them. */
export const PAGE_ANCHORS: PageAnchor[] = ["top_left", "top", "top_right", "left", "center", "right", "bottom_left", "bottom", "bottom_right"];

/** Where `object.move_to_page_position` would put the selection box `b` in `page` (for tests and the hint). */
export function pagePositionDelta(b: Box, page: Box, anchor: PageAnchor, margin: number): Vec2 {
  const dir = (s: string): [number, number] => [s.includes("left") ? -1 : s.includes("right") ? 1 : 0, s.includes("top") ? 1 : s.includes("bottom") ? -1 : 0];
  const [dx, dy] = dir(anchor);
  const pick = (d: number, lo: number, hi: number, blo: number, bhi: number) =>
    d < 0 ? lo + margin - blo : d > 0 ? hi - margin - bhi : (lo + hi) / 2 - (blo + bhi) / 2;
  return [pick(dx, page.x0, page.x1, b.x0, b.x1), pick(dy, page.y0, page.y1, b.y0, b.y1)];
}

// ---- grid array -----------------------------------------------------------------------------------------------------

export interface ArrayOptions {
  cols: number;
  rows: number;
  pitchX: number;
  pitchY: number;
  gapX: boolean;
  gapY: boolean;
  reverseX: boolean;
  reverseY: boolean;
  /** Odd rows shift along X: mm, "half" or null. */
  rowShift: number | "half" | null;
  colShift: number | "half" | null;
  mirrorCols: "x" | "y" | null;
  mirrorRows: "x" | "y" | null;
  /** LightBurn's Total Width / Height: the counts as many as fit in this many mm (override cols / rows). */
  totalWidth?: number | null;
  totalHeight?: number | null;
  /** LightBurn's Random Orientation: copies turned by random angles from this seed (null: off). */
  seed?: number | null;
  /** LightBurn's Auto-Increment Variable Text: each copy's serial/CSV Offset raised by n times this (0: off). */
  autoIncrement?: number;
  /** LightBurn's Group Results (not with a virtual array). */
  groupResults?: boolean;
  /** LightBurn's Create Virtual Array: linked clones. */
  virtual?: boolean;
}

export const MAX_ARRAY_CELLS = 10000;

/** As many cells as fit in `total` mm (the engine's fit_count): the selection, the steps and a shift counted. */
export function fitCount(total: number, size: number, step: number, shift: number): number | null {
  const room = total - size - Math.abs(shift);
  if (!(total > 0) || room < -1e-9) return null;
  if (Math.abs(step) < 1e-9) return 1;
  return Math.min(MAX_ARRAY_CELLS, Math.floor(Math.max(0, room) / Math.abs(step) + 1e-9) + 1);
}

/**
 * The options with Total Width / Height turned into counts as the engine does (two rounds, since a shift widens the
 * grid only when there is a second row or column), or null when a total is smaller than the selection.
 */
export function resolveCounts(o: ArrayOptions, box: Box): ArrayOptions | null {
  if (o.totalWidth == null && o.totalHeight == null) return o;
  const w = box.x1 - box.x0;
  const h = box.y1 - box.y0;
  const sx = o.gapX ? w + o.pitchX : o.pitchX;
  const sy = o.gapY ? h + o.pitchY : o.pitchY;
  const rs = o.rowShift === "half" ? w / 2 : (o.rowShift ?? 0);
  const cs = o.colShift === "half" ? h / 2 : (o.colShift ?? 0);
  let { cols, rows } = o;
  for (let k = 0; k < 2; k++) {
    if (o.totalWidth != null) {
      const n = fitCount(o.totalWidth, w, sx, rows > 1 ? rs : 0);
      if (n === null) return null;
      cols = n;
    }
    if (o.totalHeight != null) {
      const n = fitCount(o.totalHeight, h, sy, cols > 1 ? cs : 0);
      if (n === null) return null;
      rows = n;
    }
  }
  return { ...o, cols, rows };
}

/** Each cell's offset and flips, the engine's art.array layout (row 0, column 0 is the original). */
export function arrayCells(o: ArrayOptions, box: Box): { row: number; col: number; d: Vec2; flipX: boolean; flipY: boolean }[] {
  const w = box.x1 - box.x0;
  const h = box.y1 - box.y0;
  const sx = (o.gapX ? w + o.pitchX : o.pitchX) * (o.reverseX ? -1 : 1);
  const sy = (o.gapY ? h + o.pitchY : o.pitchY) * (o.reverseY ? -1 : 1);
  const rs = o.rowShift === "half" ? w / 2 : (o.rowShift ?? 0);
  const cs = o.colShift === "half" ? h / 2 : (o.colShift ?? 0);
  const out = [];
  for (let r = 0; r < o.rows; r++)
    for (let c = 0; c < o.cols; c++) {
      let fx = false;
      let fy = false;
      if (c % 2 === 1 && o.mirrorCols) o.mirrorCols === "x" ? (fx = !fx) : (fy = !fy);
      if (r % 2 === 1 && o.mirrorRows) o.mirrorRows === "x" ? (fx = !fx) : (fy = !fy);
      out.push({ row: r, col: c, d: [c * sx + (r % 2 === 1 ? rs : 0), r * sy + (c % 2 === 1 ? cs : 0)] as Vec2, flipX: fx, flipY: fy });
    }
  return out;
}

/** Why the options cannot make an array, or null. */
export function arrayProblem(o: ArrayOptions, box: Box | null): string | null {
  if (!box) return tr("dz.m.selectToCopy");
  if (!(Number.isInteger(o.cols) && Number.isInteger(o.rows) && o.cols >= 1 && o.rows >= 1)) return tr("dz.m.wholeCounts");
  if (o.cols * o.rows > MAX_ARRAY_CELLS) return tr("dz.m.tooMany", { max: MAX_ARRAY_CELLS });
  if (o.cols * o.rows === 1) return tr("dz.m.justOriginal");
  if (![o.pitchX, o.pitchY].every(Number.isFinite)) return tr("dz.m.spacingNumbers");
  const sx = o.gapX ? box.x1 - box.x0 + o.pitchX : o.pitchX;
  const sy = o.gapY ? box.y1 - box.y0 + o.pitchY : o.pitchY;
  if ((o.cols > 1 && Math.abs(sx) < 1e-9) || (o.rows > 1 && Math.abs(sy) < 1e-9)) return tr("dz.m.zeroStep");
  return null;
}

/** The `art.array` parameters (only what differs from the defaults is sent). */
export function arrayParams(ids: string[], o: ArrayOptions): Record<string, unknown> {
  const p: Record<string, unknown> = { ids, cols: o.cols, rows: o.rows, pitch_x_mm: o.pitchX, pitch_y_mm: o.pitchY };
  if (o.gapX) p.spacing_x = "gap";
  if (o.gapY) p.spacing_y = "gap";
  if (o.reverseX) p.reverse_x = true;
  if (o.reverseY) p.reverse_y = true;
  if (o.rowShift !== null && o.rowShift !== 0) p.row_shift = o.rowShift;
  if (o.colShift !== null && o.colShift !== 0) p.col_shift = o.colShift;
  if (o.mirrorCols) p.mirror_alternate_cols = o.mirrorCols;
  if (o.mirrorRows) p.mirror_alternate_rows = o.mirrorRows;
  if (o.totalWidth != null) p.total_width_mm = o.totalWidth;
  if (o.totalHeight != null) p.total_height_mm = o.totalHeight;
  if (o.seed != null) p.random_orientation_seed = o.seed;
  if (o.autoIncrement) p.auto_increment = o.autoIncrement;
  if (o.virtual) p.virtual = true;
  else if (o.groupResults) p.group_results = true;
  return p;
}

/** `art.array` laying out the existing virtual array `array` again with `o` (LightBurn's Edit Array). */
export function editArrayParams(array: string, o: ArrayOptions): Record<string, unknown> {
  const { ids: _ids, virtual: _v, group_results: _g, ...rest } = arrayParams([], { ...o, virtual: false, groupResults: false });
  return { ids: [], edit_array: array, ...rest };
}

/** The virtual array an object belongs to (as an original or a clone), or null. */
export function virtualArrayOf(doc: GDocument | null, ids: readonly string[]): VirtualArray | null {
  const want = new Set(ids);
  for (const id of ids) {
    const g = doc?.objects.find((o) => o.id === id)?.group;
    if (g) want.add(g);
  }
  return doc?.virtual_arrays?.find((a) => a.sources.some((s) => want.has(s)) || a.clones.some((c) => want.has(c))) ?? null;
}

/** The dialog options of a virtual array's kept settings (the engine's Grid), for Edit Array. */
export function arrayOptionsOf(settings: Record<string, unknown>): ArrayOptions {
  const n = (v: unknown, d: number) => (typeof v === "number" && Number.isFinite(v) ? v : d);
  const shift = (v: unknown): number | "half" | null => (v === "half" ? "half" : typeof v === "number" ? v : null);
  const mirror = (v: unknown): "x" | "y" | null => (v === "x" || v === "y" ? v : null);
  return {
    cols: n(settings.cols, 1),
    rows: n(settings.rows, 1),
    pitchX: n(settings.pitch_x_mm, 0),
    pitchY: n(settings.pitch_y_mm, 0),
    gapX: settings.spacing_x === "gap",
    gapY: settings.spacing_y === "gap",
    reverseX: settings.reverse_x === true,
    reverseY: settings.reverse_y === true,
    rowShift: shift(settings.row_shift),
    colShift: shift(settings.col_shift),
    mirrorCols: mirror(settings.mirror_alternate_cols),
    mirrorRows: mirror(settings.mirror_alternate_rows),
    seed: typeof settings.random_orientation_seed === "number" ? settings.random_orientation_seed : null,
    autoIncrement: n(settings.auto_increment, 0),
    virtual: true,
  };
}

/** How many cells end up partly outside the centred workspace `ws` (W x H). */
export function cellsOutside(o: ArrayOptions, box: Box, ws: Vec2): number {
  const [hx, hy] = [ws[0] / 2, ws[1] / 2];
  let n = 0;
  for (const c of arrayCells(o, box)) if (box.x0 + c.d[0] < -hx || box.x1 + c.d[0] > hx || box.y0 + c.d[1] < -hy || box.y1 + c.d[1] > hy) n++;
  return n;
}

// ---- clipboard --------------------------------------------------------------------------------------------------------

/** An Art Library fragment (commands::ArtFragment), the internal clipboard's content. */
export interface ArtFragment {
  format: "openetch-art";
  objects: DocObject[];
  groups: Group[];
  layers: { id: string; name: string; color: string }[];
  /** Cut properties by object id (tabs, start point, priority, power scale): a pasted copy keeps them. */
  object_options?: Record<string, unknown>;
}

/** The fragment's `object_options` for just these objects (absent when none has any). */
function optionsFor(all: Record<string, unknown> | undefined, objects: readonly DocObject[]): Record<string, unknown> | undefined {
  if (!all) return undefined;
  const out: Record<string, unknown> = {};
  for (const o of objects) if (all[o.id] !== undefined) out[o.id] = JSON.parse(JSON.stringify(all[o.id]));
  return Object.keys(out).length ? out : undefined;
}

/** Every group `id` sits in, innermost first. */
function groupChain(doc: GDocument, id: string | null | undefined): string[] {
  const parents = new Map((doc.groups ?? []).map((g) => [g.id, g.parent ?? null]));
  const out: string[] = [];
  let g = id ?? null;
  while (g && !out.includes(g)) {
    out.push(g);
    g = parents.get(g) ?? null;
  }
  return out;
}

/** The objects `ids` of `doc` as a fragment (as `art_fragment` builds it): document order, their groups, their layers. */
export function fragmentOf(doc: GDocument, ids: readonly string[]): ArtFragment | null {
  const want = new Set(ids);
  const objects = doc.objects.filter((o) => want.has(o.id));
  if (!objects.length) return null;
  const gids = new Set(objects.flatMap((o) => groupChain(doc, o.group)));
  const lids = new Set(objects.map((o) => o.layer));
  const all = (doc as { object_options?: unknown }).object_options;
  const options = optionsFor(all && typeof all === "object" ? (all as Record<string, unknown>) : undefined, objects);
  return {
    format: "openetch-art",
    objects: objects.map((o) => JSON.parse(JSON.stringify(o)) as DocObject),
    groups: (doc.groups ?? []).filter((g) => gids.has(g.id)).map((g) => ({ ...g })),
    layers: doc.layers.filter((l) => lids.has(l.id)).map((l) => ({ id: l.id, name: l.name, color: l.color })),
    ...(options ? { object_options: options } : {}),
  };
}

/** Bounds of a fragment's objects. */
export function fragmentBounds(f: ArtFragment): Box | null {
  return unionBoxes(f.objects.map(objectBounds));
}

/**
 * A fragment split into its top-level pieces (a top group, or an ungrouped object), each with its centre: pasting
 * every piece with its own `art.insert` (one batch, one undo step) keeps them as separate pieces, where one insert of
 * several pieces would wrap them in a new group.
 */
export function fragmentPieces(f: ArtFragment): { fragment: ArtFragment; center: Vec2 }[] {
  const asDoc: GDocument = { revision: 0, name: "", stock: null, layers: [], objects: f.objects, groups: f.groups, next_id: 0 };
  const pieces = new Map<string, DocObject[]>();
  for (const o of f.objects) {
    const key = topGroup(asDoc, o.id) ?? o.id;
    const list = pieces.get(key);
    if (list) list.push(o);
    else pieces.set(key, [o]);
  }
  const out: { fragment: ArtFragment; center: Vec2 }[] = [];
  for (const objs of pieces.values()) {
    const gids = new Set(objs.flatMap((o) => groupChain(asDoc, o.group)));
    const lids = new Set(objs.map((o) => o.layer));
    const b = unionBoxes(objs.map(objectBounds));
    if (!b) continue;
    const options = optionsFor(f.object_options, objs);
    out.push({
      fragment: {
        format: "openetch-art",
        objects: objs,
        groups: f.groups.filter((g) => gids.has(g.id)),
        layers: f.layers.filter((l) => lids.has(l.id)),
        ...(options ? { object_options: options } : {}),
      },
      center: [(b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2],
    });
  }
  return out;
}

/**
 * The `art.insert` invocations that paste `f` with its bounds centre at `at` (LightBurn's Paste, at the pointer), or in
 * place when `at` is null (Paste in Place). Layers are matched by colour, as the engine does for Art Library items.
 */
export function pasteInvocations(f: ArtFragment, at: Vec2 | null): { id: string; params: Record<string, unknown> }[] {
  const b = fragmentBounds(f);
  if (!b) return [];
  const c: Vec2 = [(b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2];
  const off: Vec2 = at ? [at[0] - c[0], at[1] - c[1]] : [0, 0];
  return fragmentPieces(f).map((p) => ({
    id: "art.insert",
    params: { content: JSON.stringify(p.fragment), center: [r4(p.center[0] + off[0]), r4(p.center[1] + off[1])] },
  }));
}

const XML_ESC: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&apos;" };
const esc = (s: string) => s.replace(/[&<>"']/g, (c) => XML_ESC[c]);

/** The marker a copied SVG carries, so pasting it back here uses the full internal copy. */
export const CLIP_MARKER = "openetch-clip:";

/**
 * The selection as an SVG document in millimetres (y down, as SVG is), each object a path stroked in its layer
 * colour, images embedded, for pasting into Inkscape, Illustrator or CorelDraw. `marker` tags our own copies.
 */
export function selectionSvg(doc: GDocument, ids: readonly string[], marker: string | null): string | null {
  const want = new Set(ids);
  const objs = doc.objects.filter((o) => want.has(o.id));
  const b = unionBoxes(objs.map(objectBounds));
  if (!b || !objs.length) return null;
  const color = new Map(doc.layers.map((l) => [l.id, l.color]));
  const w = Math.max(b.x1 - b.x0, 1e-3);
  const h = Math.max(b.y1 - b.y0, 1e-3);
  const parts: string[] = [];
  for (const o of objs) {
    const g = o.geometry;
    if (g.kind === "image") {
      const t = o.transform;
      parts.push(
        `<image transform="matrix(${t.map(r4).join(" ")}) scale(1 -1)" x="${r4(-g.width_mm / 2)}" y="${r4(-g.height_mm / 2)}" width="${r4(g.width_mm)}" height="${r4(g.height_mm)}" preserveAspectRatio="none" href="data:${esc(g.mime)};base64,${g.data_base64}"/>`,
      );
      continue;
    }
    const d = toPathData(geometryCmds(g), o.transform);
    if (!d) continue;
    parts.push(`<path d="${d}" fill="none" stroke="${esc(color.get(o.layer) ?? "#000000")}" stroke-width="0.1"><title>${esc(o.name)}</title></path>`);
  }
  const mark = marker ? `<!-- ${CLIP_MARKER}${esc(marker)} -->\n` : "";
  return (
    `<?xml version="1.0" encoding="UTF-8"?>\n${mark}` +
    `<svg xmlns="http://www.w3.org/2000/svg" width="${r4(w)}mm" height="${r4(h)}mm" viewBox="${r4(b.x0)} ${r4(-b.y1)} ${r4(w)} ${r4(h)}">\n` +
    `<g transform="scale(1 -1)">\n${parts.join("\n")}\n</g>\n</svg>\n`
  );
}

/** The marker in clipboard text we wrote, or null. */
export function clipMarkerOf(text: string | null | undefined): string | null {
  if (!text) return null;
  const i = text.indexOf(CLIP_MARKER);
  if (i < 0) return null;
  const m = /^[A-Za-z0-9_-]+/.exec(text.slice(i + CLIP_MARKER.length));
  return m ? m[0] : null;
}

/** An `<svg>` document inside text or HTML (Inkscape and Illustrator put SVG code on the clipboard as text), or null. */
export function svgIn(text: string | null | undefined): string | null {
  if (!text) return null;
  const start = text.search(/<svg[\s>]/i);
  if (start < 0) return null;
  const end = text.toLowerCase().lastIndexOf("</svg>");
  if (end < start) return null;
  const head = text.slice(0, start);
  // Keep an XML prolog / doctype only when it directly precedes the svg element.
  const prolog = /^\s*(<\?xml[^>]*>\s*)?(<!--[\s\S]*?-->\s*)*(<!DOCTYPE[^>]*>\s*)?$/i.test(head) ? head.trimStart() : "";
  return prolog + text.slice(start, end + 6);
}

/** What a system paste holds, in the order LightBurn prefers them: our own copy, vectors, a bitmap, then text. */
export type ClipContent =
  | { kind: "internal"; marker: string }
  | { kind: "svg"; svg: string }
  | { kind: "image"; file: { type: string; name: string } }
  | { kind: "text"; text: string }
  | { kind: "none" };

export function classifyClipboard(c: { text?: string | null; html?: string | null; svg?: string | null; files?: { type: string; name: string }[] }): ClipContent {
  const marker = clipMarkerOf(c.svg) ?? clipMarkerOf(c.text);
  if (marker) return { kind: "internal", marker };
  const svg = svgIn(c.svg) ?? svgIn(c.text) ?? svgIn(c.html);
  if (svg) return { kind: "svg", svg };
  const img = (c.files ?? []).find((f) => /^image\/(png|jpeg|gif|bmp|webp)$/i.test(f.type));
  if (img) return { kind: "image", file: img };
  const svgFile = (c.files ?? []).find((f) => f.type === "image/svg+xml");
  if (svgFile) return { kind: "image", file: svgFile };
  const text = (c.text ?? "").replace(/\r\n?/g, "\n").trim();
  if (text) return { kind: "text", text: text.length > 2000 ? text.slice(0, 2000) : text };
  return { kind: "none" };
}

// ---- guides in the document -----------------------------------------------------------------------------------------

/** The document's guides in the canvas's form. */
export function docGuides(doc: GDocument | null): Guide[] {
  const list: DocGuide[] = Array.isArray(doc?.guides) ? (doc?.guides ?? []) : [];
  return list
    .filter((g) => (g.axis === "x" || g.axis === "y") && Number.isFinite(g.position_mm) && typeof g.id === "string")
    .map((g) => ({ id: g.id, axis: g.axis, v: g.position_mm }));
}

/**
 * Guides an older version kept in this browser that the document does not have yet (same axis and position, within a
 * micrometre, count as the same guide): what to move into the document.
 */
export function guidesToMigrate(browser: readonly Guide[], inDoc: readonly Guide[]): Guide[] {
  const out: Guide[] = [];
  for (const g of browser) {
    const same = (x: Guide) => x.axis === g.axis && Math.abs(x.v - g.v) < 1e-3;
    if (!inDoc.some(same) && !out.some(same)) out.push(g);
  }
  return out.slice(0, 1000);
}

// ---- Art Library ------------------------------------------------------------------------------------------------------

/** The `format=` an Art Library import takes for a file, or a reason it cannot. */
export function artImportFormat(name: string): { format: string } | { error: string } {
  const ext = extOf(name);
  if (ext === "lbart") return { format: "lbart" };
  if (ext === "lbrn2") return { format: "lbrn2" };
  if (ext === "openetch") return { format: "openetch" };
  const f = engineFormatOf(name);
  if (!f) return { error: tr("dz.m.notArt", { name }) };
  return { format: f };
}

/** A library name problem (the engine follows the project-name rules), or null. */
export function libraryNameProblem(name: string, existing: readonly string[], current?: string): string | null {
  const n = name.trim();
  if (!n) return tr("dz.m.enterName");
  if (n.length > 100) return tr("dz.m.nameTooLong");
  if (/[\\/:*?"<>|]/.test(n) || /[\u0000-\u001f]/.test(n)) return tr("dz.m.nameChars");
  if (n.startsWith(".") || n.endsWith(".")) return tr("dz.m.nameDot");
  if (existing.some((e) => e.toLowerCase() === n.toLowerCase() && e.toLowerCase() !== current?.toLowerCase())) return tr("dz.m.nameTaken");
  return null;
}

// ---- tangent circles ------------------------------------------------------------------------------------------------

export interface TangentCandidate {
  index: number;
  center: Vec2;
  radius_mm: number;
}

export function parseCandidates(r: unknown): TangentCandidate[] {
  const list = (r as { candidates?: unknown } | null)?.candidates;
  if (!Array.isArray(list)) return [];
  return list.filter(
    (c): c is TangentCandidate =>
      !!c && typeof c.index === "number" && Array.isArray(c.center) && c.center.length === 2 && c.center.every(Number.isFinite) && typeof c.radius_mm === "number" && c.radius_mm > 0,
  );
}

/** The candidate whose outline passes nearest `p` within `tol` mm, or null. */
export function nearestCandidate(cands: readonly TangentCandidate[], p: Vec2, tol: number): TangentCandidate | null {
  let best: { d: number; c: TangentCandidate } | null = null;
  for (const c of cands) {
    const d = Math.abs(Math.hypot(p[0] - c.center[0], p[1] - c.center[1]) - c.radius_mm);
    if (d <= tol && (!best || d < best.d)) best = { d, c };
  }
  return best?.c ?? null;
}

// ---- Shape Properties ---------------------------------------------------------------------------------------------

export type ShapeParams =
  | { kind: "rect"; width_mm: number; height_mm: number; corner_radius_mm: number }
  | { kind: "ellipse"; rx_mm: number; ry_mm: number }
  | { kind: "polygon"; sides: number; radius_mm: number; star_ratio: number | null; corner_radius_mm: number; ry_mm: number | null };

/** The editable parameters of a primitive, or null for paths, text and images. */
export function shapeParams(g: Geometry): ShapeParams | null {
  switch (g.kind) {
    case "rect":
      return { kind: "rect", width_mm: g.width_mm, height_mm: g.height_mm, corner_radius_mm: g.corner_radius_mm ?? 0 };
    case "ellipse":
      return { kind: "ellipse", rx_mm: g.rx_mm, ry_mm: g.ry_mm };
    case "polygon":
      return {
        kind: "polygon",
        sides: g.sides,
        radius_mm: g.radius_mm,
        star_ratio: g.star_ratio ?? null,
        corner_radius_mm: g.corner_radius_mm ?? 0,
        ry_mm: g.ry_mm ?? null,
      };
    default:
      return null;
  }
}

/** Why a `shape.update` patch would be refused (the engine's limits), or null. */
export function shapePatchProblem(p: ShapeParams, patch: Record<string, number | null>): string | null {
  const v = (k: string): number | null | undefined => patch[k];
  for (const k of ["width_mm", "height_mm", "rx_mm", "ry_mm", "radius_mm"]) {
    const x = v(k);
    if (x !== undefined && !(x !== null && x > 0 && x <= 1e6)) return tr("dz.m.sizesPositive");
  }
  const cr = v("corner_radius_mm");
  if (cr !== undefined && !(cr !== null && cr >= 0)) return tr("dz.m.radiusNegative");
  if (p.kind === "rect" && cr !== undefined && cr !== null) {
    const w = v("width_mm") ?? p.width_mm;
    const h = v("height_mm") ?? p.height_mm;
    if (cr > Math.min(w ?? 0, h ?? 0) / 2 + 1e-9) return tr("dz.m.radiusHalf");
  }
  const s = v("sides");
  if (s !== undefined && !(s !== null && Number.isInteger(s) && s >= 3 && s <= 1000)) return tr("dz.m.sides");
  const sr = v("star_ratio");
  if (sr !== undefined && sr !== null && !(sr > 0 && sr < 1)) return tr("dz.m.starRatio");
  return null;
}

// ---- misc -----------------------------------------------------------------------------------------------------------

export function r4(v: number): number {
  return Math.round(v * 1e4) / 1e4;
}
