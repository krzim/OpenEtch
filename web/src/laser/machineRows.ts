// The Machine card's rows (owner: P5, docs/ux/ui-plan-2026-11.md P5): GET /api/machine as friendly labelled rows
// ("Focus light: Off", "Field: 153.75 × 153.75 mm"), and every other plain value under Advanced with its raw key, so
// no snake_case key shows outside Advanced.

import type { MachineStatus } from "../api/types.ts";
import { tr } from "../workspace/tr.ts";

export interface MachineRow {
  label: string;
  value: string;
  /** The row is an error (red). */
  error?: boolean;
}

export interface MachineRows {
  rows: MachineRow[];
  /** [raw key, value] for the Advanced section, sorted by key. */
  advanced: [string, string][];
}

/** Keys the friendly rows show (or that are objects shown elsewhere): never repeated under Advanced. */
const CONSUMED = new Set([
  "connected",
  "sim",
  "simulated",
  "link",
  "busy",
  "state",
  "running",
  "framing",
  "field_mm",
  "field_height_mm",
  "profile",
  "laser_source",
  "driver",
  "max_power_pct",
  "focus_light",
  "last_error",
  "shutting_down",
  "frequency_khz_min",
  "frequency_khz_max",
]);

function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

function fmt(v: number): string {
  return String(Number(v.toFixed(3)));
}

function str(v: unknown): string | null {
  return typeof v === "string" && v.trim() ? v.trim() : null;
}

/** "waiting_for_start_input" -> "Waiting for start input". */
export function sentenceOf(s: string): string {
  const t = s.replace(/[_-]+/g, " ").trim();
  return t ? t.charAt(0).toUpperCase() + t.slice(1) : t;
}

function stateText(m: MachineStatus): string {
  if (m.framing === true) return tr("lz.mFraming");
  if (m.running === true) return tr("lz.mRunning");
  const s = str(m.state);
  if (s === "idle" || (!s && !m.busy)) return tr("lz.mIdle");
  if (s === "busy" || (!s && m.busy)) return tr("lz.mBusy");
  return sentenceOf(s ?? "");
}

function flatten(prefix: string, v: unknown, out: [string, string][]): void {
  if (v === null || v === undefined) return;
  if (Array.isArray(v)) {
    if (v.length && v.every((x) => typeof x !== "object")) out.push([prefix, v.join(", ")]);
    return;
  }
  if (typeof v === "object") {
    for (const [k, x] of Object.entries(v as Record<string, unknown>)) if (typeof x !== "object" || x === null) flatten(`${prefix}.${k}`, x, out);
    return;
  }
  out.push([prefix, String(v)]);
}

export function machineRows(m: MachineStatus): MachineRows {
  const rows: MachineRow[] = [];
  const sim = !!(m.simulated ?? m.sim);
  const link = str(m.link);
  const how = sim ? ` (${tr("lz.mSim")})` : link === "usb" ? ` (${tr("lz.mUsb")})` : "";
  rows.push({ label: tr("lz.mLink"), value: (m.connected ? tr("lz.mConnected") : tr("lz.mNotConnected")) + how, error: !m.connected });
  rows.push({ label: tr("lz.mState"), value: stateText(m) });
  const device = m.device && typeof m.device === "object" ? str((m.device as Record<string, unknown>).display_name) : null;
  const dev = device ?? str(m.profile);
  if (dev) rows.push({ label: tr("lz.mDevice"), value: dev });
  const source = str(m.laser_source);
  if (source) rows.push({ label: tr("lz.mSource"), value: source });
  const driver = str(m.driver);
  if (driver) rows.push({ label: tr("lz.mDriver"), value: driver.toUpperCase() });
  const fw = num(m.field_mm);
  const fh = num(m.field_height_mm);
  if (fw !== null) rows.push({ label: tr("lz.mField"), value: `${fmt(fw)} × ${fmt(fh ?? fw)} mm` });
  const power = num(m.max_power_pct);
  if (power !== null) rows.push({ label: tr("lz.mPowerLimit"), value: `${fmt(power)}%` });
  const fmin = num(m.frequency_khz_min);
  const fmax = num(m.frequency_khz_max);
  if (fmin !== null && fmax !== null) rows.push({ label: tr("lz.mFrequency"), value: `${fmt(fmin)}–${fmt(fmax)} kHz` });
  if (typeof m.focus_light === "boolean") rows.push({ label: tr("lz.mFocusLight"), value: m.focus_light ? tr("lz.mOn") : tr("lz.mOff") });
  const approval = m.approval as { mode?: unknown; foot_switch?: { input_bit?: unknown } | null } | undefined;
  if (approval && typeof approval === "object") {
    if (approval.mode === "click" || approval.mode === "pin") rows.push({ label: tr("lz.mApproval"), value: approval.mode === "click" ? tr("lz.mApprovalClick") : tr("lz.mApprovalPin") });
    const bit = num(approval.foot_switch?.input_bit);
    if (bit !== null) rows.push({ label: tr("lz.mFootSwitch"), value: tr("lz.mFootInput", { bit }) });
  }
  if (m.shutting_down === true) rows.push({ label: tr("lz.mShutdown"), value: tr("lz.mShuttingDown"), error: true });
  const err = str(m.last_error);
  if (err) rows.push({ label: tr("lz.mLastError"), value: err, error: true });

  const advanced: [string, string][] = [];
  for (const [k, v] of Object.entries(m)) {
    if (CONSUMED.has(k) || v === null || v === undefined) continue;
    if (k === "sim_counters" || (typeof v !== "object" && !Array.isArray(v))) flatten(k, v, advanced);
    else if (Array.isArray(v) && v.every((x) => typeof x !== "object")) flatten(k, v, advanced);
  }
  advanced.sort((a, b) => a[0].localeCompare(b[0]));
  return { rows, advanced };
}
