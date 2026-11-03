// The laser workflow's shared state (owner: P5, docs/ux/ui-plan-2026-11.md P5). The Start button, the status chip, the
// connection banner and the empty-canvas card take no props (their Stage A signatures are final), so what they need
// from the app comes from here:
// - **Sources** publish what they know: the app shell (`useLaserBridge`, one line in App.tsx), the Laser dock (machine,
//   link, selection, revision, design-only reason) and the preview cards (the estimate). Later sources win per field:
//   self < dock < preview < app.
// - **Self poll**: while a P5 component is mounted and no source reports the engine link, GET /api/machine is polled
//   every 2 s (and on machine events), so the chip and the banner work before the shell publishes anything.
// - **Workflow state** of the one Start flow: the request in flight, the last refusal (shown above Start until it is
//   dismissed or the next Start), the shared Job Checklist ticks and gates, and the revision last framed (the Beginner
//   Mode step bar).
//
// Pure TypeScript apart from `useSyncExternalStore`: node tests import it.

import { useEffect, useSyncExternalStore } from "react";
import type { LinkState } from "../api/useEngine.ts";
import type { MachineStatus } from "../api/types.ts";
import type { Workspace } from "../components/TopBar.tsx";
import { libsToast } from "../libs/store.ts";
import { errorText } from "../api/errorText.ts";

export type Notify = (msg: string, kind?: "info" | "error") => void;

/** What the app knows about the laser; every field optional, merged across sources. */
export interface LaserFacts {
  machine: MachineStatus | null;
  link: LinkState;
  httpError: string | null;
  /** Why the active device cannot frame or burn (design-only), or null. */
  designOnly: string | null;
  workspace: Workspace;
  selection: readonly string[];
  revision: number | null;
  /** The preview's job estimate in seconds (whole job), when known. */
  estimateS: number | null;
  /** The document has no objects (null: not known; the shell decides when to show the empty-canvas card). */
  docEmpty: boolean | null;
  /** Open the Laser workspace and its Laser window (desktop) or the Job sheet (phone). */
  openLaser: () => void;
  notify: Notify;
  report: (e: unknown) => void;
  refreshMachine: () => void;
  /** The pending runs' approval is shown under Request run in the Laser dock (the Pending runs card then only points there). */
  approvalsInDock: boolean;
}

export type LaserSource = Partial<LaserFacts>;
export type SourceId = "self" | "dock" | "preview" | "app";
const ORDER: readonly SourceId[] = ["self", "dock", "preview", "app"];

/** The browser event `openLaser` dispatches when no source gave one (the shell may listen for it). */
export const OPEN_LASER_EVENT = "openetch:open-laser";

function fallbackToast(msg: string, kind?: "info" | "error"): void {
  // Before the shell publishes `notify`, the libs toast bus (set by useLibsUi to App's notify) shows the message.
  libsToast(msg, kind);
}

export const DEFAULT_FACTS: LaserFacts = {
  machine: null,
  link: "connecting",
  httpError: null,
  designOnly: null,
  workspace: "design",
  selection: [],
  revision: null,
  estimateS: null,
  docEmpty: null,
  openLaser: () => {
    if (typeof window !== "undefined") window.dispatchEvent(new CustomEvent(OPEN_LASER_EVENT));
  },
  notify: fallbackToast,
  report: (e) => fallbackToast(errorText(e), "error"),
  refreshMachine: () => undefined,
  approvalsInDock: false,
};

/** Merge the sources in priority order (pure; exported for tests). */
export function mergeSources(sources: Partial<Record<SourceId, LaserSource>>): LaserFacts {
  const out: LaserFacts = { ...DEFAULT_FACTS };
  for (const id of ORDER) {
    const s = sources[id];
    if (!s) continue;
    for (const [k, v] of Object.entries(s)) if (v !== undefined) (out as unknown as Record<string, unknown>)[k] = v;
  }
  return out;
}

/** Whether a source update changes anything a component shows (functions are swapped in silently). */
export function sourceChanged(before: LaserSource | undefined, after: LaserSource | null): boolean {
  if (!before || !after) return before !== undefined || after !== null;
  const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
  for (const k of keys) {
    const a = (before as Record<string, unknown>)[k];
    const b = (after as Record<string, unknown>)[k];
    if (typeof a === "function" && typeof b === "function") continue;
    if (!Object.is(a, b)) return true;
  }
  return false;
}

// ---- workflow state -------------------------------------------------------------------------------------------------

/** The Job Checklist and gates of the active device, as the last mounted gate hook saw them. */
export interface GatesSnapshot {
  checklistOn: boolean;
  items: number;
  requireFraming: boolean;
}

export interface LaserFlow {
  /** A Start request is in flight. */
  starting: boolean;
  /** The last refusal of Start (engine or gate), shown above Start until dismissed or the next Start. */
  refusal: string | null;
  /** The Job Checklist ticks, shared by every Start (dock, sheet, toolbar, hotkey). */
  ticks: readonly boolean[];
  gates: GatesSnapshot | null;
  /** The document revision of the last frame started from this page (null: none since load). */
  framedRevision: number | null;
}

const NO_FLOW: LaserFlow = { starting: false, refusal: null, ticks: [], gates: null, framedRevision: null };

// ---- the store ------------------------------------------------------------------------------------------------------

const sources: Partial<Record<SourceId, LaserSource>> = {};
// A copy, never DEFAULT_FACTS itself: an unchanged source update swaps its functions into this object in place.
let facts: LaserFacts = { ...DEFAULT_FACTS };
let flow: LaserFlow = NO_FLOW;
const subs = new Set<() => void>();

function emit(): void {
  for (const f of [...subs]) f();
}

function subscribe(f: () => void): () => void {
  subs.add(f);
  return () => {
    subs.delete(f);
  };
}

/** Publish (or with null, withdraw) one source's facts. */
export function setLaserSource(id: SourceId, s: LaserSource | null): void {
  const before = sources[id];
  const changed = sourceChanged(before, s);
  if (s) sources[id] = s;
  else delete sources[id];
  // Functions are read through `facts` at call time, so swap them in even when nothing visible changed, but keep the
  // snapshot's identity then: a component that publishes on every render and also reads the facts (LaserDock) would
  // otherwise see a new snapshot after each commit and re-render forever (React error #185).
  const merged = mergeSources(sources);
  if (changed) {
    facts = merged;
    emit();
  } else Object.assign(facts, merged);
}

export function getLaserFacts(): LaserFacts {
  return facts;
}

/** Whether a source other than the self poll reports the engine link. */
export function linkPublished(): boolean {
  return ORDER.some((id) => id !== "self" && sources[id]?.link !== undefined);
}

export function getLaserFlow(): LaserFlow {
  return flow;
}

export function setLaserFlow(patch: Partial<LaserFlow>): void {
  const next = { ...flow, ...patch };
  if (Object.keys(patch).every((k) => Object.is((flow as unknown as Record<string, unknown>)[k], (next as unknown as Record<string, unknown>)[k]))) return;
  flow = next;
  emit();
}

/** Reset everything (tests). */
export function resetLaserStore(): void {
  for (const id of ORDER) delete sources[id];
  facts = { ...DEFAULT_FACTS };
  flow = NO_FLOW;
  emit();
}

export function useLaserFacts(): LaserFacts {
  return useSyncExternalStore(subscribe, getLaserFacts, getLaserFacts);
}

export function useLaserFlow(): LaserFlow {
  return useSyncExternalStore(subscribe, getLaserFlow, getLaserFlow);
}

/** Publish `s` as source `id` while the calling component is mounted. */
export function useLaserSource(id: SourceId, s: LaserSource): void {
  useEffect(() => {
    setLaserSource(id, s);
  });
  useEffect(() => () => setLaserSource(id, null), [id]);
}

/**
 * The shell's one line (App.tsx): everything the Start button, chip, banner and empty card need from the app, e.g.
 * `useLaserBridge({ machine, link: engine.link, httpError: engine.httpError, designOnly: laserDisabled, workspace,
 * selection: selection.objects, revision: doc?.revision ?? null, docEmpty: !!doc && doc.objects.length === 0,
 * openLaser, notify, report, refreshMachine: () => void engine.refreshMachine() })`.
 */
export function useLaserBridge(s: LaserSource): void {
  useLaserSource("app", s);
}

/** The engine answers: the event socket is open, or it is (re)connecting while GET /api/machine still succeeds. */
export function onlineOf(f: Pick<LaserFacts, "link" | "machine" | "httpError">): boolean {
  return f.link === "open" || (f.link === "connecting" && f.machine !== null && f.httpError === null);
}
