// Pure helpers for the laser side of the UI: the MOPA source's allowed pulse widths and frequency range, per-layer
// energy for preview shading, objects outside the galvo field, engine messages that name internal ids, preview stats
// labels and per-layer preview summaries. No DOM, so `npm test` covers it.

import { objectBounds, type Box } from "../geometry.ts";
import type { CutSettings, GDocument, Layer, MachineStatus, Preview, PreviewSegment } from "./types.ts";

/** JPT M7 MOPA pulse widths (fixtures/b6-mopa-60w.toml), used when the engine does not report its profile's list. */
export const DEFAULT_PULSE_WIDTHS_NS = [2, 4, 6, 9, 13, 20, 30, 45, 55, 60, 80, 100, 150, 200, 250, 300, 350, 400, 450, 500];

/** The pulse widths the source accepts: the engine's `pulse_widths_ns` when it reports one, else the B6 list. */
export function pulseWidths(machine: MachineStatus | null): number[] {
  const raw = machine?.pulse_widths_ns;
  if (Array.isArray(raw)) {
    const v = raw.filter((x): x is number => typeof x === "number" && Number.isFinite(x) && x > 0);
    if (v.length) return [...new Set(v)].sort((a, b) => a - b);
  }
  return DEFAULT_PULSE_WIDTHS_NS;
}

/** The source's frequency range in kHz (engine `frequency_khz_min/max`, else the B6's 1-4000). */
export function frequencyRange(machine: MachineStatus | null): { min: number; max: number } {
  const num = (v: unknown, d: number) => (typeof v === "number" && Number.isFinite(v) && v > 0 ? v : d);
  return { min: num(machine?.frequency_khz_min, 1), max: num(machine?.frequency_khz_max, 4000) };
}

/** The allowed pulse width closest to `ns`. */
export function nearestPulseWidth(ns: number, allowed: readonly number[]): number {
  return allowed.reduce((best, v) => (Math.abs(v - ns) < Math.abs(best - ns) ? v : best), allowed[0]);
}

/** Energy per mm of a layer's main pass times its passes, for relative shading (power % x passes / speed). */
export function layerEnergy(s: CutSettings | undefined): number {
  if (!s || !(s.speed_mm_s > 0)) return 0;
  return (Math.max(0, s.power_pct) * Math.max(1, s.passes)) / s.speed_mm_s;
}

/** Opacity for a layer's preview lines: darker means more energy (0.25 for the weakest layer, 1 for the strongest). */
export function energyOpacity(e: number, max: number): number {
  if (!(max > 0)) return 1;
  return 0.25 + 0.75 * Math.min(1, Math.max(0, e / max));
}

/** Objects whose bounds reach past the ±field/2 square, by id. */
export function outsideField(doc: GDocument | null, fieldMm: number): Set<string> {
  const out = new Set<string>();
  if (!doc || !(fieldMm > 0)) return out;
  const h = fieldMm / 2;
  for (const o of doc.objects) {
    const b = objectBounds(o);
    if (b && boxOutside(b, h)) out.add(o.id);
  }
  return out;
}

/** Whether a box reaches the field edge (the engine refuses points with |coord| >= field/2). */
export function boxOutside(b: Box, half: number): boolean {
  return b.x0 <= -half || b.y0 <= -half || b.x1 >= half || b.y1 >= half;
}

/** Replace internal object and layer ids in an engine message with their names ("obj-5" -> "Ellipse [0,2]"). */
export function humanizeIds(msg: string, doc: GDocument | null): string {
  if (!doc) return msg;
  const names = new Map<string, string>();
  for (const o of doc.objects) names.set(o.id, `"${o.name || o.id}"`);
  for (const l of doc.layers) names.set(l.id, `layer "${l.name}"`);
  return msg.replace(/\b(obj|layer)-\d+\b/g, (id) => names.get(id) ?? id);
}

/** An engine error for display: the HTTP prefix ("POST /api/runs: 422 ") dropped and ids named. */
export function friendlyError(msg: string, doc: GDocument | null): string {
  const m = /^[A-Z]+ \/api\/\S+: \d{3} (.*)$/s.exec(msg);
  return humanizeIds(m ? m[1] : msg, doc);
}

/** Seconds as "0.3 s", "42 s", "3 min 05 s" or "1 h 02 min". */
export function formatDuration(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "";
  if (s < 10) return `${Number(s.toFixed(1))} s`;
  if (s < 60) return `${Math.round(s)} s`;
  if (s < 3600) {
    const m = Math.floor(s / 60);
    return `${m} min ${String(Math.round(s - m * 60)).padStart(2, "0")} s`;
  }
  const h = Math.floor(s / 3600);
  return `${h} h ${String(Math.round((s - h * 3600) / 60)).padStart(2, "0")} min`;
}

/** Preview/run stats as labelled rows, skipping counts already shown elsewhere. */
export function statRows(stats: Record<string, unknown> | undefined, skip: readonly string[] = []): [string, string][] {
  if (!stats) return [];
  const labels: Record<string, [string, (v: number) => string]> = {
    estimated_time_s: ["Estimated time", formatDuration],
    mark_length_mm: ["Mark length", (v) => `${v.toFixed(1)} mm`],
    jump_length_mm: ["Travel length", (v) => `${v.toFixed(1)} mm`],
    segment_count: ["Segments", (v) => String(v)],
    polyline_count: ["Paths", (v) => String(v)],
    packet_count: ["Packets", (v) => String(v)],
    pass_count: ["Passes", (v) => String(v)],
  };
  const rows: [string, string][] = [];
  for (const [k, v] of Object.entries(stats)) {
    if (skip.includes(k) || v === null || typeof v === "object") continue;
    const l = labels[k];
    if (l && typeof v === "number") rows.push([l[0], l[1](v)]);
    else if (!l) rows.push([humanKey(k), typeof v === "number" ? String(Number(v.toFixed(3))) : String(v)]);
  }
  return rows;
}

/** "jump_length_mm" -> "Jump length (mm)". */
export function humanKey(k: string): string {
  const unit = /_(mm|s|ms|us|ns|khz|pct)$/.exec(k);
  const base = (unit ? k.slice(0, -unit[0].length) : k).replace(/_/g, " ");
  const u = unit ? ` (${unit[1] === "pct" ? "%" : unit[1] === "khz" ? "kHz" : unit[1]})` : "";
  return base.charAt(0).toUpperCase() + base.slice(1) + u;
}

export interface LayerPreview {
  id: string;
  name: string;
  color: string;
  mode: string;
  segments: number;
  markLengthMm: number;
  estimatedTimeS: number | null;
  energy: number;
}

/** One row per layer that has marks: the engine's per-layer data when present, else counted from the segments. */
export function previewLayers(preview: Preview | null, doc: GDocument | null): LayerPreview[] {
  if (!preview) return [];
  const byId = new Map<string, Layer>((doc?.layers ?? []).map((l) => [l.id, l]));
  const counted = new Map<string, { n: number; len: number }>();
  for (const s of preview.segments ?? []) {
    if (s.kind !== "mark") continue;
    const k = s.layer ?? "";
    const c = counted.get(k) ?? { n: 0, len: 0 };
    c.n++;
    c.len += segLen(s);
    counted.set(k, c);
  }
  const rows: LayerPreview[] = [];
  const seen = new Set<string>();
  for (const l of preview.layers ?? []) {
    if (!l || typeof l.layer_id !== "string") continue;
    const dl = byId.get(l.layer_id);
    seen.add(l.layer_id);
    rows.push({
      id: l.layer_id,
      name: l.name ?? dl?.name ?? l.layer_id,
      color: l.color ?? dl?.color ?? "#333",
      mode: String(l.mode ?? dl?.settings.mode ?? ""),
      segments: typeof l.segments === "number" ? l.segments : (counted.get(l.layer_id)?.n ?? 0),
      markLengthMm: typeof l.mark_length_mm === "number" ? l.mark_length_mm : (counted.get(l.layer_id)?.len ?? 0),
      estimatedTimeS: typeof l.estimated_time_s === "number" ? l.estimated_time_s : null,
      energy: layerEnergy(dl?.settings),
    });
  }
  for (const [id, c] of counted) {
    if (seen.has(id)) continue;
    const dl = byId.get(id);
    rows.push({
      id,
      name: dl?.name ?? (id || "Unknown layer"),
      color: dl?.color ?? "#333",
      mode: dl?.settings.mode ?? "",
      segments: c.n,
      markLengthMm: c.len,
      estimatedTimeS: null,
      energy: layerEnergy(dl?.settings),
    });
  }
  // Layer list order (LightBurn burns layers top to bottom).
  const order = new Map((doc?.layers ?? []).map((l, i) => [l.id, i]));
  return rows.sort((a, b) => (order.get(a.id) ?? 1e9) - (order.get(b.id) ?? 1e9));
}

export function segLen(s: PreviewSegment): number {
  return Math.hypot(s.to[0] - s.from[0], s.to[1] - s.from[1]);
}

/** Machine status keys the Machine card already shows (or that are objects), so they are not repeated as raw rows. */
export const MACHINE_SHOWN_KEYS = new Set([
  "connected",
  "sim",
  "simulated",
  "link",
  "busy",
  "state",
  "running",
  "framing",
  "field_mm",
  "pending_runs",
  "runs",
  "approval",
  "last_error",
  "sim_counters",
  "profile",
  "laser_source",
  "max_power_pct",
  "pulse_widths_ns",
  "frequency_khz_min",
  "frequency_khz_max",
]);
