// Jigs and placement in the web app (short list #2, #8, #9, #15; engine side: crates/commands/src/builtin/jigs/,
// crates/engine/src/zones.rs): the document's jig types, field-frame geometry of pockets and keep-out zones, the
// command invocations the UI sends (the same commands MCP agents call), region helpers for Re-mark region and the
// pre-flight finding texts. Pure; tested in jigs.test.ts.

import type { BezPathJson, DocObject, GDocument, Geometry, Vec2, Affine } from "../api/types.ts";
import type { Invocation } from "../api/client.ts";
import { apply, flattenCmds, geometryCmds, mul, rotate, translate, type Box, type PathCmd } from "../geometry.ts";

/** doc::jig::PocketShape. */
export type PocketShape =
  | { kind: "rect"; width_mm: number; height_mm: number; corner_radius_mm?: number }
  | { kind: "circle"; diameter_mm: number }
  | { kind: "path"; path: BezPathJson };

/** doc::jig::Pocket (jig frame). `enabled` is absent while true. */
export interface Pocket {
  id: string;
  number: number;
  center: Vec2;
  rotation_deg?: number;
  shape: PocketShape;
  enabled?: boolean;
}

/** doc::jig::KeepOut (jig frame). */
export interface KeepOut {
  id: string;
  name: string;
  path: BezPathJson;
  enabled?: boolean;
}

/** doc::jig::Jig. */
export interface Jig {
  id: string;
  name: string;
  datum: Vec2;
  rotation_deg?: number;
  material_height_mm?: number | null;
  pockets: Pocket[];
  keep_out?: KeepOut[];
  outline?: BezPathJson | null;
}

/** A device keep-out zone (engine zones::DeviceZone, `PUT /api/prefs` `keep_out`), field mm. */
export interface DeviceZone {
  name: string;
  points: Vec2[];
  enabled?: boolean;
}

/** Object extras key on copies placed in jig pockets (doc::jig::JIG_POCKET_KEY): "<jig id>/<pocket id>". */
export const JIG_POCKET_KEY = "openetch.jig_pocket";

export function jigsOf(doc: GDocument | null): Jig[] {
  return ((doc as { jigs?: Jig[] } | null)?.jigs ?? []) as Jig[];
}

export function pocketOn(p: Pocket): boolean {
  return p.enabled !== false;
}

/** Jig frame -> field frame. */
export function jigTransform(j: Jig): Affine {
  return mul(translate(j.datum[0], j.datum[1]), rotate(j.rotation_deg ?? 0));
}

/** Pocket frame -> field frame. */
export function pocketTransform(j: Jig, p: Pocket): Affine {
  return mul(jigTransform(j), mul(translate(p.center[0], p.center[1]), rotate(p.rotation_deg ?? 0)));
}

function shapeGeometry(s: PocketShape): Geometry {
  if (s.kind === "rect") return { kind: "rect", width_mm: s.width_mm, height_mm: s.height_mm, corner_radius_mm: s.corner_radius_mm ?? 0 };
  if (s.kind === "circle") return { kind: "ellipse", rx_mm: s.diameter_mm / 2, ry_mm: s.diameter_mm / 2 };
  return { kind: "path", path: s.path };
}

/** A pocket's outline in the pocket frame, and the transform to the field. */
export function pocketCmds(j: Jig, p: Pocket): { cmds: PathCmd[]; transform: Affine } {
  return { cmds: geometryCmds(shapeGeometry(p.shape)), transform: pocketTransform(j, p) };
}

export function pocketCenter(j: Jig, p: Pocket): Vec2 {
  return apply(jigTransform(j), p.center);
}

/** Pockets in number order. */
export function sortedPockets(j: Jig): Pocket[] {
  return [...j.pockets].sort((a, b) => a.number - b.number);
}

/** The pocket whose outline holds the field point `f` (the last drawn on top), or null. */
export function pocketAt(jigs: readonly Jig[], f: Vec2): { jig: Jig; pocket: Pocket } | null {
  for (let k = jigs.length - 1; k >= 0; k--) {
    const j = jigs[k];
    for (const p of j.pockets) {
      const { cmds, transform } = pocketCmds(j, p);
      if (insideAny(f, flattenCmds(cmds, transform).map((l) => l.pts))) return { jig: j, pocket: p };
    }
  }
  return null;
}

/** Even-odd point in any of the rings. */
export function insideAny(f: Vec2, rings: readonly Vec2[][]): boolean {
  let c = false;
  for (const r of rings) {
    for (let i = 0, j = r.length - 1; i < r.length; j = i++) {
      const [a, b] = [r[i], r[j]];
      if (a[1] > f[1] !== b[1] > f[1] && f[0] < ((b[0] - a[0]) * (f[1] - a[1])) / (b[1] - a[1]) + a[0]) c = !c;
    }
  }
  return c;
}

/** Keep-out zones of every jig (field frame) and of the device, enabled ones only, as rings with names. */
export function keepOutRings(doc: GDocument | null, device: readonly DeviceZone[]): { name: string; ring: Vec2[]; device: boolean }[] {
  const out: { name: string; ring: Vec2[]; device: boolean }[] = [];
  for (const j of jigsOf(doc)) {
    const t = jigTransform(j);
    for (const k of j.keep_out ?? []) {
      if (k.enabled === false) continue;
      for (const l of flattenCmds(geometryCmds({ kind: "path", path: k.path }), t)) out.push({ name: `${j.name} / ${k.name}`, ring: l.pts, device: false });
    }
  }
  for (const z of device) if (z.enabled !== false) out.push({ name: z.name, ring: z.points, device: true });
  return out;
}

/** The placement (virtual array whose settings name a jig) of a jig, by array id. */
export function placementsOf(doc: GDocument | null, jig: string): { id: string; sources: string[]; clones: string[]; sourcePocket: string }[] {
  const out: { id: string; sources: string[]; clones: string[]; sourcePocket: string }[] = [];
  for (const a of doc?.virtual_arrays ?? []) {
    const s = a.settings as { jig?: unknown; source_pocket?: unknown };
    if (s.jig === jig && typeof s.source_pocket === "string") out.push({ id: a.id, sources: a.sources, clones: a.clones, sourcePocket: s.source_pocket });
  }
  return out;
}

/** Whether a virtual array is a jig placement (Edit Array and Duplicate Array do not apply to it). */
export function isPlacement(a: { settings: Record<string, unknown> }): boolean {
  return typeof a.settings.jig === "string";
}

/** The objects placed in a pocket: the originals in the placement's source pocket, the copies tagged with it. */
export function objectsInPocket(doc: GDocument | null, jig: string, pocket: string): string[] {
  if (!doc) return [];
  const out: string[] = [];
  const tag = `${jig}/${pocket}`;
  for (const p of placementsOf(doc, jig)) if (p.sourcePocket === pocket) out.push(...p.sources);
  for (const o of doc.objects as DocObject[]) if (o.extras?.[JIG_POCKET_KEY] === tag) out.push(o.id);
  return out;
}

// ---- invocations (the same commands MCP agents call) ----------------------------------------------------------------

export type FitRotate = "none" | "ninety" | "best";

export const J = {
  fitToStock: (ids: readonly string[], margin_mm: number, rotate: FitRotate): Invocation => ({ id: "object.fit_to_stock", params: { ids: [...ids], margin_mm, rotate } }),
  create: (p: { name?: string; rows: number; cols: number; pitch_x_mm: number; pitch_y_mm: number; pocket: PocketShape; center?: Vec2; material_height_mm?: number | null; numbering?: Numbering }): Invocation => {
    const params: Record<string, unknown> = { rows: p.rows, cols: p.cols, pitch_x_mm: p.pitch_x_mm, pitch_y_mm: p.pitch_y_mm, pocket: p.pocket };
    if (p.name) params.name = p.name;
    if (p.center) params.center = p.center;
    if (p.material_height_mm != null) params.material_height_mm = p.material_height_mm;
    if (p.numbering) params.numbering = p.numbering;
    return { id: "jig.create", params };
  },
  fromObjects: (ids: readonly string[], name?: string): Invocation => ({ id: "jig.from_objects", params: name ? { ids: [...ids], name } : { ids: [...ids] } }),
  importFile: (content: string, format: "svg" | "dxf", name?: string, center?: Vec2 | null): Invocation => {
    const params: Record<string, unknown> = { content, format };
    if (name) params.name = name;
    if (center) params.center = center;
    return { id: "jig.import", params };
  },
  edit: (jig: string, patch: { name?: string; material_height_mm?: number | null; numbering?: Numbering }): Invocation => ({ id: "jig.edit", params: { jig, ...patch } }),
  move: (jig: string, p: { datum?: Vec2; translate?: Vec2; rotation_deg?: number; rotate_by_deg?: number }): Invocation => ({ id: "jig.move", params: { jig, ...p } }),
  setPockets: (jig: string, pockets: readonly (number | string)[] | "all", enabled: boolean): Invocation =>
    ({ id: "jig.set_pockets", params: pockets === "all" ? { jig, all: true, enabled } : { jig, pockets: [...pockets], enabled } }),
  addKeepOut: (jig: string, ids: readonly string[], name?: string): Invocation => ({ id: "jig.add_keep_out", params: name ? { jig, ids: [...ids], name } : { jig, ids: [...ids] } }),
  keepOut: (jig: string, keep_out: string, enabled?: boolean): Invocation => ({ id: "jig.remove_keep_out", params: enabled === undefined ? { jig, keep_out } : { jig, keep_out, enabled } }),
  remove: (jig: string): Invocation => ({ id: "jig.delete", params: { jig } }),
  place: (jig: string, ids: readonly string[], auto_increment = 1): Invocation => ({ id: "jig.place", params: { jig, ids: [...ids], auto_increment } }),
  preflight: (keep_out: readonly DeviceZone[]): Invocation => ({
    id: "job.preflight",
    params: { keep_out: keep_out.filter((z) => z.enabled !== false).map((z) => ({ name: z.name, points: z.points })) },
  }),
  preflightSet: (check: string, enabled: boolean): Invocation => ({ id: "job.preflight_set", params: { check, enabled } }),
  remarkCheck: (region: readonly Vec2[], selection: readonly string[] | null): Invocation =>
    ({ id: "job.remark_check", params: selection && selection.length ? { region: region.map((p) => [p[0], p[1]]), selection: [...selection] } : { region: region.map((p) => [p[0], p[1]]) } }),
};

export type Numbering = "rows" | "columns" | "serpentine";

/** Pockets for a grid jig preview: rows x cols centres, row 0 at the far side (+Y), like jig.create. */
export function gridCentres(rows: number, cols: number, px: number, py: number, centre: Vec2): Vec2[] {
  const out: Vec2[] = [];
  const w = (cols - 1) * px;
  const h = (rows - 1) * py;
  for (let r = rows - 1; r >= 0; r--) for (let c = 0; c < cols; c++) out.push([centre[0] + c * px - w / 2, centre[1] + r * py - h / 2]);
  return out;
}

/** Why a grid jig cannot be made (a message key), or null. */
export function gridProblem(rows: number, cols: number, px: number, py: number, size: Vec2): "jig.errCount" | "jig.errPitch" | "jig.errSize" | null {
  if (!Number.isInteger(rows) || !Number.isInteger(cols) || rows < 1 || cols < 1 || rows * cols > 500) return "jig.errCount";
  if (!(size[0] > 0 && size[1] > 0)) return "jig.errSize";
  if ((cols > 1 && !(px >= size[0])) || (rows > 1 && !(py >= size[1]))) return "jig.errPitch";
  return null;
}

// ---- regions (Re-mark region) ---------------------------------------------------------------------------------------

/** The four corners of the rectangle between two field points. */
export function rectRegion(a: Vec2, b: Vec2): Vec2[] {
  const [x0, x1] = [Math.min(a[0], b[0]), Math.max(a[0], b[0])];
  const [y0, y1] = [Math.min(a[1], b[1]), Math.max(a[1], b[1])];
  return [
    [x0, y0],
    [x1, y0],
    [x1, y1],
    [x0, y1],
  ];
}

/** Absolute area of a ring. */
export function ringArea(r: readonly Vec2[]): number {
  let a = 0;
  for (let i = 0; i < r.length; i++) {
    const [p, q] = [r[i], r[(i + 1) % r.length]];
    a += p[0] * q[1] - q[0] * p[1];
  }
  return Math.abs(a) / 2;
}

/**
 * A lasso path simplified to what the engine needs: points closer than `tol` mm to the previous kept one are dropped
 * (a finger or mouse sends many), at most `max` points (thinned evenly).
 */
export function simplifyLasso(pts: readonly Vec2[], tol: number, max = 400): Vec2[] {
  const out: Vec2[] = [];
  for (const p of pts) {
    const last = out[out.length - 1];
    if (!last || Math.hypot(p[0] - last[0], p[1] - last[1]) >= tol) out.push([p[0], p[1]]);
  }
  if (out.length <= max) return out;
  const step = out.length / max;
  return Array.from({ length: max }, (_, i) => out[Math.floor(i * step)]);
}

/** Whether a region is usable: 3 points or more and an area of at least 0.01 mm². */
export function regionUsable(r: readonly Vec2[] | null): r is Vec2[] {
  return !!r && r.length >= 3 && ringArea(r) >= 0.01;
}

/** Field-frame bounds of a ring. */
export function ringBox(r: readonly Vec2[]): Box {
  let b: Box = { x0: Infinity, y0: Infinity, x1: -Infinity, y1: -Infinity };
  for (const p of r) b = { x0: Math.min(b.x0, p[0]), y0: Math.min(b.y0, p[1]), x1: Math.max(b.x1, p[0]), y1: Math.max(b.y1, p[1]) };
  return b;
}

// ---- pre-flight -----------------------------------------------------------------------------------------------------

/** One `job.preflight` finding. */
export interface PreflightWarning {
  check: string;
  blocking: boolean;
  text: string;
  params: Record<string, unknown>;
  layers: string[];
  objects: string[];
}

export interface PreflightResult {
  warnings: PreflightWarning[];
  disabled: string[];
  checks: string[];
}

export function parsePreflight(v: unknown): PreflightResult {
  const o = (v ?? {}) as Partial<PreflightResult>;
  return {
    warnings: Array.isArray(o.warnings) ? (o.warnings as PreflightWarning[]) : [],
    disabled: Array.isArray(o.disabled) ? (o.disabled as string[]) : [],
    checks: Array.isArray(o.checks) ? (o.checks as string[]) : [],
  };
}

/** The message key and values for a finding (the engine's English text is the fallback). */
export function preflightKey(w: PreflightWarning): { key: string; vars: Record<string, string | number> } {
  const p = w.params ?? {};
  const s = (k: string) => String(p[k] ?? "");
  const n = (k: string) => Number(p[k] ?? 0);
  switch (w.check) {
    case "cut_before_fill":
      return { key: "pf.cutBeforeFill", vars: { cut: s("cut"), fill: s("fill") } };
    case "empty_layer":
      return { key: "pf.emptyLayer", vars: { layer: s("layer") } };
    case "duplicate_shapes":
      return { key: "pf.duplicates", vars: { count: n("count"), names: s("names") } };
    case "keep_out":
      return { key: "pf.keepOut", vars: { name: s("name"), layer: s("layer"), zone: s("zone") } };
    case "unfilled_variable":
      switch (p.kind) {
        case "no_csv":
          return { key: "pf.noCsv", vars: { names: s("names") } };
        case "past_end":
          return { key: "pf.csvPastEnd", vars: { names: s("names"), row: n("row"), last: n("last") } };
        case "empty_cell":
          return { key: "pf.csvEmptyCell", vars: { names: s("names"), row: n("row"), column: n("column") } };
        case "serial_wrap":
          return { key: "pf.serialWrap", vars: { names: s("names"), end: n("end"), start: n("start") } };
        case "stop_at_end":
          return { key: "pf.stopAtEnd", vars: { names: s("names"), end: n("end") } };
      }
      break;
    case "contact_sheet":
      // The contact sheet's checks of the next run's row (engine `contact_sheet::next_run_problems`).
      if (p.kind === "partial") return { key: "pf.rowPartial", vars: { n: n("checked") } };
      return { key: "pf.rowCheck", vars: { name: s("name"), value: n("value"), message: s("message") } };
  }
  return { key: "", vars: {} };
}

/** Findings that are warnings (can be switched off per project) and refusals (keep-out). */
export function splitFindings(r: PreflightResult): { refusals: PreflightWarning[]; warnings: PreflightWarning[] } {
  return { refusals: r.warnings.filter((w) => w.blocking), warnings: r.warnings.filter((w) => !w.blocking) };
}
