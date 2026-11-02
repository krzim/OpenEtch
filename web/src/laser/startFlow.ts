// STAGE A STUB (owner: P5). The one Start flow (docs/ux/ui-plan-2026-11.md P5), used by the toolbar, the
// `laser.requestRun` hotkey (App.tsx), the Laser dock, the phone's Job sheet and "Start here". A disabled Start shows
// its reason (offline, running, framing, design-only, checklist incomplete); the label carries the scope and time
// ("Start · selection (3) · ~2:14"). Starting still goes through the engine's approval (approve.rs arms runs; the
// app never bypasses it). The signatures below are final; until P5 lands `startFlow` does nothing and `startState`
// reports a disabled Start with no reason.

import type { MachineStatus } from "../api/types.ts";
import type { Workspace } from "../components/TopBar.tsx";

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
  /** The job checklist is on and not every item is ticked. */
  checklistIncomplete?: boolean;
  /** Estimated job time in seconds, when the preview knows it. */
  estimateS?: number | null;
  /** Open the Laser panel (desktop) or the Job sheet (phone), with the checklist inline. */
  openLaser: () => void;
  notify: (msg: string) => void;
  report: (e: unknown) => void;
  /** Refresh GET /api/machine after a request. */
  refreshMachine?: () => void;
}

export interface StartState {
  enabled: boolean;
  /** Why Start is disabled, as visible text (null when enabled). */
  reason: string | null;
  /** Button label with scope and time. */
  label: string;
}

/** Press Start: request the run (or open the checklist / Laser panel first). */
export async function startFlow(_ctx: StartCtx): Promise<void> {}

/** Whether Start can be pressed now, why not, and its label. */
export function startState(_ctx: StartCtx): StartState {
  return { enabled: false, reason: null, label: "" };
}
