// Job control on the web side (ARCHITECTURE.md, "Job control, calibration and device settings"): the job status of
// GET /api/job and the `job_progress` event, the run options of POST /api/runs (LightBurn's Laser window: repeat, run
// continuously, Start Marking input, Cut Selected Graphics, Start Here), the framing request of
// POST /api/machine/frame (LightBurn's framing styles and options) and the preview timeline's time maths (segment
// `t_s`). Pure: no React and no fetch, so `npm test` covers it.

import type { Preview, PreviewSegment } from "./types.ts";
import { tr } from "../workspace/tr.ts";

// ---- job status -----------------------------------------------------------------------------------------------------

export type JobState = "idle" | "starting" | "waiting_for_start_input" | "running" | "paused";

/** GET /api/job (also `job_progress.status`). */
export interface JobStatus {
  active: boolean;
  run_id: string | null;
  state: JobState | string;
  /** 1-based iteration under way (0 when idle). */
  iteration: number;
  /** Iterations asked for; null = continuous (or idle). */
  iterations: number | null;
  completed: number;
  packets_sent: number;
  packets_total: number;
  part: number;
  parts: number;
  paused: boolean;
  elapsed_s: number;
  estimated_time_s: number;
  remaining_s: number;
  /** The estimate is used up (or under a second is left) while the job still runs: show "Finishing…", never "0 s left". */
  finishing: boolean;
  /** The device's learned correction already in `estimated_time_s` (1: none yet). */
  estimate_correction: number;
  /** 0..1, from the packets the board has finished and the clock; below 1 while the job runs. */
  progress: number;
  /** The job counter of the active device (LightBurn's Count). */
  count: number;
  stop_at: number | null;
  end_after_current: boolean;
}

export const IDLE_JOB: JobStatus = {
  active: false,
  run_id: null,
  state: "idle",
  iteration: 0,
  iterations: null,
  completed: 0,
  packets_sent: 0,
  packets_total: 0,
  part: 0,
  parts: 0,
  paused: false,
  elapsed_s: 0,
  estimated_time_s: 0,
  remaining_s: 0,
  finishing: false,
  estimate_correction: 1,
  progress: 0,
  count: 0,
  stop_at: null,
  end_after_current: false,
};

function num(v: unknown, d = 0): number {
  return typeof v === "number" && Number.isFinite(v) ? v : d;
}

function nonNeg(v: unknown, d = 0): number {
  return Math.max(0, num(v, d));
}

/** GET /api/job (or `job_progress.status`, or `GET /api/machine` `job`) as a `JobStatus`; anything else is idle. */
export function jobStatus(raw: unknown): JobStatus {
  const o = raw && typeof raw === "object" && !Array.isArray(raw) ? (raw as Record<string, unknown>) : null;
  if (!o) return IDLE_JOB;
  const iterations = typeof o.iterations === "number" && o.iterations > 0 ? Math.floor(o.iterations) : null;
  const stopAt = typeof o.stop_at === "number" && o.stop_at >= 0 ? Math.floor(o.stop_at) : null;
  return {
    active: o.active === true,
    run_id: typeof o.run_id === "string" ? o.run_id : null,
    state: typeof o.state === "string" ? o.state : "idle",
    iteration: nonNeg(o.iteration),
    iterations,
    completed: nonNeg(o.completed),
    packets_sent: nonNeg(o.packets_sent),
    packets_total: nonNeg(o.packets_total),
    part: nonNeg(o.part),
    parts: nonNeg(o.parts),
    paused: o.paused === true || o.state === "paused",
    elapsed_s: nonNeg(o.elapsed_s),
    estimated_time_s: nonNeg(o.estimated_time_s),
    remaining_s: nonNeg(o.remaining_s),
    finishing: o.finishing === true,
    estimate_correction: num(o.estimate_correction, 1) > 0 ? num(o.estimate_correction, 1) : 1,
    progress: Math.min(1, nonNeg(o.progress)),
    count: nonNeg(o.count),
    stop_at: stopAt,
    end_after_current: o.end_after_current === true,
  };
}

/** Whether Pause applies (a job holds the machine and is not paused yet). */
export function canPause(j: JobStatus): boolean {
  return j.active && !j.paused;
}

/** Whether Resume applies. */
export function canResume(j: JobStatus): boolean {
  return j.active && j.paused;
}

/** Whether the job repeats (repeat > 1 or continuous), so "End after this run" means something. */
export function repeating(j: JobStatus): boolean {
  return j.active && (j.iterations === null || j.iterations > 1);
}

/** Message key and vars for the job's state line. */
export function jobStateKey(j: JobStatus): { key: "job.idle" | "job.starting" | "job.waitingPedal" | "job.running" | "job.paused"; vars?: Record<string, number> } {
  if (!j.active) return { key: "job.idle" };
  if (j.paused) return { key: "job.paused" };
  switch (j.state) {
    case "starting":
      return { key: "job.starting" };
    case "waiting_for_start_input":
      return { key: "job.waitingPedal" };
    default:
      return { key: "job.running" };
  }
}

/** "Run 2 of 5", "Run 7 (continuous)" or "" for a single run. */
export function iterationText(j: JobStatus): string {
  if (!j.active) return "";
  if (j.iterations === null) return `Run ${Math.max(1, j.iteration)} (continuous)`;
  if (j.iterations <= 1) return "";
  return `Run ${Math.max(1, j.iteration)} of ${j.iterations}`;
}

/** Progress in percent, 0..100, one decimal. */
export function progressPct(j: JobStatus): number {
  return Math.round(j.progress * 1000) / 10;
}

/** The "time left" line: the seconds left, or "Finishing…" once the engine says so (or, from an engine that does not,
 * under a second is left while marking), so a job still running never shows "0 s left". */
export function remainingLine(j: JobStatus): { key: "job.remaining"; time: number } | { key: "job.finishing" } {
  if (j.finishing || (j.active && j.state === "running" && !j.paused && j.remaining_s < 1)) return { key: "job.finishing" };
  return { key: "job.remaining", time: j.remaining_s };
}

/** Whether Stop At blocks the next approval (the engine refuses approvals once the count reaches it). */
export function stopAtReached(j: JobStatus): boolean {
  return j.stop_at !== null && j.count >= j.stop_at;
}

// ---- run options (POST /api/runs) -----------------------------------------------------------------------------------

/** How the next requested run should go (LightBurn's Laser window options). */
export interface RunOptions {
  /** Repeat marking: 1..10000. */
  repeat: number;
  /** Run continuously until stopped, Stop At or "end after the current run". */
  continuous: boolean;
  /** Wait for the foot pedal (the profile's start-marking input) before each run. */
  startInput: boolean;
  /** With startInput: the pedal must be released and pressed again (LightBurn's "Require Full Pulse"). */
  fullPulse: boolean;
  /** Cut Selected Graphics. */
  cutSelected: boolean;
}

export const DEFAULT_RUN_OPTIONS: RunOptions = { repeat: 1, continuous: false, startInput: false, fullPulse: true, cutSelected: false };

export const MAX_REPEAT = 10000;

/** A stored or typed options object, with every value within range. */
export function normalizeRunOptions(raw: unknown): RunOptions {
  const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const r = Math.round(num(o.repeat, 1));
  return {
    repeat: Math.min(MAX_REPEAT, Math.max(1, r)),
    continuous: o.continuous === true,
    startInput: o.startInput === true,
    fullPulse: o.fullPulse !== false,
    cutSelected: o.cutSelected === true,
  };
}

/**
 * The POST /api/runs body for these options. `selection` is the selected object ids (used with Cut Selected Graphics),
 * `fromSegment` the Start Here segment. Throws an Error with a user-facing reason for impossible combinations (the
 * engine refuses them too: selection with Start Here, Cut Selected Graphics with nothing selected).
 */
export function runRequestBody(o: RunOptions, extra: { selection?: readonly string[]; fromSegment?: number | null } = {}): Record<string, unknown> {
  const body: Record<string, unknown> = { origin: "ui" };
  const from = extra.fromSegment ?? null;
  if (o.cutSelected && from === null) {
    if (!extra.selection || extra.selection.length === 0) throw new Error(tr("apij.cutSelectedGraphicsIsOnBut"));
    body.selection = [...extra.selection];
  }
  if (from !== null) {
    if (!Number.isInteger(from) || from < 0) throw new Error(`Start Here needs a segment number (got ${from}).`);
    body.from_segment = from;
  }
  if (o.continuous) body.continuous = true;
  else if (o.repeat > 1) body.repeat = Math.min(MAX_REPEAT, Math.round(o.repeat));
  if (o.startInput) {
    body.start_input = true;
    if (!o.fullPulse) body.full_pulse = false;
  }
  return body;
}

/** A one-line description of the options, for the Request button's title. */
export function runOptionsSummary(o: RunOptions, selected: number): string {
  const parts: string[] = [];
  parts.push(o.continuous ? "run continuously" : o.repeat > 1 ? `${o.repeat} times` : "once");
  if (o.startInput) parts.push(o.fullPulse ? "each run starts on a full foot-pedal press" : "each run starts while the foot pedal is pressed");
  if (o.cutSelected) parts.push(selected ? `only the ${selected} selected object${selected === 1 ? "" : "s"}` : "only the selection (nothing selected)");
  return parts.join(", ");
}

// ---- framing (POST /api/machine/frame) ------------------------------------------------------------------------------

export type FrameStyle = "bounds" | "hull" | "outline";
export type FramePointer = "red" | "focus" | "both";

/** LightBurn's framing options (Galvo Framing). */
export interface FrameOptions {
  style: FrameStyle;
  /** Bounds / hull: one frame per shape (LightBurn's Frame Individually; not for Contour). */
  individually: boolean;
  /** Contour: skip shapes inside others (letter counters). */
  outsideOnly: boolean;
  /** Frame only the shapes on tool layers (output off). */
  toolLayersOnly: boolean;
  /** Frame only the selection. */
  selectionOnly: boolean;
  /** Trace in a loop until stopped, or once. */
  continuous: boolean;
  /** Pointer speed, mm/s; null = the device's frame speed. */
  speedMmS: number | null;
  pointer: FramePointer;
  /** Pointer off between shapes; null = the device's setting. */
  blanking: boolean | null;
  /** Live framing: the frame follows edits (and the keyboard nudges while framing). */
  live: boolean;
  /** LightBurn's Image Bounds: images are framed by their content (non-white pixels), not their rectangle. */
  imageBounds?: boolean;
}

export const DEFAULT_FRAME_OPTIONS: FrameOptions = {
  style: "bounds",
  individually: false,
  outsideOnly: false,
  toolLayersOnly: false,
  selectionOnly: false,
  continuous: true,
  speedMmS: null,
  pointer: "red",
  blanking: null,
  live: false,
};

const STYLES: readonly FrameStyle[] = ["bounds", "hull", "outline"];
const POINTERS: readonly FramePointer[] = ["red", "focus", "both"];

export function normalizeFrameOptions(raw: unknown): FrameOptions {
  const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const d = DEFAULT_FRAME_OPTIONS;
  const speed = typeof o.speedMmS === "number" && Number.isFinite(o.speedMmS) && o.speedMmS > 0 ? o.speedMmS : null;
  return {
    style: STYLES.includes(o.style as FrameStyle) ? (o.style as FrameStyle) : o.style === "contour" ? "outline" : d.style,
    individually: o.individually === true,
    outsideOnly: o.outsideOnly === true,
    toolLayersOnly: o.toolLayersOnly === true,
    selectionOnly: o.selectionOnly === true,
    continuous: o.continuous !== false,
    speedMmS: speed,
    pointer: POINTERS.includes(o.pointer as FramePointer) ? (o.pointer as FramePointer) : d.pointer,
    blanking: typeof o.blanking === "boolean" ? o.blanking : null,
    live: o.live === true,
    imageBounds: o.imageBounds === true,
  };
}

/**
 * The POST /api/machine/frame body. Options that do not apply are left out (individually with Contour, outside only
 * without it, tool layers with a selection). Throws with a reason when the selection is asked for but empty.
 */
export function frameRequestBody(o: FrameOptions, selection: readonly string[] = []): Record<string, unknown> {
  const body: Record<string, unknown> = { mode: o.style };
  if (o.selectionOnly) {
    if (selection.length === 0) throw new Error(tr("apij.selectShapesToFrameOrTurn"));
    body.ids = [...selection];
  } else if (o.toolLayersOnly) body.tool_layers_only = true;
  if (o.style !== "outline" && o.individually) body.individually = true;
  if (o.style === "outline" && o.outsideOnly) body.outside_only = true;
  body.continuous = o.continuous;
  if (o.speedMmS !== null) body.speed_mm_s = o.speedMmS;
  if (o.pointer !== "red") body.pointer = o.pointer;
  if (o.blanking !== null) body.blanking = o.blanking;
  if (o.live) body.live = true;
  if (o.imageBounds) body.image_bounds = true;
  return body;
}

/** Live-framing keys (LightBurn's Galvo Framing): arrows move 5 mm (Shift 20, Ctrl 0.5), PgUp/PgDn scale, ./, rotate. */
export type FrameKeyEdit = { translate: [number, number] } | { scale: number } | { rotate_deg: number } | null;

export function frameKeyEdit(key: string, m: { shift: boolean; ctrl: boolean }): FrameKeyEdit {
  const step = m.ctrl ? 0.5 : m.shift ? 20 : 5;
  switch (key) {
    case "ArrowLeft":
      return { translate: [-step, 0] };
    case "ArrowRight":
      return { translate: [step, 0] };
    case "ArrowUp":
      return { translate: [0, step] };
    case "ArrowDown":
      return { translate: [0, -step] };
    case "PageUp":
      return { scale: m.ctrl ? 1.01 : m.shift ? 1.1 : 1.05 };
    case "PageDown":
      return { scale: m.ctrl ? 1 / 1.01 : m.shift ? 1 / 1.1 : 1 / 1.05 };
    case ".":
    case ">":
      return { rotate_deg: -(m.ctrl ? 45 : m.shift ? 1.5 : 15) };
    case ",":
    case "<":
      return { rotate_deg: m.ctrl ? 45 : m.shift ? 1.5 : 15 };
    default:
      return null;
  }
}

// ---- preview timeline (segment t_s) ---------------------------------------------------------------------------------

type TimedSegment = PreviewSegment & { t_s?: number };

/** Each segment's end time in seconds: the engine's `t_s` when every segment has one, else spread by length. */
export function segmentTimes(preview: Preview | null): number[] {
  const segs = (preview?.segments ?? []) as TimedSegment[];
  if (segs.length === 0) return [];
  if (segs.every((s) => typeof s.t_s === "number" && Number.isFinite(s.t_s))) {
    // Never decreasing, even if the engine rounds.
    const out: number[] = [];
    let last = 0;
    for (const s of segs) {
      last = Math.max(last, s.t_s as number);
      out.push(last);
    }
    return out;
  }
  // Older engines: the job time spread over the segments by length (jumps count a little).
  const total = typeof preview?.stats?.estimated_time_s === "number" && preview.stats.estimated_time_s > 0 ? preview.stats.estimated_time_s : segs.length;
  const w = segs.map((s) => Math.hypot(s.to[0] - s.from[0], s.to[1] - s.from[1]) * (s.kind === "mark" ? 1 : 0.25) + 1e-3);
  const sum = w.reduce((a, b) => a + b, 0);
  let acc = 0;
  return w.map((x) => (acc += (x / sum) * total));
}

/** How many segments have finished at time `t` (the toolpath shown up to that time). */
export function segmentsDoneAt(times: readonly number[], t: number): number {
  let lo = 0;
  let hi = times.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (times[mid] <= t + 1e-9) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** The time at which `n` segments have finished (0 for none). */
export function timeOfSegments(times: readonly number[], n: number): number {
  if (n <= 0 || times.length === 0) return 0;
  return times[Math.min(times.length, n) - 1];
}

/** Playback speeds relative to real time (LightBurn's preview speed). */
export const PLAYBACK_SPEEDS = [0.25, 0.5, 1, 2, 5, 10, 25, 100];

/** "0:07.4" or "12:03" or "1:02:03" for a timeline readout. */
export function clockText(s: number): string {
  if (!Number.isFinite(s) || s < 0) s = 0;
  if (s < 60) return `0:${s.toFixed(1).padStart(4, "0")}`;
  const total = Math.round(s);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const sec = total % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return `${h > 0 ? `${h}:` : ""}${mm}:${String(sec).padStart(2, "0")}`;
}

/**
 * The Start Here segment for the slider position: the first segment not yet shown (the engine starts a jump at the
 * polyline it travels to, and a mark at its own start point, `plan::start_at`); null at the very end or with nothing
 * to run.
 */
export function startHereSegment(preview: Preview | null, done: number): number | null {
  const segs = preview?.segments ?? [];
  if (segs.length === 0 || done >= segs.length) return null;
  return Math.max(0, Math.floor(done));
}
