// The production features' data on the web side (engine `production`): the cockpit's state from GET /api/production,
// what its big panel says ([`cockpitPhase`]), the serial slots and the ledger as rows, and the drafts the Settings
// sections edit (notification targets, reminders, the extractor). Pure: no React, no fetch, so `npm test` covers it.

import type { JobStatus } from "../api/job.ts";
import type { FootCycle } from "../api/types.ts";
import { tr } from "../workspace/tr.ts";

// ---- GET /api/production -----------------------------------------------------------------------------------------

export interface SlotValue {
  object: string;
  mode: string;
  shown: string;
}

export interface Slot {
  number: number;
  offset: number;
  label: string;
  objects: string[];
  empty: boolean;
  past_end: boolean;
  value: number;
  values: SlotValue[];
}

export interface SlotPlan {
  slots: Slot[];
  current: number;
  start: number;
  end: number;
  stop_at_end: boolean;
  auto_advance: boolean;
  advance: number;
  exhausted: boolean;
}

export interface LedgerSlot {
  number: number;
  offset: number;
  label: string;
  value: number;
  values: SlotValue[];
  marked: boolean;
  skipped?: string | null;
}

export interface LedgerEntry {
  time_unix_s: number;
  run_id: string;
  state: string;
  device: string;
  project: string | null;
  document: string;
  completed: number;
  counter: number;
  slots: LedgerSlot[];
  remark_of?: { run_id: string; slot: number } | null;
}

export interface Reminder {
  id: string;
  name: string;
  every_hours: number | null;
  every_runs: number | null;
  hours_since: number;
  runs_since: number;
  left_hours: number | null;
  left_runs: number | null;
  done_unix_s: number;
  due: boolean;
}

export interface MeterStatus {
  device: string;
  hours: number;
  measured_hours: number;
  estimated_hours: number;
  runs: number;
  since_unix_s: number | null;
  due: Reminder[];
  reminders: Reminder[];
}

export type ExtractorOutput =
  | { kind: "none" }
  | { kind: "lmc_busy" }
  | { kind: "gpio"; pin: number; active_low?: boolean }
  | { kind: "home_assistant"; target: string; entity_id: string }
  | { kind: "webhook"; target: string };

export type ExtractorGate = { kind: "none" } | { kind: "input"; bit: number; active_low?: boolean } | { kind: "switched_on" };

export interface ExtractorConfig {
  output: ExtractorOutput;
  auto: boolean;
  run_on_s: number;
  gate: ExtractorGate;
}

export interface ExtractorStatus {
  configured: boolean;
  config: ExtractorConfig;
  state: { on: boolean; since_unix_s: number | null; off_at_unix_s: number | null; last_error: string | null };
  busy_output: number | null;
}

export interface LayerPauseStatus {
  waiting: boolean;
  run_id: string | null;
  layer: string | null;
  since_unix_s: number | null;
  foot_switch: boolean;
}

export interface LockStatus {
  locked: boolean;
  holder: string | null;
  label: string | null;
  since_unix_s: number | null;
  lease_s: number;
}

export interface CycleStatus {
  last_burn_s: number | null;
  last_cycle_s: number | null;
  average_cycle_s: number | null;
  count: number;
  stop_at: number | null;
  left: number | null;
  eta_s: number | null;
}

export interface NotifyEvent {
  seq: number;
  event: string;
  operation: "frame" | "mark" | "none";
  title: string;
  message: string;
  time_unix_s: number;
}

export interface ProductionStatus {
  revision: number;
  document: string;
  project: string | null;
  device: string;
  approval_mode: "pin" | "click";
  lock: LockStatus;
  job: JobStatus;
  foot_cycle: FootCycle;
  slots: SlotPlan;
  slots_refusal: string | null;
  layer_pause: LayerPauseStatus;
  cycle: CycleStatus;
  maintenance: MeterStatus;
  extractor: ExtractorStatus;
  ledger: LedgerEntry[];
  notifications: { targets: number; last: NotifyEvent | null };
}

// ---- formatting ---------------------------------------------------------------------------------------------------

/** "0:42", "3:40", "1:02:03" (whole seconds, never negative). */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.round(Number.isFinite(seconds) ? seconds : 0));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** A short duration: "23.4 s" under a minute, else a clock. */
export function shortDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return "–";
  return seconds < 60 ? `${seconds.toFixed(1)} s` : clock(seconds);
}

/** Hours with one decimal ("12.3 h"). */
export function hours(h: number): string {
  return `${(Number.isFinite(h) ? h : 0).toFixed(1)} h`;
}

// ---- the cockpit's big panel ----------------------------------------------------------------------------------------

export type PhaseKind =
  | "offline"
  | "layer"
  | "running"
  | "paused"
  | "pedal"
  | "starting"
  | "ready_foot"
  | "framing"
  | "target"
  | "exhausted"
  | "idle";

/** Colour family, as the status chip: amber armed (waiting for you, framing), red running or bad, green ready. */
export type PhaseTone = "idle" | "ok" | "armed" | "run" | "bad";

export interface Phase {
  kind: PhaseKind;
  tone: PhaseTone;
  title: string;
  detail: string;
  /** The big number: seconds (a countdown while running, the time waiting at a layer), or null. */
  seconds: number | null;
  /** What the big number is ("left", "waiting", "framing ends in", "done in about"). */
  secondsLabel: string;
  /** Progress 0..100 while running. */
  pct: number | null;
  /** The person is needed now (amber "waiting for you"). */
  waitingForYou: boolean;
}

/**
 * What the cockpit's big panel shows, most urgent first: offline, a layer pause, a running / paused job, a run waiting
 * for the foot switch, the foot switch cycle framing ("press to burn"), framing, the target (Stop At) reached, the
 * serial range used up, else ready.
 */
export function cockpitPhase(
  p: ProductionStatus | null,
  online: boolean,
  machine: { framing?: unknown; connected?: unknown } | null,
  nowUnixS: number = Date.now() / 1000,
): Phase {
  const ph = (kind: PhaseKind, tone: PhaseTone, title: string, detail = "", extra: Partial<Phase> = {}): Phase => ({
    kind,
    tone,
    title,
    detail,
    seconds: null,
    secondsLabel: "",
    pct: null,
    waitingForYou: false,
    ...extra,
  });
  if (!online || !p) return ph("offline", "bad", tr("prod.phOffline"), tr("prod.phOfflineDetail"));
  const j = p.job;
  if (p.layer_pause.waiting) {
    const since = p.layer_pause.since_unix_s ?? nowUnixS;
    const how = p.layer_pause.foot_switch ? tr("prod.phLayerHowFoot") : tr("prod.phLayerHow");
    return ph("layer", "armed", tr("prod.phLayerTitle", { layer: p.layer_pause.layer ?? "" }), tr("prod.phLayerDetail", { layer: p.layer_pause.layer ?? "", how }), {
      seconds: Math.max(0, nowUnixS - since),
      secondsLabel: tr("prod.clkWaiting"),
      waitingForYou: true,
    });
  }
  if (j.active) {
    const pct = Math.max(0, Math.min(100, Math.floor(j.progress * 100)));
    if (j.paused) return ph("paused", "armed", tr("prod.phPaused"), tr("prod.phPausedDetail"), { pct });
    if (j.state === "waiting_for_start_input") return ph("pedal", "armed", tr("prod.phWaiting"), tr("prod.phPedalDetail"), { waitingForYou: true });
    if (j.state === "starting") return ph("starting", "run", tr("prod.phStarting"));
    const left = j.finishing ? 0 : j.remaining_s;
    return ph("running", "run", tr("prod.phRunning"), j.finishing ? tr("prod.phFinishing") : "", { seconds: left, secondsLabel: tr("prod.clkLeft"), pct });
  }
  const cycle = p.foot_cycle;
  if (cycle?.phase === "framing") {
    const ends = cycle.since_unix_s !== null && cycle.idle_timeout_s ? cycle.since_unix_s + cycle.idle_timeout_s - nowUnixS : null;
    return ph("ready_foot", "armed", tr("prod.phReadyFoot"), tr("prod.phReadyFootDetail"), {
      waitingForYou: true,
      seconds: ends,
      secondsLabel: tr("prod.clkFrameEnds"),
    });
  }
  if (cycle?.phase === "burning") return ph("starting", "run", tr("prod.phStarting"));
  if (machine?.framing === true) return ph("framing", "armed", tr("prod.phFraming"), tr("prod.phFramingDetail"));
  if (p.cycle.stop_at !== null && p.cycle.count >= p.cycle.stop_at) {
    return ph("target", "ok", tr("prod.phTarget", { count: p.cycle.count, target: p.cycle.stop_at }), tr("prod.phTargetDetail"));
  }
  if (p.slots_refusal) return ph("exhausted", "bad", tr("prod.phExhausted"), p.slots_refusal);
  if (machine && machine.connected === false) return ph("offline", "bad", tr("prod.phNoLaser"), tr("prod.phNoLaserDetail"));
  const detail = cycle?.available ? tr("prod.phIdleFoot") : p.approval_mode === "pin" ? tr("prod.phIdlePin") : tr("prod.phIdle");
  return ph("idle", "ok", tr("prod.phReady"), detail, { seconds: p.cycle.eta_s, secondsLabel: tr("prod.clkEta") });
}

/** "23 / 50" or "23" without a target, and the share done (0..1, null without a target). */
export function counterText(c: CycleStatus): { text: string; share: number | null } {
  if (c.stop_at === null || c.stop_at <= 0) return { text: String(c.count), share: null };
  return { text: `${c.count} / ${c.stop_at}`, share: Math.max(0, Math.min(1, c.count / c.stop_at)) };
}

// ---- slots and the ledger -----------------------------------------------------------------------------------------

/** One slot's line in the cockpit: what it shows next, or why it is left out. */
export function slotLine(s: Slot): { label: string; value: string; state: "marks" | "empty" | "past_end" } {
  const value = s.values.map((v) => v.shown).filter((x) => x !== "").join(" · ") || String(s.value);
  return { label: s.label, value, state: s.empty ? "empty" : s.past_end ? "past_end" : "marks" };
}

/** The `empty_slots` list with `offset` switched (a sorted copy). */
export function toggleEmpty(plan: SlotPlan, offset: number): number[] {
  const empty = new Set(plan.slots.filter((s) => s.empty).map((s) => s.offset));
  if (empty.has(offset)) empty.delete(offset);
  else empty.add(offset);
  return [...empty].sort((a, b) => a - b);
}

/** The values the next run marks, shortest first ("SN 007 – SN 012" or one value), for the cockpit's headline. */
export function nextValues(plan: SlotPlan): string {
  const marks = plan.slots.filter((s) => !s.empty && !s.past_end).map((s) => slotLine(s).value);
  if (marks.length === 0) return "–";
  return marks.length === 1 ? marks[0] : `${marks[0]} – ${marks[marks.length - 1]}`;
}

export interface LedgerRow {
  key: string;
  time_unix_s: number;
  run_id: string;
  state: string;
  project: string;
  slot: number;
  label: string;
  shown: string;
  marked: boolean;
  skipped: string | null;
  remarkOf: string | null;
}

/** The ledger as one row per slot, newest run first. */
export function ledgerRows(entries: readonly LedgerEntry[]): LedgerRow[] {
  const out: LedgerRow[] = [];
  for (const e of [...entries].reverse()) {
    for (const s of e.slots) {
      out.push({
        key: `${e.run_id}:${e.time_unix_s}:${s.number}`,
        time_unix_s: e.time_unix_s,
        run_id: e.run_id,
        state: e.state,
        project: e.project ?? e.document,
        slot: s.number,
        label: s.label,
        shown: s.values.map((v) => v.shown).join(" · ") || String(s.value),
        marked: s.marked,
        skipped: s.skipped ?? null,
        remarkOf: e.remark_of ? `${e.remark_of.run_id} #${e.remark_of.slot}` : null,
      });
    }
  }
  return out;
}

// ---- drafts of the Settings sections --------------------------------------------------------------------------------

export type TargetKind = "ntfy" | "webhook" | "home_assistant";

export const EVENT_KINDS = ["job_done", "waiting_for_foot_switch", "door", "error", "stop_at_reached", "layer_pause", "maintenance_due"] as const;
export type EventKind = (typeof EVENT_KINDS)[number];

/** A target as the Settings section edits it (PUT /api/notifications); `token` only when typed (else the engine keeps it). */
export interface TargetDraft {
  id?: string;
  name: string;
  kind: TargetKind;
  url: string;
  topic?: string | null;
  token?: string;
  events: EventKind[];
  enabled: boolean;
}

/** A target as GET /api/notifications lists it. */
export interface TargetView {
  id: string;
  name: string;
  kind: TargetKind;
  url: string;
  topic: string | null;
  token_set: boolean;
  events: EventKind[];
  enabled: boolean;
  last?: { time_unix_s: number; ok: boolean; result: string };
}

/** A new target of `kind`, with the events most people want. */
export function newTarget(kind: TargetKind, n: number): TargetDraft {
  const url = kind === "ntfy" ? "https://ntfy.sh" : kind === "home_assistant" ? "http://homeassistant.local:8123" : "https://";
  return {
    name: kind === "ntfy" ? `ntfy ${n}` : kind === "home_assistant" ? "Home Assistant" : `Webhook ${n}`,
    kind,
    url,
    topic: kind === "ntfy" ? "" : null,
    events: ["job_done", "waiting_for_foot_switch", "door", "error", "stop_at_reached", "layer_pause", "maintenance_due"],
    enabled: true,
  };
}

/** Why `t` cannot be saved, in the words the engine would use (checked again there), or null. */
export function targetProblem(t: TargetDraft, tokenSet: boolean): string | null {
  if (!t.name.trim()) return tr("prod.tgNeedName");
  if (!/^https?:\/\/[^\s"\\]{2,}$/i.test(t.url.trim())) return tr("prod.tgNeedUrl");
  if (t.kind === "ntfy" && !/^[A-Za-z0-9_-]{1,64}$/.test((t.topic ?? "").trim())) return tr("prod.tgNeedTopic");
  if (t.kind === "home_assistant" && !tokenSet && !(t.token ?? "").trim()) return tr("prod.tgNeedToken");
  if (t.token && /[\s"\\]/.test(t.token)) return tr("prod.tgBadToken");
  return null;
}

/** The draft of a listed target (no token: the engine keeps the stored one). */
export function draftOf(t: TargetView): TargetDraft {
  return { id: t.id, name: t.name, kind: t.kind, url: t.url, topic: t.topic, events: [...t.events], enabled: t.enabled };
}

export interface ReminderDraft {
  id?: string;
  name: string;
  every_hours?: number | null;
  every_runs?: number | null;
}

/** Why a reminder cannot be saved, or null. */
export function reminderProblem(r: ReminderDraft): string | null {
  if (!r.name.trim()) return tr("prod.remNeedName");
  const h = r.every_hours ?? null;
  const n = r.every_runs ?? null;
  if (h === null && n === null) return tr("prod.remNeedEvery");
  if (h !== null && !(h > 0)) return tr("prod.remNeedEvery");
  if (n !== null && !(Number.isInteger(n) && n >= 1)) return tr("prod.remNeedEvery");
  return null;
}

/** "12.5 h left", "3 runs left", "due" for a reminder. */
export function reminderLeft(r: Reminder): string {
  if (r.due) return tr("prod.remDue");
  const parts: string[] = [];
  if (r.left_hours !== null) parts.push(tr("prod.remLeftHours", { h: r.left_hours.toFixed(1) }));
  if (r.left_runs !== null) parts.push(tr("prod.remLeftRuns", { n: r.left_runs }));
  return parts.join(" · ");
}

export const DEFAULT_EXTRACTOR: ExtractorConfig = { output: { kind: "none" }, auto: false, run_on_s: 0, gate: { kind: "none" } };

/** Why the extractor settings cannot be saved, or null. */
export function extractorProblem(c: ExtractorConfig): string | null {
  if (!(Number.isInteger(c.run_on_s) && c.run_on_s >= 0 && c.run_on_s <= 3600)) return tr("prod.exRunOnRange");
  if (c.output.kind === "gpio" && !(Number.isInteger(c.output.pin) && c.output.pin >= 0 && c.output.pin <= 53)) return tr("prod.exPinRange");
  if (c.output.kind === "home_assistant" && !/^[a-z0-9_]+\.[a-z0-9_]+$/.test(c.output.entity_id)) return tr("prod.exEntity");
  if ((c.output.kind === "home_assistant" || c.output.kind === "webhook") && !c.output.target) return tr("prod.exTarget");
  if (c.gate.kind === "input" && !(Number.isInteger(c.gate.bit) && c.gate.bit >= 0 && c.gate.bit <= 15)) return tr("prod.exBitRange");
  if (c.gate.kind === "switched_on" && c.output.kind === "none") return tr("prod.exGateNeedsOutput");
  return null;
}

// ---- the cockpit's route and lock ------------------------------------------------------------------------------------

/** The installable app opens the cockpit at `#/cockpit` (a Home Screen bookmark or the manifest's shortcut). */
export const COCKPIT_HASH = "#/cockpit";

export function isCockpitHash(hash: string): boolean {
  return hash === COCKPIT_HASH || hash.startsWith(`${COCKPIT_HASH}?`);
}

/** How often the cockpit renews its design lock (the engine's lease is 45 s). */
export const LOCK_RENEW_MS = 15_000;
