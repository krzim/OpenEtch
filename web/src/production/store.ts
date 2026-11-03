// The production state shared by the cockpit, the Laser dock's Production card, the maintenance chip and the layer
// pause HUD: GET /api/production, fetched again on the engine events that change it (machine `production`,
// `layer_pause`, runs, the job counter, the foot switch cycle, document edits for the slots) and polled, fast (2 s)
// while a "live" user such as the cockpit is mounted, slowly (30 s) otherwise. `job_progress` events update the job
// part at once, so the countdown runs smoothly. React reads it through `useSyncExternalStore`.

import { useEffect, useSyncExternalStore } from "react";
import { onEngineEvent } from "../api/events.ts";
import type { JobStatus } from "../api/job.ts";
import { prodApi } from "./api.ts";
import type { ProductionStatus } from "./model.ts";

export interface ProdSnapshot {
  status: ProductionStatus | null;
  /** The last fetch failed (an older engine without the routes answers 404). */
  error: string | null;
  /** The engine has no production routes. */
  unsupported: boolean;
}

const FAST_MS = 2000;
const SLOW_MS = 30_000;

let snap: ProdSnapshot = { status: null, error: null, unsupported: false };
const subs = new Set<() => void>();
let users = 0;
let liveUsers = 0;
let timer: number | undefined;
let unlisten: (() => void) | null = null;
let inFlight = false;
let again = false;
let debounce: number | undefined;

function emit(): void {
  for (const f of [...subs]) f();
}

function set(next: Partial<ProdSnapshot>): void {
  snap = { ...snap, ...next };
  emit();
}

/** Fetch GET /api/production now (one request at a time; a request asked for meanwhile runs after it). */
export async function refreshProduction(): Promise<void> {
  if (inFlight) {
    again = true;
    return;
  }
  inFlight = true;
  try {
    const status = await prodApi.status();
    set({ status, error: null, unsupported: false });
  } catch (e) {
    const status = (e as { status?: number }).status;
    set({ error: e instanceof Error ? e.message : String(e), unsupported: status === 404 });
  } finally {
    inFlight = false;
    if (again) {
      again = false;
      void refreshProduction();
    }
  }
}

function soon(): void {
  window.clearTimeout(debounce);
  debounce = window.setTimeout(() => void refreshProduction(), 150);
}

const REFETCH_MACHINE = new Set([
  "production",
  "layer_pause",
  "layer_continue",
  "run_approved",
  "run_started",
  "run_finished",
  "job_count",
  "foot_cycle",
  "framing",
  "frame_stopped",
  "stopped",
  "paused",
  "resumed",
  "waiting_for_start_input",
  "device_changed",
]);

function onEvent(ev: { type: string; [k: string]: unknown }): void {
  if (ev.type === "job_progress" && snap.status && ev.status && typeof ev.status === "object") {
    set({ status: { ...snap.status, job: { ...snap.status.job, ...(ev.status as JobStatus) } } });
    return;
  }
  if (ev.type === "machine" && typeof ev.event === "string" && REFETCH_MACHINE.has(ev.event)) soon();
  else if (ev.type === "run_finished" || ev.type === "run_pending" || ev.type === "document_changed" || ev.type === "device_changed") soon();
}

function schedule(): void {
  window.clearInterval(timer);
  timer = window.setInterval(() => void refreshProduction(), liveUsers > 0 ? FAST_MS : SLOW_MS);
}

function subscribe(f: () => void): () => void {
  subs.add(f);
  return () => {
    subs.delete(f);
  };
}

function getSnap(): ProdSnapshot {
  return snap;
}

/** The production state, kept current while the caller is mounted (`live`: poll fast, for the cockpit). */
export function useProduction(live = false): ProdSnapshot {
  const s = useSyncExternalStore(subscribe, getSnap, getSnap);
  useEffect(() => {
    if (users++ === 0) {
      unlisten = onEngineEvent(onEvent);
    }
    if (live) liveUsers++;
    schedule();
    void refreshProduction();
    return () => {
      if (live) liveUsers--;
      if (--users === 0) {
        unlisten?.();
        unlisten = null;
        window.clearInterval(timer);
        timer = undefined;
      } else schedule();
    };
  }, [live]);
  return s;
}

/** Replace the state (tests). */
export function setProductionForTest(status: ProductionStatus | null): void {
  set({ status, error: null, unsupported: false });
}
