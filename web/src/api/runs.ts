// Pure run and machine-response logic for the Laser workspace: which runs wait for approval and which are under way,
// what a run will do (its stats), what the approval error codes mean, and what to tell the user when a run ends or
// STOP answers. No React, no fetch, so `npm test` covers it.

import type { MachineStatus, PendingRun, RunFinishedEvent, RunStats } from "./types.ts";

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

/** Why a pending run cannot be approved right now, or null when it can. */
export function approveBlocker(run: PendingRun, m: MachineStatus | null): string | null {
  if (run.stale) return "Document changed - request a new run.";
  if (run.state !== undefined && run.state !== "pending") return `Run is ${stateLabel(run.state).toLowerCase()}.`;
  if (m?.approval && m.approval.pin_set === false) return APPROVAL_CODE_TEXT.pin_not_set;
  return null;
}

/** The operator PIN is 4-12 digits (`openetch set-pin`). */
export function validPin(pin: string): boolean {
  return /^\d{4,12}$/.test(pin);
}

/** User-facing text for the approval error codes (HTTP 403 `{ok:false, error, code}`). */
export const APPROVAL_CODE_TEXT: Readonly<Record<string, string>> = {
  pin_not_set: "No operator PIN is set. Run 'openetch set-pin' on the engine machine once.",
  pin_invalid: "Wrong PIN.",
  pin_required: "Enter the operator PIN.",
  pin_locked: "Too many wrong PINs. Approvals are locked for 60 s.",
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
      return "Waiting for approval";
    case "approved":
      return "Approved, starting";
    case "awaiting_confirm":
      return "Waiting for the foot pedal";
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

function mm(v: number): string {
  return v >= 1000 ? `${(v / 1000).toFixed(2)} m` : `${v.toFixed(1)} mm`;
}

const num = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/** What a run will do, as label/value rows for the approval card. Missing fields are left out. */
export function runStatRows(run: PendingRun): [string, string][] {
  const s: RunStats = run.stats && typeof run.stats === "object" ? run.stats : {};
  const rows: [string, string][] = [];
  if (num(s.estimated_time_s)) rows.push(["Est. time", formatDuration(s.estimated_time_s)]);
  if (num(s.mark_length_mm)) rows.push(["Mark", mm(s.mark_length_mm)]);
  if (num(s.jump_length_mm)) rows.push(["Jump", mm(s.jump_length_mm)]);
  const layers = num(s.layer_count) ? s.layer_count : Array.isArray(s.layers) ? s.layers.length : num(s.layers) ? s.layers : null;
  if (layers !== null) rows.push(["Layers", String(layers)]);
  if (num(s.segment_count)) rows.push(["Segments", String(s.segment_count)]);
  if (num(run.packet_count)) rows.push(["Packets", String(run.packet_count)]);
  if (num(run.revision)) rows.push(["Revision", `r${run.revision}`]);
  return rows;
}

/** Toast for a `run_finished` event: an error for anything but a clean finish. */
export function runFinishedToast(ev: RunFinishedEvent): { msg: string; kind: "info" | "error" } {
  const id = ev.id ?? ev.run_id ?? "run";
  if (ev.ok) return { msg: `Run ${id} finished.`, kind: "info" };
  const what = ev.state === "aborted" ? "was aborted" : ev.state === "cancelled" ? "was cancelled" : "failed";
  return { msg: `Run ${id} ${what}${ev.error ? `: ${ev.error}` : "."}`, kind: "error" };
}

/** Toast for the answer to POST /api/machine/stop. */
export function stopToast(res: unknown): string {
  const r = res && typeof res === "object" ? (res as { connected?: unknown; cancelled_runs?: unknown }) : {};
  const cancelled = Array.isArray(r.cancelled_runs) ? r.cancelled_runs.length : num(r.cancelled_runs) ? r.cancelled_runs : 0;
  const tail = cancelled > 0 ? ` ${cancelled} pending run${cancelled === 1 ? "" : "s"} cancelled.` : "";
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
