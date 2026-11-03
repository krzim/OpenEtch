// The machine status chip's state (owner: P5, docs/ux/ui-plan-2026-11.md P5) and the connection banner's rule, as pure
// functions: Idle, Framing (with an inline Stop frame), Running 42% · 3:12 (with Pause), Paused (with Resume), Offline.

import { jobStatus, type JobStatus } from "../api/job.ts";
import { activeRuns, pendingRuns } from "../api/runs.ts";
import type { MachineStatus } from "../api/types.ts";
import type { LinkState } from "../api/useEngine.ts";
import { tr } from "../workspace/tr.ts";
import { onlineOf } from "./laserContext.ts";
import { shortClock } from "./startFlow.ts";

export type ChipKind = "offline" | "connecting" | "noLaser" | "idle" | "busy" | "framing" | "starting" | "pedal" | "running" | "paused";
/** Colour family: grey idle, green ready, amber armed (framing, waiting), red running or bad. */
export type ChipTone = "idle" | "ok" | "armed" | "run" | "bad";
export type ChipAction = "stopFrame" | "pause" | "resume" | null;

export interface ChipState {
  kind: ChipKind;
  tone: ChipTone;
  text: string;
  /** Job progress in percent (0..100, whole numbers), when a job runs. */
  pct: number | null;
  remainingS: number | null;
  /** The inline button next to the chip. */
  action: ChipAction;
  /** Runs waiting for approval. */
  pending: number;
  /** Pulse (framing); off with prefers-reduced-motion. */
  pulse: boolean;
}

function base(kind: ChipKind, tone: ChipTone, text: string, extra: Partial<ChipState> = {}): ChipState {
  return { kind, tone, text, pct: null, remainingS: null, action: null, pending: 0, pulse: false, ...extra };
}

/** The chip for the machine status, the engine link and the job (default: GET /api/machine's `job`). */
export function chipState(machine: MachineStatus | null, link: LinkState, job: JobStatus | null = null, httpError: string | null = null): ChipState {
  if (!onlineOf({ link, machine, httpError })) {
    return link === "connecting" && !httpError ? base("connecting", "idle", tr("lz.chipConnecting")) : base("offline", "bad", tr("lz.chipOffline"));
  }
  if (!machine) return base("connecting", "idle", tr("lz.chipConnecting"));
  const j = job ?? jobStatus(machine.job);
  const pending = pendingRuns(machine).length;
  if (machine.framing === true) return base("framing", "armed", tr("lz.chipFraming"), { action: "stopFrame", pulse: true, pending });
  if (j.active) {
    const pct = Math.max(0, Math.min(100, Math.floor(j.progress * 100)));
    if (j.paused) return base("paused", "armed", tr("lz.chipPaused", { pct }), { pct, action: "resume", pending });
    if (j.state === "starting") return base("starting", "run", tr("lz.chipStarting"), { action: "pause", pending });
    if (j.state === "waiting_for_start_input") return base("pedal", "armed", tr("lz.chipPedal"), { pending });
    // A run waiting at a layer for the operator (production::layer_pause): Continue in the cockpit or the HUD.
    if (j.state === "waiting_at_layer") return base("pedal", "armed", tr("prod.chipLayer"), { pending });
    const left = j.finishing || j.remaining_s < 1 ? null : j.remaining_s;
    const text = left === null ? tr("lz.chipRunning", { pct }) : tr("lz.chipRunningLeft", { pct, time: shortClock(left) });
    return base("running", "run", text, { pct, remainingS: left, action: "pause", pending });
  }
  if (machine.running === true || activeRuns(machine).length > 0) return base("running", "run", tr("lz.chipRunningPlain"), { pending });
  if (machine.connected === false) return base("noLaser", "bad", tr("lz.chipNoLaser"), { pending });
  if (machine.busy === true) return base("busy", "armed", tr("lz.chipBusy"), { pending });
  const sim = !!(machine.simulated ?? machine.sim);
  return base("idle", pending > 0 ? "armed" : "ok", sim ? tr("lz.chipIdleSim") : tr("lz.chipIdle"), { pending });
}

/** How long the link may be "connecting" before the banner shows (a reconnect usually takes less). */
export const BANNER_GRACE_MS = 1500;

/**
 * Whether the lost-connection banner shows: at once when the link is closed, after the grace period while it keeps
 * reconnecting; never while the engine answers.
 */
export function bannerVisible(online: boolean, link: LinkState, notOnlineForMs: number): boolean {
  if (online) return false;
  return link === "closed" || notOnlineForMs >= BANNER_GRACE_MS;
}
