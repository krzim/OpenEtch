// Pure run and machine-response logic for the Laser workspace: which runs wait for approval, which are under way and
// which ended, what a run will do (its stats), when approvals are blocked (stale run, PIN unset or changed, lockout)
// and what the approval error codes mean, and what to tell the user when a run ends or STOP answers. No React, no fetch, so `npm test` covers it.

import type { FootCycle, FootSwitch, MachineStatus, PendingRun, RunFinishedEvent, RunStats } from "./types.ts";
import type { LengthUnit } from "../workspace/prefs.ts";
import { MM_PER_IN } from "../workspace/units.ts";
import { tr } from "../workspace/tr.ts";

/**
 * Whether an engine event changes the browser preview's planning context (GET /api/preview/context): another device,
 * and the job counter and learned estimate correction, which move on after every run of a Repeat / Run Continuously
 * job (machine `job_count`, so auto-rotating fills show the next run's angle mid-job) and when a run ends.
 */
export function changesPreviewContext(ev: { type: string; event?: unknown }): boolean {
  return ev.type === "device_changed" || ev.type === "run_finished" || (ev.type === "machine" && ev.event === "job_count");
}

/** States of a run that has been approved but not yet ended. */
export const ACTIVE_STATES: readonly string[] = ["approved", "awaiting_confirm", "running"];

/** Pending runs from the machine status: `pending_runs`, or `runs` filtered to state "pending". */
export function pendingRuns(m: MachineStatus | null): PendingRun[] {
  if (!m) return [];
  if (Array.isArray(m.pending_runs)) return m.pending_runs.filter(isRun);
  if (Array.isArray(m.runs)) return m.runs.filter((r) => isRun(r) && (r.state === undefined || r.state === "pending"));
  return [];
}

/** Approved runs that have not ended yet ("approved", "awaiting_confirm", "running"), from `runs`. */
export function activeRuns(m: MachineStatus | null): PendingRun[] {
  if (!m || !Array.isArray(m.runs)) return [];
  return m.runs.filter((r) => isRun(r) && typeof r.state === "string" && ACTIVE_STATES.includes(r.state));
}

function isRun(r: unknown): r is PendingRun {
  return !!r && typeof r === "object" && typeof (r as { id?: unknown }).id === "string";
}

/** States of a run that has ended. "cancelled" runs were stopped before they streamed anything. */
export const ENDED_STATES: readonly string[] = ["finished", "completed", "failed", "aborted", "cancelled"];

/** Runs that have ended (finished, failed, aborted while streaming, cancelled before it), newest first. */
export function recentRuns(m: MachineStatus | null, limit = 5): PendingRun[] {
  if (!m || !Array.isArray(m.runs)) return [];
  const ended = m.runs.filter((r) => isRun(r) && typeof r.state === "string" && ENDED_STATES.includes(r.state));
  const t = (r: PendingRun) => (num(r.created_unix_s) ? r.created_unix_s : 0);
  return ended
    .map((r, i) => ({ r, i }))
    .sort((a, b) => t(b.r) - t(a.r) || b.i - a.i)
    .slice(0, limit)
    .map((x) => x.r);
}

/** Seconds until approvals unlock after too many wrong PINs, or 0 when they are not locked. */
export function lockSecondsLeft(m: MachineStatus | null, nowUnixS: number): number {
  const until = m?.approval?.locked_until_unix_s;
  if (!num(until)) return 0;
  // The engine caps a lockout at 1 h; a larger value means the two clocks disagree, so do not count past it. Once the
  // engine unlocks it reports null, which ends the lock here whatever this clock says.
  return Math.min(3600, Math.max(0, Math.ceil(until - nowUnixS)));
}

/** "42 s", "2 min 05 s": a lockout countdown. */
export function formatCountdown(s: number): string {
  const total = Math.max(0, Math.ceil(s));
  if (total < 60) return `${total} s`;
  return `${Math.floor(total / 60)} min ${String(total % 60).padStart(2, "0")} s`;
}

/**
 * Machine-wide reason approvals cannot happen right now (no PIN, PIN file changed, lockout with its countdown), or
 * null. Shown once on the approval card; `approveBlocker` returns the same text for each run.
 */
export function approvalNotice(m: MachineStatus | null, nowUnixS: number = Date.now() / 1000): string | null {
  const a = m?.approval;
  if (!a) return null;
  // Without a PIN there is no PIN file to set or watch and no lockout.
  if (noPinMode(m)) return null;
  if (a.pin_set === false) return APPROVAL_CODE_TEXT.pin_not_set;
  if (a.pin_changed === true) return APPROVAL_CODE_TEXT.pin_changed;
  const left = lockSecondsLeft(m, nowUnixS);
  if (left > 0) return `Too many wrong PINs. Approvals unlock in ${formatCountdown(left)}.`;
  return null;
}

/**
 * True when the engine approves runs without a PIN (`--approval click`): Start in this app or the foot switch. An
 * engine that does not report `approval.mode` uses the PIN.
 */
export function noPinMode(m: MachineStatus | null): boolean {
  return m?.approval?.mode === "click";
}

/** The foot switch of the no-PIN mode (it frames, then burns what was framed), or null. */
export function footSwitch(m: MachineStatus | null): FootSwitch | null {
  const f = noPinMode(m) ? m?.approval?.foot_switch : null;
  return f && num(f.input_bit) ? f : null;
}

/** The line on the pending-runs card that says how a run is approved. */
export function approvalHowText(m: MachineStatus | null): string {
  const pedal = m?.approval?.physical_confirm ?? null;
  const confirm = pedal ? tr("lp.thenTheFootPedalInputV1", { v1: pedal.input_bit }) : "";
  if (!noPinMode(m)) return `Approval needs the operator PIN${confirm}.`;
  const foot = footSwitch(m);
  const how = foot ? tr("apir.startOrFootSwitchV1", { v1: foot.input_bit }) : tr("apir.startNoPin");
  return `${how}${confirm}.`;
}

/** The foot switch cycle (no-PIN mode with a foot switch), or null. */
export function footCycle(m: MachineStatus | null): FootCycle | null {
  const c = noPinMode(m) ? m?.foot_cycle : null;
  return c && c.available === true ? c : null;
}

/** The Laser window's line about the foot switch cycle: what a press does now. Null without a foot switch cycle.
 * With the Job Checklist on, the engine refuses every foot switch burn (a foot cannot tick the list): the line says so. */
export function footCycleText(m: MachineStatus | null, checklistOn = false): string | null {
  const c = footCycle(m);
  if (!c) return null;
  if (c.phase === "burning") return tr("apir.cycleBurning");
  if (c.phase === "framing") return tr(checklistOn ? "apir.cycleFramingChecklist" : "apir.cycleFraming");
  const foot = footSwitch(m);
  return tr(checklistOn ? "apir.cycleOffChecklistV1" : "apir.cycleOffV1", { v1: foot ? foot.input_bit : "?" });
}

/**
 * Toast for the foot switch cycle's `foot_cycle_notice` machine event (the engine's own text: why a press did not
 * burn, or why the cycle ended). An error when nothing was burned; null for other events.
 */
export function footSwitchToast(event: unknown, detail: unknown): { msg: string; kind: "info" | "error" } | null {
  if (event !== "foot_cycle_notice" || typeof detail !== "string" || !detail) return null;
  const bad = /not burned|could not|did not finish/i.test(detail);
  return { msg: detail, kind: bad ? "error" : "info" };
}

/**
 * The machine status with an `approval_locked` event applied, so the countdown shows before the next refresh. A
 * status without `approval` (an older engine) is returned unchanged.
 */
export function withApprovalLock(m: MachineStatus | null, untilUnixS: unknown): MachineStatus | null {
  if (!m?.approval || !num(untilUnixS)) return m;
  return { ...m, approval: { ...m.approval, locked_until_unix_s: untilUnixS } };
}

export interface BlockerContext {
  /** Revision of the document this UI shows. A run planned from another revision is treated as stale. */
  docRevision?: number | null;
  /** Current time in Unix seconds (for the lockout countdown); defaults to the browser clock. */
  nowUnixS?: number;
}

/**
 * True when the engine flagged the run stale, or it was planned from another revision than the document shown. The
 * engine's flag arrives with the next machine refresh; the document revision may arrive first.
 */
export function isStale(run: PendingRun, docRevision?: number | null): boolean {
  if (run.stale === true) return true;
  return num(run.revision) && num(docRevision) && docRevision >= 0 && run.revision !== docRevision;
}

/** Why a pending run cannot be approved right now, or null when it can. */
export function approveBlocker(run: PendingRun, m: MachineStatus | null, ctx: BlockerContext = {}): string | null {
  if (isStale(run, ctx.docRevision)) return STALE_TEXT;
  if (run.state !== undefined && run.state !== "pending") return `Run is ${stateLabel(run.state).toLowerCase()}.`;
  return approvalNotice(m, ctx.nowUnixS ?? Date.now() / 1000);
}

const STALE_TEXT = tr("apir.documentChangedRequestANewRun");

/** The operator PIN is 6-12 digits (`openetch set-pin`). */
export function validPin(pin: string): boolean {
  return /^\d{6,12}$/.test(pin);
}

/** Hint under a PIN being typed: null when it is empty or long enough. */
export function pinHint(pin: string): string | null {
  return pin.length > 0 && pin.length < 6 ? tr("apir.atLeast6Digits") : null;
}

/** User-facing text for the approval error codes (HTTP 403 `{ok:false, error, code}`). */
export const APPROVAL_CODE_TEXT: Readonly<Record<string, string>> = {
  pin_not_set: tr("apir.noOperatorPinIsSetRun"),
  pin_invalid: "Wrong PIN.",
  pin_required: tr("apir.enterTheOperatorPin"),
  pin_locked: tr("apir.tooManyWrongPinsApprovalsAre"),
  pin_changed: tr("apir.pinFileChangedRestartTheEngine"),
  browser_required: tr("apir.browserRequired"),
  approval_mode: tr("apir.approvalModeChanged"),
};

/** Message for a failed approval: the code's text when known, else the engine's own message. */
export function approvalErrorText(code: string | undefined, fallback: string): string {
  return (code && APPROVAL_CODE_TEXT[code]) || fallback;
}

/** Short label for a run state. */
export function stateLabel(state: string | undefined): string {
  switch (state) {
    case undefined:
    case "pending":
      return tr("apir.waitingForApproval");
    case "approved":
      return tr("apir.approvedStarting");
    case "awaiting_confirm":
      return tr("apir.waitingForTheFootPedal");
    case "running":
      return "Running";
    case "finished":
    case "completed":
      return "Finished";
    case "failed":
      return "Failed";
    case "aborted":
      return "Aborted";
    case "cancelled":
      return "Cancelled";
    default:
      return state;
  }
}

/** "1 h 02 min", "3 min 05 s", "12.4 s". */
export function formatDuration(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "?";
  if (s < 60) return `${s.toFixed(1)} s`;
  const total = Math.round(s);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const sec = total % 60;
  if (h > 0) return `${h} h ${String(m).padStart(2, "0")} min`;
  return `${m} min ${String(sec).padStart(2, "0")} s`;
}

/** "512.3 mm" / "1.25 m", or "20.17 in" in inches. */
function len(v: number, u: LengthUnit): string {
  if (u === "in") return `${(v / MM_PER_IN).toFixed(2)} in`;
  return v >= 1000 ? `${(v / 1000).toFixed(2)} m` : `${v.toFixed(1)} mm`;
}

const num = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/** What a run will do, as label/value rows for the approval card. Missing fields are left out. */
export function runStatRows(run: PendingRun, lu: LengthUnit = "mm"): [string, string][] {
  const s: RunStats = run.stats && typeof run.stats === "object" ? run.stats : {};
  const rows: [string, string][] = [];
  if (num(s.estimated_time_s)) rows.push([tr("apir.estTime"), formatDuration(s.estimated_time_s)]);
  if (num(s.mark_length_mm)) rows.push(["Mark", len(s.mark_length_mm, lu)]);
  if (num(s.jump_length_mm)) rows.push(["Jump", len(s.jump_length_mm, lu)]);
  if (num(s.segment_count)) rows.push(["Segments", String(s.segment_count)]);
  // The engine drops a run's packets once it is approved, so the count means something only while it is pending.
  const pending = run.state === undefined || run.state === "pending";
  if (pending && num(run.packet_count)) rows.push(["Packets", String(run.packet_count)]);
  if (num(run.revision)) rows.push(["Revision", `r${run.revision}`]);
  return rows;
}

/** Toast for a `run_finished` event ("Job finished."), localised; an error for anything but a clean finish. */
export function runFinishedToast(ev: RunFinishedEvent): { msg: string; kind: "info" | "error" } {
  if (ev.ok) return { msg: tr("apir.jobFinished"), kind: "info" };
  const what = ev.state === "aborted" ? tr("apir.jobAborted") : ev.state === "cancelled" ? tr("apir.jobCancelled") : tr("apir.jobFailed");
  // The engine's reason stays as it came (it names the device or file).
  return { msg: ev.error ? `${what} ${ev.error}` : what, kind: "error" };
}

/** Toast for the answer to POST /api/machine/stop. */
export function stopToast(res: unknown): string {
  const r = res && typeof res === "object" ? (res as { connected?: unknown; cancelled_runs?: unknown }) : {};
  const cancelled = Array.isArray(r.cancelled_runs) ? r.cancelled_runs.length : num(r.cancelled_runs) ? r.cancelled_runs : 0;
  // `cancelled_runs` counts every run that had not started streaming (pending, approved, waiting for the pedal).
  const tail = cancelled > 0 ? ` ${cancelled} run${cancelled === 1 ? "" : "s"} cancelled before starting.` : "";
  if (r.connected === false) return `No laser connected; nothing to stop.${tail}`;
  return `Stop sent.${tail}`;
}

/**
 * The engine answers undo/redo (and some machine calls) with HTTP 200 `{ok:false, error}` when there is nothing to do.
 * Returns the error text for such an answer, or null when it succeeded.
 */
export function okError(res: unknown, fallback: string): string | null {
  if (res && typeof res === "object" && (res as { ok?: unknown }).ok === false) {
    const e = (res as { error?: unknown }).error;
    return typeof e === "string" && e ? e : fallback;
  }
  return null;
}
