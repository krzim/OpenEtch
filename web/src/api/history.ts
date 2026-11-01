// The timeline model, Fusion style: every recorded step with its state, and a marker after the last applied step.
// Newer engines answer GET /api/history with `{marker, steps}`; older ones with an append-only log of entries (no
// rollback). Both are normalised here so the Timeline renders either. Pure, so `npm test` covers it.

import type { HistoryEntry, HistoryStep, Origin } from "./types.ts";

export interface TimelineStep {
  /** The engine's step index (what `history.goto` and `history.delete` take). */
  index: number;
  /** Short command label, e.g. "transform" or "transform +1". */
  label: string;
  /** Full command ids, for the tooltip. */
  commands: string;
  origin: Origin;
  summary: string;
  state: "active" | "rolled_back" | "failed";
  error: string | null;
  revision: number | null;
}

export interface TimelineModel {
  steps: TimelineStep[];
  /** Index of the last applied step; `start` when everything is rolled back. */
  marker: number;
  /** The marker value meaning "before the first step". */
  start: number;
  /** The engine supports goto/delete (newer engines). */
  editable: boolean;
}

export const EMPTY_TIMELINE: TimelineModel = { steps: [], marker: -1, start: -1, editable: false };

/** "object.transform, layer.update" -> "transform +1". */
export function shortCommand(cmd: string): string {
  const parts = cmd
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  const first = parts[0] ?? cmd;
  const name = first.split(".").pop() ?? first;
  return parts.length > 1 ? `${name} +${parts.length - 1}` : name;
}

function normState(s: unknown): TimelineStep["state"] {
  return s === "rolled_back" || s === "failed" ? s : "active";
}

/** Normalise GET /api/history: `{marker, steps}` (timeline) or an array of entries (older engines). */
export function normalizeHistory(raw: unknown): TimelineModel {
  if (Array.isArray(raw)) {
    const steps: TimelineStep[] = (raw as HistoryEntry[])
      .filter((h) => h && typeof h === "object")
      .map((h, i) => ({
        index: i,
        label: shortCommand(String(h.command ?? "")),
        commands: String(h.command ?? ""),
        origin: h.origin,
        summary: String(h.summary ?? ""),
        state: "active",
        error: null,
        revision: typeof h.revision === "number" ? h.revision : null,
      }));
    return { steps, marker: steps.length - 1, start: -1, editable: false };
  }
  if (!raw || typeof raw !== "object") return EMPTY_TIMELINE;
  const r = raw as { marker?: unknown; steps?: unknown };
  if (!Array.isArray(r.steps)) return EMPTY_TIMELINE;
  const steps: TimelineStep[] = (r.steps as HistoryStep[])
    .filter((s) => s && typeof s === "object" && typeof s.index === "number")
    .map((s) => {
      const ids = Array.isArray(s.invocations) ? s.invocations.map((i) => String(i?.id ?? "")).filter(Boolean) : [];
      const commands = ids.join(", ");
      return {
        index: s.index,
        label: ids.length ? shortCommand(commands) : shortCommand(String((s as { command?: unknown }).command ?? "edit")),
        commands: commands || String((s as { command?: unknown }).command ?? ""),
        origin: s.origin,
        summary: String(s.summary ?? ""),
        state: normState(s.state),
        error: typeof s.error === "string" ? s.error : null,
        revision: typeof s.revision === "number" ? s.revision : null,
      };
    })
    .sort((a, b) => a.index - b.index);
  const start = steps.length ? steps[0].index - 1 : -1;
  const marker = typeof r.marker === "number" && Number.isFinite(r.marker) ? r.marker : steps.length ? steps[steps.length - 1].index : start;
  return { steps, marker, start, editable: true };
}

/** Whether `step` is applied (left of the marker) — failed steps are skipped even then. */
export function isApplied(m: TimelineModel, step: TimelineStep): boolean {
  return step.index <= m.marker && step.state !== "rolled_back";
}

/**
 * Where the marker sits in the strip: the number of steps to its left. The marker follows the last step whose index
 * is at most `marker`.
 */
export function markerSlot(m: TimelineModel): number {
  let n = 0;
  for (const s of m.steps) if (s.index <= m.marker) n++;
  return n;
}

/** The marker value for slot `slot` (0 = before every step). */
export function markerForSlot(m: TimelineModel, slot: number): number {
  if (slot <= 0 || m.steps.length === 0) return m.start;
  return m.steps[Math.min(slot, m.steps.length) - 1].index;
}

/** The nearest slot boundary to `x`, given the right edge of each chip in strip coordinates. */
export function nearestSlot(chipRights: readonly number[], chipLefts: readonly number[], x: number): number {
  let best = 0;
  let bestD = Infinity;
  for (let slot = 0; slot <= chipRights.length; slot++) {
    const at = slot === 0 ? (chipLefts[0] ?? 0) : chipRights[slot - 1];
    const d = Math.abs(at - x);
    if (d < bestD) {
      bestD = d;
      best = slot;
    }
  }
  return best;
}
