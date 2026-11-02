// LightBurn's cut options beyond the laser parameters, the project's Optimization Settings, the galvo rotary setup and
// per-shape cut properties, as the engine stores them (`doc::job`, ARCHITECTURE.md "Cut order and cut options" and
// "Rotary"), with the command builders that edit them: `layer.update` / `layer.sub_update` `settings.options`,
// `settings.timing`, `settings.hatch.lines_per_inch` and `global_passes`; `job.set_optimization`; `job.set_rotary`;
// `object.set_cut_properties`. Pure, so `npm test` covers it.

import type { Invocation } from "./client.ts";
import type { CutSettings, GDocument, Layer } from "./types.ts";
import { tr } from "../workspace/tr.ts";

export const CUT_CMD = {
  setOptimization: "job.set_optimization",
  setRotary: "job.set_rotary",
  setCutProperties: "object.set_cut_properties",
} as const;

export type FillGrouping = "individually" | "all_at_once" | "by_group";

/**
 * `doc::CutOptions`: every field optional on the wire (defaults are left out of JSON). `fill_grouping` is the
 * exception: a missing one means `individually` (settings saved before new layers filled all at once), so the
 * engine writes `all_at_once`, LightBurn's default and that of new layers, explicitly.
 */
export interface CutOptions {
  output?: boolean;
  pause_before_ms?: number;
  pause_after_ms?: number;
  dot_mode?: { dot_time_ms: number; dot_spacing_mm: number } | null;
  wobble?: { step_mm: number; size_mm: number } | null;
  fill_dot_width_mm?: number;
  fill_grouping?: FillGrouping;
  auto_rotate?: boolean;
  flood_fill?: boolean;
  perforation?: { cut_mm: number; skip_mm: number } | null;
  tabs?: Partial<TabSettings> | null;
  z_offset_mm?: number;
  z_step_mm?: number;
  /** Kerf Offset: closed outlines of line passes traced outward (+) or inward (-), -50..=50 mm. */
  kerf_offset_mm?: number;
  /** GRBL gantries only: constant power (M3) for these passes even when the device uses dynamic power (M4). */
  constant_power?: boolean;
  /** GRBL gantries only: air assist (M8) while these passes mark. */
  air_assist?: boolean;
  /** GRBL gantries only: laser-off run-in and run-out of every scan line (fills, images), 0..=50 mm. */
  overscan_mm?: number;
  /** GRBL gantries only: closed outlines cut on past their start, 0..=50 mm. */
  overcut_mm?: number;
  /** GRBL gantries only, fills: power ramps up from `ramp_min_power_pct` at the design's edges over this distance, 0..=50 mm. */
  ramp_length_mm?: number;
  /** GRBL ramp: also ramp the outermost edges (LightBurn's Ramp Outer Edge). */
  ramp_outer_edge?: boolean;
  /** GRBL ramp: power at the edge, %. */
  ramp_min_power_pct?: number;
  /** GRBL gantries only, line passes: lead-in / lead-out of every path. */
  lead_in?: Lead | null;
  lead_out?: Lead | null;
  /** GRBL gantries only, line passes: laser-off dwell before / after every path, ms (0..=60000). */
  start_pause_ms?: number;
  end_pause_ms?: number;
}

/** A lead-in or lead-out (`doc::Lead`): angle -90..=90 (positive outside a closed shape), length 0..=50 mm. */
export interface Lead {
  style: "line" | "arc";
  length_mm: number;
  angle_deg: number;
}

/** The resolved options of a pass (defaults filled in). */
export interface ResolvedOptions {
  output: boolean;
  pause_before_ms: number;
  pause_after_ms: number;
  dot_mode: { dot_time_ms: number; dot_spacing_mm: number } | null;
  wobble: { step_mm: number; size_mm: number } | null;
  fill_dot_width_mm: number;
  fill_grouping: FillGrouping;
  auto_rotate: boolean;
  flood_fill: boolean;
  perforation: { cut_mm: number; skip_mm: number } | null;
  tabs: TabSettings | null;
  z_offset_mm: number;
  z_step_mm: number;
  kerf_offset_mm: number;
  constant_power: boolean;
  air_assist: boolean;
  overscan_mm: number;
  overcut_mm: number;
  ramp_length_mm: number;
  ramp_outer_edge: boolean;
  ramp_min_power_pct: number;
  lead_in: Lead | null;
  lead_out: Lead | null;
  start_pause_ms: number;
  end_pause_ms: number;
}

/** A pass's tabs (`doc::Tabs`, LightBurn's Tabs / Bridges). */
export interface TabSettings {
  /** The Tabs / Bridges switch: off keeps the settings and the hand-placed tabs. */
  enabled: boolean;
  /** "auto": spread evenly (shapes with hand-placed tabs use those); "manual": hand-placed tabs only. */
  generation: "auto" | "manual";
  count: number;
  spacing_mm: number;
  /** With a spacing: at most this many per outline (0: no limit). */
  max_tabs: number;
  size_mm: number;
  skip_inner: boolean;
  /** Tab Cut Power, % of the pass's power (0: leave a gap). */
  cut_power_pct: number;
}

/** Longest overscan, overcut, ramp and lead, mm (`doc::MAX_GANTRY_EXTRA_MM`). */
export const MAX_GANTRY_EXTRA_MM = 50;
/** Longest start or end pause of a GRBL path, ms (`doc::MAX_PATH_PAUSE_MS`). */
export const MAX_PATH_PAUSE_MS = 60000;

/** LightBurn-like starting values when an option is switched on. */
export const OPTION_DEFAULTS = {
  dot_mode: { dot_time_ms: 0.5, dot_spacing_mm: 0.1 },
  wobble: { step_mm: 0.1, size_mm: 0.5 },
  perforation: { cut_mm: 1, skip_mm: 1 },
  tabs: { enabled: true, generation: "auto", count: 4, spacing_mm: 0, max_tabs: 0, size_mm: 0.5, skip_inner: false, cut_power_pct: 0 } as TabSettings,
  lead: { style: "line", length_mm: 2, angle_deg: 90 } as Lead,
};

function lead(v: unknown): Lead | null {
  const l = o(v);
  if (!l) return null;
  return { style: l.style === "arc" ? "arc" : "line", length_mm: n(l.length_mm, 2), angle_deg: n(l.angle_deg, 90) };
}

function o(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function n(v: unknown, d: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : d;
}

/** The options of a pass's settings, defaults filled in. */
export function resolveOptions(settings: CutSettings | null | undefined): ResolvedOptions {
  const r = o((settings as { options?: unknown } | null | undefined)?.options) ?? {};
  const dot = o(r.dot_mode);
  const wob = o(r.wobble);
  const per = o(r.perforation);
  const tabs = o(r.tabs);
  const fg = r.fill_grouping;
  return {
    output: r.output !== false,
    pause_before_ms: n(r.pause_before_ms, 0),
    pause_after_ms: n(r.pause_after_ms, 0),
    dot_mode: dot ? { dot_time_ms: n(dot.dot_time_ms, 0.5), dot_spacing_mm: n(dot.dot_spacing_mm, 0.1) } : null,
    wobble: wob ? { step_mm: n(wob.step_mm, 0.1), size_mm: n(wob.size_mm, 0.5) } : null,
    fill_dot_width_mm: n(r.fill_dot_width_mm, 0),
    fill_grouping: fg === "all_at_once" || fg === "by_group" ? fg : "individually",
    auto_rotate: r.auto_rotate === true,
    flood_fill: r.flood_fill === true,
    perforation: per ? { cut_mm: n(per.cut_mm, 1), skip_mm: n(per.skip_mm, 1) } : null,
    tabs: tabs
      ? {
          enabled: tabs.enabled !== false,
          generation: tabs.generation === "manual" ? "manual" : "auto",
          count: Math.round(n(tabs.count, 0)),
          spacing_mm: n(tabs.spacing_mm, 0),
          max_tabs: Math.round(n(tabs.max_tabs, 0)),
          size_mm: n(tabs.size_mm, 0.5),
          skip_inner: tabs.skip_inner === true,
          cut_power_pct: n(tabs.cut_power_pct, 0),
        }
      : null,
    z_offset_mm: n(r.z_offset_mm, 0),
    z_step_mm: n(r.z_step_mm, 0),
    kerf_offset_mm: n(r.kerf_offset_mm, 0),
    constant_power: r.constant_power === true,
    air_assist: r.air_assist === true,
    overscan_mm: n(r.overscan_mm, 0),
    overcut_mm: n(r.overcut_mm, 0),
    ramp_length_mm: n(r.ramp_length_mm, 0),
    ramp_outer_edge: r.ramp_outer_edge === true,
    ramp_min_power_pct: n(r.ramp_min_power_pct, 0),
    lead_in: lead(r.lead_in),
    lead_out: lead(r.lead_out),
    start_pause_ms: n(r.start_pause_ms, 0),
    end_pause_ms: n(r.end_pause_ms, 0),
  };
}

/**
 * How many options differ from a new layer's (for the collapsed section's badge). The GRBL layer options count only
 * when `gantry` (the active device is a GRBL gantry, the only one that shows and uses them).
 */
export function activeOptionCount(r: ResolvedOptions, gantry = false): number {
  let c = 0;
  if (!r.output) c++;
  if (r.pause_before_ms) c++;
  if (r.pause_after_ms) c++;
  if (r.dot_mode) c++;
  if (r.wobble) c++;
  if (r.fill_dot_width_mm) c++;
  if (r.fill_grouping !== "all_at_once") c++;
  if (r.auto_rotate) c++;
  if (r.flood_fill) c++;
  if (r.perforation) c++;
  if (r.tabs?.enabled) c++;
  if (r.z_offset_mm) c++;
  if (r.z_step_mm) c++;
  if (r.kerf_offset_mm) c++;
  if (gantry) {
    if (r.constant_power) c++;
    if (r.air_assist) c++;
    if (r.overscan_mm) c++;
    if (r.overcut_mm) c++;
    if (r.ramp_length_mm) c++;
    if (r.lead_in) c++;
    if (r.lead_out) c++;
    if (r.start_pause_ms) c++;
    if (r.end_pause_ms) c++;
  }
  return c;
}

/** Which pass a form edits: the layer's main settings (`sub` null) or sub-layer `sub`. */
export interface PassRef {
  layer: string;
  sub: number | null;
}

/** A partial settings patch for one pass (`layer.update` or `layer.sub_update`). */
export function settingsPatch(ref: PassRef, settings: Record<string, unknown>): Invocation {
  return ref.sub === null
    ? { id: "layer.update", params: { id: ref.layer, settings } }
    : { id: "layer.sub_update", params: { layer: ref.layer, index: ref.sub, settings } };
}

/** Change cut options (a JSON merge patch: `null` resets one option). */
export function optionsPatch(ref: PassRef, patch: CutOptions | Record<string, unknown>): Invocation {
  return settingsPatch(ref, { options: patch });
}

/** LightBurn's LPI entry: the interval becomes 25.4 / LPI (the engine computes it). */
export function lpiPatch(ref: PassRef, lpi: number): Invocation {
  return settingsPatch(ref, { hatch: { lines_per_inch: lpi } });
}

/** Lines per inch for a hatch interval in mm. */
export function intervalToLpi(intervalMm: number): number {
  return intervalMm > 0 ? 25.4 / intervalMm : 0;
}

/** Global passes of a layer (the whole layer schedule repeats). */
export function globalPasses(layer: Layer): number {
  const g = (layer as Layer & { global_passes?: unknown }).global_passes;
  return typeof g === "number" && g >= 1 ? Math.round(g) : 1;
}

export function globalPassesUpdate(layer: string, passes: number): Invocation {
  return { id: "layer.update", params: { id: layer, global_passes: Math.max(1, Math.min(1000, Math.round(passes))) } };
}

// ---- timing override ------------------------------------------------------------------------------------------------

/** `doc::GalvoTiming` (a layer's timing override; all fields required). */
export interface GalvoTiming {
  laser_on_delay_us: number;
  laser_off_delay_us: number;
  polygon_delay_us: number;
  end_delay_us: number;
  jump_speed_mm_s: number;
  min_jump_delay_us: number;
  max_jump_delay_us: number;
  jump_distance_limit_mm: number;
}

export const TIMING_KEYS: (keyof GalvoTiming)[] = [
  "laser_on_delay_us",
  "laser_off_delay_us",
  "polygon_delay_us",
  "end_delay_us",
  "jump_speed_mm_s",
  "min_jump_delay_us",
  "max_jump_delay_us",
  "jump_distance_limit_mm",
];

/** The B6 profile's timing, used when the device's is unknown. */
export const FALLBACK_TIMING: GalvoTiming = {
  laser_on_delay_us: -300,
  laser_off_delay_us: 100,
  polygon_delay_us: 100,
  end_delay_us: 300,
  jump_speed_mm_s: 2000,
  min_jump_delay_us: 8,
  max_jump_delay_us: 200,
  jump_distance_limit_mm: 10,
};

/**
 * A device profile's timing (`machine::Timing`: `jump_delay_min_us` / `jump_delay_max_us`) as a layer override
 * (`doc::GalvoTiming`: `min_jump_delay_us` / `max_jump_delay_us`); missing values come from `fallback`.
 */
export function deviceTiming(profileTiming: unknown, fallback: GalvoTiming = FALLBACK_TIMING): GalvoTiming {
  const t = o(profileTiming) ?? {};
  return {
    laser_on_delay_us: n(t.laser_on_delay_us, fallback.laser_on_delay_us),
    laser_off_delay_us: n(t.laser_off_delay_us, fallback.laser_off_delay_us),
    polygon_delay_us: n(t.polygon_delay_us, fallback.polygon_delay_us),
    end_delay_us: n(t.end_delay_us, fallback.end_delay_us),
    jump_speed_mm_s: n(t.jump_speed_mm_s, fallback.jump_speed_mm_s),
    min_jump_delay_us: n(t.min_jump_delay_us ?? t.jump_delay_min_us, fallback.min_jump_delay_us),
    max_jump_delay_us: n(t.max_jump_delay_us ?? t.jump_delay_max_us, fallback.max_jump_delay_us),
    jump_distance_limit_mm: n(t.jump_distance_limit_mm, fallback.jump_distance_limit_mm),
  };
}

/** The layer's timing override, or null when it uses the device's. */
export function layerTiming(settings: CutSettings | null | undefined): GalvoTiming | null {
  const t = o(settings?.timing);
  if (!t) return null;
  return deviceTiming(t);
}

/** Turn the override on (prefilled from the device), change one value, or off (null). */
export function timingPatch(ref: PassRef, timing: GalvoTiming | null): Invocation {
  return settingsPatch(ref, { timing });
}

// ---- optimization settings ------------------------------------------------------------------------------------------

export type OrderBy = "layer" | "group" | "priority";
export type CutDirection = "top_to_bottom" | "bottom_to_top" | "left_to_right" | "right_to_left";

/** `doc::Optimization` (LightBurn's Optimization Settings). */
export interface Optimization {
  enabled: boolean;
  order_by: OrderBy[];
  inner_first: boolean;
  direction_order: CutDirection | null;
  reduce_travel: boolean;
  reduce_direction_changes: boolean;
  best_start: boolean;
  corners: boolean;
  best_direction: boolean;
  remove_overlaps: boolean;
  overlap_distance_mm: number;
  cuts_last: boolean;
}

export const DEFAULT_OPTIMIZATION: Optimization = {
  enabled: true,
  order_by: ["layer"],
  inner_first: false,
  direction_order: null,
  reduce_travel: true,
  reduce_direction_changes: false,
  best_start: true,
  corners: false,
  best_direction: true,
  remove_overlaps: false,
  overlap_distance_mm: 0.025,
  cuts_last: false,
};

const ORDERS: readonly OrderBy[] = ["layer", "group", "priority"];
const DIRECTIONS: readonly CutDirection[] = ["top_to_bottom", "bottom_to_top", "left_to_right", "right_to_left"];

/** The document's optimization settings (left out of its JSON while they are the defaults). */
export function optimizationOf(doc: GDocument | null): Optimization {
  const r = o((doc as (GDocument & { optimization?: unknown }) | null)?.optimization) ?? {};
  const d = DEFAULT_OPTIMIZATION;
  const b = (k: keyof Optimization) => (typeof r[k] === "boolean" ? (r[k] as boolean) : (d[k] as boolean));
  const order = Array.isArray(r.order_by) ? (r.order_by.filter((x) => ORDERS.includes(x as OrderBy)) as OrderBy[]) : d.order_by;
  return {
    enabled: b("enabled"),
    order_by: [...new Set(order)],
    inner_first: b("inner_first"),
    direction_order: DIRECTIONS.includes(r.direction_order as CutDirection) ? (r.direction_order as CutDirection) : null,
    reduce_travel: b("reduce_travel"),
    reduce_direction_changes: b("reduce_direction_changes"),
    best_start: b("best_start"),
    corners: b("corners"),
    best_direction: b("best_direction"),
    remove_overlaps: b("remove_overlaps"),
    overlap_distance_mm: n(r.overlap_distance_mm, d.overlap_distance_mm),
    cuts_last: b("cuts_last"),
  };
}

/** The changed keys only (a merge patch), or null when nothing changed. */
export function optimizationPatch(cur: Optimization, next: Optimization): Partial<Optimization> | null {
  const out: Record<string, unknown> = {};
  for (const k of Object.keys(next) as (keyof Optimization)[]) {
    const a = cur[k];
    const b = next[k];
    if (Array.isArray(a) && Array.isArray(b) ? a.join() !== b.join() : a !== b) out[k] = b;
  }
  return Object.keys(out).length ? (out as Partial<Optimization>) : null;
}

export function setOptimization(patch: Partial<Optimization> | Record<string, unknown>): Invocation {
  return { id: CUT_CMD.setOptimization, params: { optimization: patch } };
}

/** Move an entry of the Order By list up or down. */
export function moveOrder(list: readonly OrderBy[], i: number, delta: -1 | 1): OrderBy[] {
  const j = i + delta;
  if (i < 0 || i >= list.length || j < 0 || j >= list.length) return [...list];
  const out = [...list];
  [out[i], out[j]] = [out[j], out[i]];
  return out;
}

// ---- rotary ---------------------------------------------------------------------------------------------------------

export type RotaryKind = "chuck" | "roller";

/** `doc::Rotary` (galvo rotary setup, saved with the project). */
export interface Rotary {
  enabled: boolean;
  kind: RotaryKind;
  steps_per_rotation: number;
  object_diameter_mm: number;
  roller_diameter_mm: number;
  split_width_mm: number;
  cylinder_correction: boolean;
  taper: { end_diameter_mm: number; length_mm: number } | null;
}

export const DEFAULT_ROTARY: Rotary = {
  enabled: false,
  kind: "chuck",
  steps_per_rotation: 3200,
  object_diameter_mm: 80,
  roller_diameter_mm: 30,
  split_width_mm: 10,
  cylinder_correction: true,
  taper: null,
};

export function rotaryOf(doc: GDocument | null): Rotary {
  const r = o((doc as (GDocument & { rotary?: unknown }) | null)?.rotary);
  if (!r) return { ...DEFAULT_ROTARY };
  const d = DEFAULT_ROTARY;
  const t = o(r.taper);
  return {
    enabled: r.enabled === true,
    kind: r.kind === "roller" ? "roller" : "chuck",
    steps_per_rotation: Math.round(n(r.steps_per_rotation, d.steps_per_rotation)),
    object_diameter_mm: n(r.object_diameter_mm, d.object_diameter_mm),
    roller_diameter_mm: n(r.roller_diameter_mm, d.roller_diameter_mm),
    split_width_mm: n(r.split_width_mm, d.split_width_mm),
    cylinder_correction: r.cylinder_correction !== false,
    taper: t ? { end_diameter_mm: n(t.end_diameter_mm, d.object_diameter_mm), length_mm: n(t.length_mm, 100) } : null,
  };
}

/** Problems the engine also refuses (`doc::Rotary::problem`), by field. */
export function rotaryProblems(r: Rotary): Partial<Record<keyof Rotary | "taper.end_diameter_mm" | "taper.length_mm", string>> {
  const out: Partial<Record<string, string>> = {};
  const pos = (k: string, v: number) => {
    if (!(Number.isFinite(v) && v > 0 && v <= 10000)) out[k] = "0 to 10000 mm";
  };
  if (!(Number.isInteger(r.steps_per_rotation) && r.steps_per_rotation >= 1 && r.steps_per_rotation <= 1_000_000)) out.steps_per_rotation = "1 to 1000000";
  pos("object_diameter_mm", r.object_diameter_mm);
  if (r.kind === "roller") pos("roller_diameter_mm", r.roller_diameter_mm);
  pos("split_width_mm", r.split_width_mm);
  if (!out.split_width_mm && r.split_width_mm > r.object_diameter_mm) out.split_width_mm = tr("apic.notMoreThanTheObjectDiameter");
  if (r.taper) {
    pos("taper.end_diameter_mm", r.taper.end_diameter_mm);
    pos("taper.length_mm", r.taper.length_mm);
  }
  return out;
}

/** Circumference of the work, mm (π · diameter): a design taller than this would mark over itself. */
export function circumference(r: Rotary): number {
  return Math.PI * r.object_diameter_mm;
}

/** Motor steps for one turn of the work (a roller turns the work by roller / object diameter). */
export function stepsPerObjectTurn(r: Rotary): number {
  return r.kind === "chuck" ? r.steps_per_rotation : (r.steps_per_rotation * r.object_diameter_mm) / r.roller_diameter_mm;
}

/** Strips a design of height `h` mm splits into. */
export function stripCount(r: Rotary, h: number): number {
  return r.split_width_mm > 0 && h > 0 ? Math.ceil(h / r.split_width_mm - 1e-9) : 0;
}

/** The whole setup (job.set_rotary merges; the whole object keeps it simple and exact). */
export function setRotary(r: Rotary | null): Invocation {
  return { id: CUT_CMD.setRotary, params: { rotary: r } };
}

// ---- shape cut properties -------------------------------------------------------------------------------------------

export interface ObjectOptions {
  priority: number | null;
  power_scale_pct: number;
}

export function objectOptions(doc: GDocument | null, id: string): ObjectOptions {
  const all = o((doc as (GDocument & { object_options?: unknown }) | null)?.object_options);
  const r = o(all?.[id]) ?? {};
  return {
    priority: typeof r.priority === "number" && r.priority >= 0 ? Math.round(r.priority) : null,
    power_scale_pct: n(r.power_scale_pct, 100),
  };
}

/** The shared value of the selection, or null when the objects differ ("mixed"). */
export function sharedOptions(doc: GDocument | null, ids: readonly string[]): { priority: number | null | "mixed"; power_scale_pct: number | "mixed" } {
  const all = ids.map((id) => objectOptions(doc, id));
  if (all.length === 0) return { priority: null, power_scale_pct: 100 };
  const p = all.every((x) => x.priority === all[0].priority) ? all[0].priority : "mixed";
  const s = all.every((x) => x.power_scale_pct === all[0].power_scale_pct) ? all[0].power_scale_pct : "mixed";
  return { priority: p, power_scale_pct: s };
}

export function setCutProperties(ids: readonly string[], p: { priority?: number | null; power_scale_pct?: number }): Invocation {
  const params: Record<string, unknown> = { ids: [...ids] };
  if (p.priority !== undefined) params.priority = p.priority === null ? null : Math.max(0, Math.round(p.priority));
  if (p.power_scale_pct !== undefined) params.power_scale_pct = Math.max(0, Math.min(100, p.power_scale_pct));
  return { id: CUT_CMD.setCutProperties, params };
}
