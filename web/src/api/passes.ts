// Pure helpers for a layer's passes, LightBurn-style: the main settings plus sub-layers (each a further pass over the
// same objects, run after the main settings in list order). The Properties panel's pass list (which pass is open,
// where the selection goes after a delete or move, cleanup defaults, labels) and the preview's per-pass rows (which
// sub-layer a segment belongs to, per-pass stats and colours, hide and solo). No DOM, so `npm test` covers it.
//
// A pass number is 0 for the layer's main settings and n for `sub_layers[n - 1]`, the same numbering as
// `plan::PassPlan::sub_layer`. The sub-layer commands take `n - 1`.
//
// Run order (doc::Cleanup): the main settings `passes` times; a cleanup sub-layer runs after main passes n, 2n, ...
// and after the last one; then the other sub-layers in list order, each its own `passes` times.

/** Most sub-layers one layer may have (`commands::builtin::sub_layer::MAX_SUB_LAYERS`). */
export const MAX_SUB_LAYERS = 64;

import * as C from "./commands.ts";
import { layerEnergy } from "./laser.ts";
import type { Cleanup, CutSettings, FillMode, GDocument, Layer, Preview, PreviewSegment } from "./types.ts";
import type { SpeedUnit } from "../workspace/prefs.ts";
import { formatSpeed } from "../workspace/units.ts";

/** One tab of a layer's pass list. */
export interface PassEntry {
  /** 0 for the main settings, n for `sub_layers[n - 1]`. */
  pass: number;
  /** Index into `sub_layers`, or null for the main settings. */
  sub: number | null;
  label: string;
  settings: CutSettings;
  cleanup: Cleanup | null;
}

/** The main settings and every sub-layer, in run order. */
export function passEntries(layer: Layer): PassEntry[] {
  const main: PassEntry = { pass: 0, sub: null, label: passLabel(layer, 0), settings: layer.settings, cleanup: null };
  return [main, ...(layer.sub_layers ?? []).map((s, i) => ({ pass: i + 1, sub: i, label: passLabel(layer, i + 1), settings: s, cleanup: s.cleanup ?? null }))];
}

/** A pass's settings, or undefined for a pass the layer does not have. */
export function passSettings(layer: Layer, pass: number): CutSettings | undefined {
  return pass === 0 ? layer.settings : layer.sub_layers?.[pass - 1];
}

/** "Main", a sub-layer's name, "Cleanup" for an unnamed cleanup sub-layer, else "Sub-layer n". */
export function passLabel(layer: Layer, pass: number): string {
  const s = passSettings(layer, pass);
  const name = s?.name?.trim();
  if (pass === 0) return name ? `Main · ${name}` : "Main";
  if (name) return name;
  return s?.cleanup ? "Cleanup" : `Sub-layer ${pass}`;
}

/** The open pass, kept within the layer's passes (a remote edit or undo may have removed it). */
export function clampPass(layer: Layer, pass: number): number {
  const n = layer.sub_layers?.length ?? 0;
  if (!Number.isInteger(pass) || pass < 0) return 0;
  return Math.min(pass, n);
}

/** The pass to open after sub-layer `sub` is deleted from a layer that had `count` sub-layers: the next, else the previous. */
export function passAfterDelete(sub: number, count: number): number {
  const left = count - 1;
  if (left <= 0) return 0;
  return Math.min(sub, left - 1) + 1;
}

/** The open pass after sub-layer `from` moves to `to`: the moved one stays open, others follow their own shift. */
export function passAfterMove(open: number, from: number, to: number): number {
  if (open === 0) return 0;
  const sub = open - 1;
  if (sub === from) return to + 1;
  if (from < sub && to >= sub) return sub; // moved past it from above: it shifts up
  if (from > sub && to <= sub) return sub + 2; // moved in front of it from below: it shifts down
  return open;
}

/** The pass a successful add or duplicate should open: the command's result index when it gave one, else `fallback`. */
export function passFromResult(result: unknown, fallback: number): number {
  const v = typeof result === "number" ? result : typeof (result as { index?: unknown })?.index === "number" ? (result as { index: number }).index : null;
  return v !== null && Number.isInteger(v) && v >= 0 ? v + 1 : fallback;
}

/**
 * Parameters of "Add cleanup pass", LightBurn-like: the same mode and hatch interval as the main settings (the same
 * shape), one fast low-energy pass (a quarter of the power, twice the speed, up to twice the frequency within the
 * source's range) with the hatch turned 45°, cleaning after the layer's main passes `passes` (so once, at the end).
 */
export function cleanupPass(main: CutSettings, frequencyMaxKhz = Infinity): { settings: C.CutSettingsPatch; name: string; cleanup: Cleanup } {
  const freq = Math.min(Math.max(main.frequency_khz, main.frequency_khz * 2), frequencyMaxKhz);
  return {
    name: "Cleanup",
    cleanup: { every_n_passes: Math.max(1, Math.round(main.passes) || 1) },
    settings: {
      power_pct: round(Math.max(0, main.power_pct) / 4, 1),
      speed_mm_s: round(main.speed_mm_s * 2, 1),
      frequency_khz: round(Number.isFinite(freq) && freq > 0 ? freq : main.frequency_khz, 1),
      passes: 1,
      hatch: { angle_deg: normAngle(main.hatch.angle_deg + 45), angle_increment_deg: 0, cross_hatch: false },
    },
  };
}

/** "Line", "Fill", "Fill+Line". */
export function modeLabel(mode: FillMode | string | undefined): string {
  return mode === "fill" ? "Fill" : mode === "fill_then_line" ? "Fill+Line" : mode === "line" ? "Line" : mode === "offset_fill" ? "Offset Fill" : String(mode ?? "");
}

/** One-line settings summary: "Fill · 20% · 1000 mm/s · 30 kHz · 200 ns · ×2", the speed in the display unit. */
export function settingsSummary(s: CutSettings, speedUnit: SpeedUnit = "mm/s"): string {
  const speed = speedUnit === "mm/s" ? `${num(s.speed_mm_s)} mm/s` : formatSpeed(s.speed_mm_s, speedUnit);
  const parts = [modeLabel(s.mode), `${num(s.power_pct)}%`, speed, `${num(s.frequency_khz)} kHz`];
  if (s.pulse_width_ns !== null && s.pulse_width_ns !== undefined) parts.push(`${num(s.pulse_width_ns)} ns`);
  if (s.passes !== 1) parts.push(`×${s.passes}`);
  return parts.join(" · ");
}

// ---- preview ------------------------------------------------------------------------------------------------------

/** Key of one pass of one layer in the preview's hidden set. */
export function passKey(layer: string, pass: number): string {
  return `${layer}#${pass}`;
}

/** Which pass of its layer a preview segment belongs to (0 main, n sub-layer n), or null when it cannot tell. */
export type PassResolver = (s: PreviewSegment) => number | null;

/**
 * Resolve segments to passes: the segment's own `sub_layer`, else the preview's `passes[seg.pass].sub_layer`, else
 * (older engines) by counting the layer's job passes against its settings when that is unambiguous: a layer without
 * sub-layers is all main; otherwise the n-th job pass of the layer is matched to [`expectedPasses`], only when the
 * counts agree (the planner leaves out passes with nothing to mark, e.g. a fill pass over open paths, and then
 * nothing is guessed).
 */
export function passResolver(preview: Preview | null, doc: GDocument | null): PassResolver {
  const plan = preview?.passes;
  const derived = new Map<number, number>();
  const segs = preview?.segments ?? [];
  const reported = (Array.isArray(plan) && plan.length > 0) || segs.some((s) => typeof s.sub_layer === "number");
  const layers = new Map((doc?.layers ?? []).map((l) => [l.id, l]));
  if (!reported) {
    const seen = new Map<string, number[]>();
    for (const s of segs) {
      if (typeof s.pass !== "number" || !s.layer) continue;
      const list = seen.get(s.layer) ?? [];
      if (list[list.length - 1] !== s.pass && !list.includes(s.pass)) list.push(s.pass);
      seen.set(s.layer, list);
    }
    for (const [id, jobPasses] of seen) {
      const l = layers.get(id);
      if (!l) continue;
      const expected = expectedPasses(l);
      if ((l.sub_layers?.length ?? 0) === 0) jobPasses.forEach((p) => derived.set(p, 0));
      else if (expected.length === jobPasses.length) jobPasses.forEach((p, i) => derived.set(p, expected[i]));
    }
  }
  return (s) => {
    if (typeof s.sub_layer === "number") return s.sub_layer;
    if (typeof s.pass !== "number") return null;
    const p = plan?.[s.pass];
    if (p && typeof p.sub_layer === "number" && (!s.layer || p.layer_id === s.layer)) return p.sub_layer;
    return derived.get(s.pass) ?? null;
  };
}

/**
 * The pass number of each job pass a layer plans when none is empty, in run order: the main passes with every cleanup
 * sub-layer after main passes n, 2n, ... and the last, then the other sub-layers, each repeated its own `passes`.
 */
export function expectedPasses(layer: Layer): number[] {
  const out: number[] = [];
  const reps = (s: CutSettings) => Math.max(0, Math.min(1000, Math.round(s.passes) || 0));
  const entries = passEntries(layer);
  const cleanups = entries.filter((e) => e.cleanup);
  const main = reps(layer.settings);
  for (let k = 1; k <= main; k++) {
    out.push(0);
    for (const c of cleanups) {
      const n = Math.max(1, Math.round(c.cleanup!.every_n_passes) || 1);
      if (k % n === 0 || k === main) for (let r = 0; r < reps(c.settings); r++) out.push(c.pass);
    }
  }
  for (const e of entries) if (e.pass > 0 && !e.cleanup) for (let r = 0; r < reps(e.settings); r++) out.push(e.pass);
  return out;
}

/** What one pass of a layer contributes to the previewed job. */
export interface PassPreview {
  layerId: string;
  pass: number;
  key: string;
  label: string;
  settings: CutSettings;
  cleanup: Cleanup | null;
  color: string;
  /** Job passes (repeats) planned. */
  jobPasses: number;
  segments: number;
  markLengthMm: number;
  jumpLengthMm: number;
  /** The engine's estimate, or a share of the layer's estimate by mark time (`approx`), or null. */
  estimatedTimeS: number | null;
  approx: boolean;
}

/**
 * Per-pass rows for every layer with sub-layers, keyed by layer id: counted from the segments (resolved with
 * `resolve`), with the engine's per-pass stats when it reports them. Time: the engine's per-pass estimates, else the
 * layer's estimate shared by mark length / speed.
 */
export function previewPasses(preview: Preview | null, doc: GDocument | null, resolve: PassResolver): Map<string, PassPreview[]> {
  const out = new Map<string, PassPreview[]>();
  if (!preview || !doc) return out;
  const multi = doc.layers.filter((l) => (l.sub_layers?.length ?? 0) > 0);
  if (multi.length === 0) return out;
  const rows = new Map<string, PassPreview>();
  for (const l of multi) {
    out.set(
      l.id,
      passEntries(l).map((e) => {
        const row: PassPreview = {
          layerId: l.id,
          pass: e.pass,
          key: passKey(l.id, e.pass),
          label: e.label,
          settings: e.settings,
          cleanup: e.cleanup,
          color: passColor(l.color, e.pass),
          jobPasses: 0,
          segments: 0,
          markLengthMm: 0,
          jumpLengthMm: 0,
          estimatedTimeS: null,
          approx: false,
        };
        rows.set(row.key, row);
        return row;
      }),
    );
  }
  const jobPasses = new Map<string, Set<number>>();
  for (const s of preview.segments ?? []) {
    if (!s.layer || !out.has(s.layer)) continue;
    const pass = resolve(s);
    if (pass === null) continue;
    const row = rows.get(passKey(s.layer, pass));
    if (!row) continue;
    const len = Math.hypot(s.to[0] - s.from[0], s.to[1] - s.from[1]);
    if (s.kind === "mark") {
      row.segments++;
      row.markLengthMm += len;
    } else row.jumpLengthMm += len;
    if (typeof s.pass === "number") {
      const set = jobPasses.get(row.key) ?? new Set<number>();
      set.add(s.pass);
      jobPasses.set(row.key, set);
    }
  }
  for (const [k, set] of jobPasses) rows.get(k)!.jobPasses = set.size;
  // The engine's per-pass stats, when it reports them, replace the counts.
  const plan = Array.isArray(preview.passes) ? preview.passes : [];
  const fromEngine = new Set<string>();
  plan.forEach((p) => {
    const row = p && rows.get(passKey(p.layer_id, p.sub_layer));
    if (!row) return;
    if (!fromEngine.has(row.key)) {
      fromEngine.add(row.key);
      Object.assign(row, { jobPasses: 0, segments: 0, markLengthMm: 0, jumpLengthMm: 0, estimatedTimeS: 0 });
    }
    row.jobPasses++;
    row.segments += num0(p.segments);
    row.markLengthMm += num0(p.mark_length_mm);
    row.jumpLengthMm += num0(p.jump_length_mm);
    row.estimatedTimeS = typeof p.estimated_time_s === "number" && row.estimatedTimeS !== null ? row.estimatedTimeS + p.estimated_time_s : null;
  });
  // Otherwise share the layer's time estimate by each pass's mark time.
  for (const [id, list] of out) {
    if (list.some((r) => fromEngine.has(r.key))) continue;
    const t = preview.layers?.find((l) => l.layer_id === id)?.estimated_time_s;
    if (typeof t !== "number") continue;
    const w = list.map((r) => (r.settings.speed_mm_s > 0 ? r.markLengthMm / r.settings.speed_mm_s : 0));
    const total = w.reduce((a, b) => a + b, 0);
    if (!(total > 0)) continue;
    list.forEach((r, i) => {
      r.estimatedTimeS = (t * w[i]) / total;
      r.approx = true;
    });
  }
  return out;
}

/** Relative energy of one pass (power x repeats / speed), for preview shading. */
export function passEnergy(layer: Layer | undefined, pass: number | null): number {
  if (!layer) return 0;
  return layerEnergy(pass === null ? layer.settings : (passSettings(layer, pass) ?? layer.settings));
}

/**
 * A preview colour for a pass, distinct within its layer but recognisably the layer's: the main pass keeps the layer
 * colour; sub-layers turn the hue alternately either way in growing steps and alternate darker and lighter. Colours
 * that are not #rgb / #rrggbb are returned unchanged.
 */
export function passColor(base: string, pass: number): string {
  if (pass <= 0) return base;
  const hsl = hexToHsl(base);
  if (!hsl) return base;
  const [h, s, l] = hsl;
  const step = Math.ceil(pass / 2);
  const odd = pass % 2 === 1;
  const hue = h + (odd ? 1 : -1) * 34 * step;
  const sat = Math.max(s, 55);
  const light = Math.min(68, Math.max(28, l + (odd ? -14 : 12) + (step - 1) * (odd ? 4 : -4)));
  return hslToHex(hue, sat, light);
}

/** Preview display state touched by the per-layer and per-pass toggles. */
export interface PassVisibility {
  /** Hidden layer ids. */
  hidden: ReadonlySet<string>;
  /** Hidden passes, by `passKey`. */
  hiddenPasses: ReadonlySet<string>;
}

/** Whether a mark of pass `pass` (null: unknown) on `layer` is hidden. */
export function markHidden(v: PassVisibility, layer: string, pass: number | null): boolean {
  return v.hidden.has(layer) || (pass !== null && v.hiddenPasses.has(passKey(layer, pass)));
}

/** Show or hide one pass. */
export function togglePass(v: PassVisibility, layer: string, pass: number): PassVisibility {
  const hiddenPasses = new Set(v.hiddenPasses);
  const k = passKey(layer, pass);
  if (hiddenPasses.has(k)) hiddenPasses.delete(k);
  else hiddenPasses.add(k);
  return { hidden: v.hidden, hiddenPasses };
}

/**
 * Show only one pass: every other layer hidden and the layer's other passes hidden. Soloing the pass that is already
 * the only one shown shows everything again.
 */
export function soloPass(v: PassVisibility, layerIds: readonly string[], layer: Layer, pass: number): PassVisibility {
  const others = passEntries(layer)
    .filter((e) => e.pass !== pass)
    .map((e) => passKey(layer.id, e.pass));
  const already =
    !v.hidden.has(layer.id) &&
    !v.hiddenPasses.has(passKey(layer.id, pass)) &&
    layerIds.every((id) => id === layer.id || v.hidden.has(id)) &&
    others.every((k) => v.hiddenPasses.has(k));
  if (already) return { hidden: new Set(), hiddenPasses: new Set() };
  const hiddenPasses = new Set([...v.hiddenPasses].filter((k) => !k.startsWith(`${layer.id}#`)));
  others.forEach((k) => hiddenPasses.add(k));
  return { hidden: new Set(layerIds.filter((id) => id !== layer.id)), hiddenPasses };
}

// ---- small helpers ------------------------------------------------------------------------------------------------

function round(v: number, digits: number): number {
  const f = 10 ** digits;
  return Math.round(v * f) / f;
}

function normAngle(a: number): number {
  const r = ((a % 360) + 360) % 360;
  return round(r > 180 ? r - 360 : r, 6);
}

function num(v: number): string {
  return String(Number(v.toFixed(3)));
}

function num0(v: unknown): number {
  return typeof v === "number" && Number.isFinite(v) ? v : 0;
}

function hexToHsl(c: string): [number, number, number] | null {
  let m = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/i.exec(c);
  const hex = m ? `${m[1]}${m[1]}${m[2]}${m[2]}${m[3]}${m[3]}` : (m = /^#([0-9a-f]{6})$/i.exec(c)) ? m[1] : null;
  if (!hex) return null;
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  if (d === 0) return [0, 0, l * 100];
  const s = d / (1 - Math.abs(2 * l - 1));
  const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return [(h * 60 + 360) % 360, s * 100, l * 100];
}

function hslToHex(h: number, s: number, l: number): string {
  const hh = ((h % 360) + 360) % 360;
  const ss = s / 100;
  const ll = l / 100;
  const c = (1 - Math.abs(2 * ll - 1)) * ss;
  const x = c * (1 - Math.abs(((hh / 60) % 2) - 1));
  const m = ll - c / 2;
  const [r, g, b] = hh < 60 ? [c, x, 0] : hh < 120 ? [x, c, 0] : hh < 180 ? [0, c, x] : hh < 240 ? [0, x, c] : hh < 300 ? [x, 0, c] : [c, 0, x];
  return `#${[r, g, b].map((v) => Math.round((v + m) * 255).toString(16).padStart(2, "0")).join("")}`;
}
