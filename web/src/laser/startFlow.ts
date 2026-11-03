// The one Start flow (owner: P5, docs/ux/ui-plan-2026-11.md P5), used by the toolbar, the `laser.requestRun` hotkey
// (App.tsx), the Laser dock, the phone's Job sheet and "Start here". A disabled Start shows its reason (offline,
// running, framing, design-only, checklist incomplete); the label carries the scope and time ("Start · selection (3) ·
// ~2:14"). Starting still goes through the engine's approval (approve.rs arms runs; the app never bypasses it): this
// module only decides whether to ask, and what to open first.
//
// Decision per press (`startDecision`):
// - blocked: offline, loading, shutting down, no laser, design-only, a job running, framing; and in the dock or sheet,
//   where the checklist is shown inline, an unticked Job Checklist.
// - openChecklist: from the toolbar, the hotkey or Start here with the checklist on and unticked: open the Laser panel
//   (or Job sheet) with the checklist instead of firing.
// - openLaserAndFire: from the Design workspace: open the Laser panel (so the job's status is visible) and start.
// - fire: start.

import type { MachineStatus, Vec2 } from "../api/types.ts";
import { activeRuns, noPinMode } from "../api/runs.ts";
import type { Workspace } from "../components/TopBar.tsx";
import { getRunOptions } from "../components/jobStore.ts";
import { getPrefs } from "../workspace/prefs.ts";
import { tr } from "../workspace/tr.ts";
import { getLaserFacts, getLaserFlow, onlineOf, setLaserFlow, type LaserFacts } from "./laserContext.ts";
import { runJob } from "./runJob.ts";
import { getRemarkRegion } from "../jigs/regionStore.ts";

/** Where Start was pressed. */
export type StartSource = "toolbar" | "dock" | "sheet" | "hotkey" | "startHere";

export interface StartCtx {
  source: StartSource;
  /** GET /api/machine (null: not known yet or offline). */
  machine: MachineStatus | null;
  /** The engine link (events WebSocket) is up. */
  online: boolean;
  /** Why the active device cannot burn (api/devices.ts `designOnlyReason`), or null. */
  designOnly: string | null;
  workspace: Workspace;
  /** Selected object ids (the scope with Cut Selected Graphics). */
  selection: readonly string[];
  /** The document revision the run is for. */
  revision: number | null;
  /** The document has no objects: nothing to burn (null or absent: not known). */
  docEmpty?: boolean | null;
  /** The job checklist is on and not every item is ticked (default: from the shared ticks, laser/laserContext.ts). */
  checklistIncomplete?: boolean;
  /** Estimated job time in seconds, when the preview knows it. */
  estimateS?: number | null;
  /** Open the Laser panel (desktop) or the Job sheet (phone), with the checklist inline. */
  openLaser: () => void;
  notify: (msg: string) => void;
  report: (e: unknown) => void;
  /** Refresh GET /api/machine after a request. */
  refreshMachine?: () => void;
  /** "Start here": the preview segment to start from (P5 addition; null or absent: the whole job). */
  fromSegment?: number | null;
  /** Re-mark region (jigs/): only the planned toolpath inside this field-mm polygon (null or absent: everything). */
  region?: readonly Vec2[] | null;
}

export interface StartState {
  enabled: boolean;
  /** Why Start is disabled, as visible text (null when enabled). */
  reason: string | null;
  /** Button label with scope and time. */
  label: string;
}

export type StartBlock = "offline" | "loading" | "shutdown" | "noLaser" | "designOnly" | "running" | "framing" | "empty" | "checklist";
export type StartDecision = "blocked" | "openChecklist" | "openLaserAndFire" | "fire";

/** The checklist sources that show it inline next to Start (an unticked list disables Start there). */
const INLINE_CHECKLIST: ReadonlySet<StartSource> = new Set(["dock", "sheet"]);

/** Whether the shared ticks leave the active device's checklist incomplete (false while the gates are unknown). */
export function sharedChecklistIncomplete(): boolean {
  const { gates, ticks } = getLaserFlow();
  if (!gates || !gates.checklistOn) return false;
  if (gates.items === 0) return true;
  for (let i = 0; i < gates.items; i++) if (ticks[i] !== true) return true;
  return false;
}

function checklistIncomplete(ctx: StartCtx): boolean {
  // With a PIN, the checklist belongs to the approval step (under Request run), not to the request.
  if (!noPinMode(ctx.machine)) return false;
  return ctx.checklistIncomplete ?? sharedChecklistIncomplete();
}

/** A job (or an approved run) holds the machine. */
export function runUnderWay(m: MachineStatus | null): boolean {
  if (!m) return false;
  const job = m.job as { active?: unknown } | undefined;
  return m.running === true || (job !== undefined && job !== null && job.active === true) || activeRuns(m).length > 0;
}

/** The first gate that blocks Start, in the order a person would fix them, or null. */
export function startBlock(ctx: StartCtx): StartBlock | null {
  const m = ctx.machine;
  if (!ctx.online) return "offline";
  if (!m) return "loading";
  if (m.shutting_down === true) return "shutdown";
  if (ctx.designOnly) return "designOnly";
  if (m.connected === false) return "noLaser";
  if (runUnderWay(m)) return "running";
  if (m.framing === true) return "framing";
  // An empty document: Start was enabled and only the engine's refusal said why.
  if (ctx.docEmpty === true) return "empty";
  if (checklistIncomplete(ctx)) return "checklist";
  return null;
}

/** Visible text for a gate. */
export function blockReason(b: StartBlock, ctx: StartCtx): string {
  switch (b) {
    case "offline":
      return tr("lz.whyOffline");
    case "loading":
      return tr("lz.whyLoading");
    case "shutdown":
      return tr("lz.whyShutdown");
    case "noLaser":
      return tr("lz.whyNoLaser");
    case "designOnly":
      return ctx.designOnly ?? tr("lz.whyNoLaser");
    case "running":
      return tr("lz.whyRunning");
    case "framing":
      return tr("lz.whyFraming");
    case "empty":
      return tr("lz.whyEmpty");
    case "checklist":
      return INLINE_CHECKLIST.has(ctx.source) ? tr("lz.whyChecklist") : tr("lz.whyChecklistOpen");
  }
}

/** What pressing Start does now. */
export function startDecision(ctx: StartCtx): StartDecision {
  const b = startBlock(ctx);
  if (b === "checklist") return INLINE_CHECKLIST.has(ctx.source) ? "blocked" : "openChecklist";
  if (b) return "blocked";
  if (ctx.workspace === "design" && !INLINE_CHECKLIST.has(ctx.source)) return "openLaserAndFire";
  return "fire";
}

/** What the run covers: the selection (Cut Selected Graphics with something selected), a segment on, or everything. */
export function startScope(
  ctx: Pick<StartCtx, "selection" | "fromSegment" | "region">,
): { kind: "all" } | { kind: "selection"; count: number } | { kind: "from"; segment: number } | { kind: "region" } {
  if (typeof ctx.fromSegment === "number") return { kind: "from", segment: ctx.fromSegment };
  if (ctx.region && ctx.region.length >= 3) return { kind: "region" };
  // Beginner Mode hides Cut Selected Graphics and ignores it (laser/runJob.ts).
  const cutSelected = getRunOptions().cutSelected && !getPrefs().beginnerMode;
  if (cutSelected && ctx.selection.length > 0) return { kind: "selection", count: ctx.selection.length };
  return { kind: "all" };
}

/** "0:42", "2:14", "1:02:03" (rounded to the second). */
export function shortClock(s: number): string {
  // A job under a second reads ~0:01, never ~0:00 (which looks like nothing would burn).
  const t = s > 0 && s < 1 ? 1 : Math.max(0, Math.round(s));
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const sec = String(t % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** "Start · selection (3)", "Start · all · ~2:14", "Request run · all". The estimate is the whole job's. */
export function startLabel(ctx: StartCtx, starting = false): string {
  const noPin = noPinMode(ctx.machine);
  if (starting) return noPin ? tr("lz.starting") : tr("lz.requesting");
  const parts = [noPin ? tr("lz.start") : tr("lz.request")];
  const scope = startScope(ctx);
  if (scope.kind === "selection") parts.push(tr("lz.scopeSelection", { n: scope.count }));
  else if (scope.kind === "from") parts.push(tr("lz.scopeFrom", { n: scope.segment }));
  else if (scope.kind === "region") parts.push(tr("rm.scope"));
  else parts.push(tr("lz.scopeAll"));
  const est = ctx.estimateS;
  if (scope.kind === "all" && typeof est === "number" && Number.isFinite(est) && est > 0) parts.push(tr("lz.estimate", { time: shortClock(est) }));
  return parts.join(" · ");
}

/** Whether Start can be pressed now, why not, and its label. */
export function startState(ctx: StartCtx): StartState {
  const flow = getLaserFlow();
  const b = startBlock(ctx);
  const d = startDecision(ctx);
  return {
    enabled: d !== "blocked" && !flow.starting,
    reason: b ? blockReason(b, ctx) : null,
    label: startLabel(ctx, flow.starting),
  };
}

/** The Job Checklist ticks to send with a no-PIN Start (null with the checklist off or unknown). */
function ticksForStart(ctx: StartCtx): boolean[] | null {
  if (!noPinMode(ctx.machine)) return null;
  const { gates, ticks } = getLaserFlow();
  if (!gates || !gates.checklistOn) return null;
  return Array.from({ length: gates.items }, (_, i) => ticks[i] === true);
}

/** The Start context from the shared laser facts (laser/laserContext.ts). */
export function startCtxOf(f: LaserFacts, source: StartSource, extra: Partial<StartCtx> = {}): StartCtx {
  return {
    source,
    machine: f.machine,
    online: onlineOf(f),
    designOnly: f.designOnly,
    workspace: f.workspace,
    selection: f.selection,
    revision: f.revision,
    estimateS: f.estimateS,
    docEmpty: f.docEmpty,
    openLaser: f.openLaser,
    notify: (m) => f.notify(m),
    report: f.report,
    refreshMachine: f.refreshMachine,
    // A re-mark region set on the canvas (jigs/), never with Start Here (the engine refuses both together).
    region: extra.fromSegment != null ? null : getRemarkRegion(),
    ...extra,
  };
}

/**
 * The `laser.requestRun` hotkey (App.tsx's `requestRun` body, P1): the one Start flow from the shared laser facts. It
 * opens the checklist instead of firing when the checklist is on and unticked, and the Laser panel when pressed in Design.
 */
export function startFromHotkey(extra: Partial<StartCtx> = {}): Promise<void> {
  return startFlow(startCtxOf(getLaserFacts(), "hotkey", extra));
}

/** Press Start: request the run (or open the checklist / Laser panel first). */
export async function startFlow(ctx: StartCtx): Promise<void> {
  if (getLaserFlow().starting) return;
  const d = startDecision(ctx);
  if (d === "blocked") {
    const b = startBlock(ctx);
    if (b) ctx.notify(blockReason(b, ctx));
    return;
  }
  if (d === "openChecklist") {
    ctx.openLaser();
    ctx.notify(tr("lz.checklistOpened"));
    return;
  }
  if (d === "openLaserAndFire") ctx.openLaser();
  setLaserFlow({ starting: true, refusal: null });
  try {
    const msg = await runJob(noPinMode(ctx.machine), ctx.selection, ctx.fromSegment ?? null, ticksForStart(ctx), ctx.revision, ctx.region ?? null);
    // The checklist is ticked again for every job.
    setLaserFlow({ ticks: [] });
    ctx.notify(msg);
  } catch (e) {
    setLaserFlow({ refusal: e instanceof Error ? e.message : String(e) });
    // The dock and the sheet show the refusal above Start; elsewhere it also comes as a toast.
    if (!INLINE_CHECKLIST.has(ctx.source)) ctx.report(e);
  } finally {
    setLaserFlow({ starting: false });
    ctx.refreshMachine?.();
  }
}
