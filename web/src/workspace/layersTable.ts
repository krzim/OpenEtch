// Pure parts of the Cuts / Layers window (LightBurn's Cuts/Layers): the row text for each layer (mode, speed/power,
// passes; a layer with sub-layers reads "Multi" and expands into one row per pass), bulk toggles, and copy/paste of a
// layer's full settings (main settings and sub-layers) as commands.

import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import type { CutSettings, FillMode, GDocument, Layer } from "../api/types.ts";

export function modeLabel(m: FillMode): string {
  return m === "line" ? "Line" : m === "fill" ? "Fill" : "Fill+Line";
}

export interface PassRow {
  /** 0 = main settings, n = sub_layers[n - 1]. */
  pass: number;
  name: string;
  mode: FillMode;
  speed_mm_s: number;
  power_pct: number;
  passes: number;
  cleanup: boolean;
}

export interface LayerRow {
  layer: Layer;
  /** 1-based position, LightBurn's "#". */
  number: number;
  objects: number;
  /** Has sub-layers: the Mode and Spd/Pwr columns read "Multi". */
  multi: boolean;
  /** Main settings first, then each sub-layer. */
  passes: PassRow[];
}

function passRow(s: CutSettings, pass: number): PassRow {
  return {
    pass,
    name: pass === 0 ? "Main" : s.name?.trim() || `Sub-layer ${pass}`,
    mode: s.mode,
    speed_mm_s: s.speed_mm_s,
    power_pct: s.power_pct,
    passes: s.passes,
    cleanup: !!s.cleanup,
  };
}

export function layerRows(doc: Pick<GDocument, "layers" | "objects">): LayerRow[] {
  const counts = new Map<string, number>();
  for (const o of doc.objects) counts.set(o.layer, (counts.get(o.layer) ?? 0) + 1);
  return doc.layers.map((l, i) => {
    const subs = l.sub_layers ?? [];
    return {
      layer: l,
      number: i + 1,
      objects: counts.get(l.id) ?? 0,
      multi: subs.length > 0,
      passes: [passRow(l.settings, 0), ...subs.map((s, k) => passRow(s, k + 1))],
    };
  });
}

/** Layer updates that set `field` on every layer to `value` (one batch, nothing for layers already there). */
export function setAll(layers: readonly Layer[], field: "visible" | "output", value: boolean): Invocation[] {
  return layers.filter((l) => l[field] !== value).map((l) => C.layerUpdate(l.id, { [field]: value }));
}

/** Flip `field` on every layer. */
export function invertAll(layers: readonly Layer[], field: "visible" | "output"): Invocation[] {
  return layers.map((l) => C.layerUpdate(l.id, { [field]: !l[field] }));
}

/** Everything but `keep` gets `value`'s opposite; `keep` gets `value`. */
export function onlyThis(layers: readonly Layer[], keep: string, field: "visible" | "output"): Invocation[] {
  return layers.filter((l) => (l.id === keep) !== l[field]).map((l) => C.layerUpdate(l.id, { [field]: l.id === keep }));
}

// --- copy / paste settings --------------------------------------------------------------------------------------------

/** The keys `layer.update`'s settings patch accepts (SettingsPatch, HatchPatch, ImagePatch in layer.rs). */
const HATCH_KEYS = ["interval_mm", "angle_deg", "angle_increment_deg", "cross_hatch", "bidirectional"];
const IMAGE_KEYS = [
  "dither",
  "interval_mm",
  "angle_deg",
  "angle_increment_deg",
  "bidirectional",
  "negative",
  "pass_through",
  "dot_width_mm",
  "min_power_pct",
  "halftone_cells_per_inch",
  "halftone_angle_deg",
];
const TIMING_KEYS = [
  "laser_on_delay_us",
  "laser_off_delay_us",
  "polygon_delay_us",
  "end_delay_us",
  "jump_speed_mm_s",
  "min_jump_delay_us",
  "max_jump_delay_us",
  "jump_distance_limit_mm",
];

function pick(o: unknown, keys: string[]): Record<string, unknown> | undefined {
  if (!o || typeof o !== "object") return undefined;
  const r: Record<string, unknown> = {};
  for (const k of keys) if (k in (o as Record<string, unknown>)) r[k] = (o as Record<string, unknown>)[k];
  return r;
}

/** A full settings patch reproducing `s` (name and cleanup are separate sub-layer parameters). */
export function settingsPatch(s: CutSettings): C.CutSettingsPatch {
  const p: Record<string, unknown> = {
    mode: s.mode,
    power_pct: s.power_pct,
    speed_mm_s: s.speed_mm_s,
    frequency_khz: s.frequency_khz,
    pulse_width_ns: s.pulse_width_ns ?? null,
    passes: s.passes,
    hatch: pick(s.hatch, HATCH_KEYS),
  };
  const timing = (s as { timing?: unknown }).timing;
  if (timing === null) p.timing = null;
  else {
    const t = pick(timing, TIMING_KEYS);
    if (t && Object.keys(t).length === TIMING_KEYS.length) p.timing = t;
  }
  const image = pick((s as { image?: unknown }).image, IMAGE_KEYS);
  if (image && Object.keys(image).length) p.image = image;
  return p as C.CutSettingsPatch;
}

/** Copied layer settings (LightBurn's Copy layer settings): the main settings and every sub-layer. */
export interface LayerSettingsClip {
  from: string;
  main: C.CutSettingsPatch;
  subs: { settings: C.CutSettingsPatch; name: string | null; cleanup: CutSettings["cleanup"] }[];
}

export function copyLayerSettings(l: Layer): LayerSettingsClip {
  return {
    from: l.name,
    main: settingsPatch(l.settings),
    subs: (l.sub_layers ?? []).map((s) => ({ settings: settingsPatch(s), name: s.name ?? null, cleanup: s.cleanup ?? null })),
  };
}

/**
 * Commands that give `target` the copied settings (one batch, one undo step): `layer.update` for the main settings;
 * with sub-layer commands available, the target's sub-layers are replaced by the copied ones. Name, colour, output
 * and visibility stay the target's own (as in LightBurn).
 */
export function pasteLayerSettings(target: Layer, clip: LayerSettingsClip, canSubLayers: boolean): Invocation[] {
  const invs: Invocation[] = [C.layerUpdate(target.id, { settings: clip.main })];
  if (!canSubLayers) return invs;
  const n = target.sub_layers?.length ?? 0;
  for (let i = n - 1; i >= 0; i--) invs.push(C.layerSubDelete(target.id, i));
  for (const s of clip.subs) {
    invs.push(C.layerSubAdd(target.id, { settings: s.settings, name: s.name ?? undefined, cleanup: s.cleanup ?? undefined }));
  }
  return invs;
}
