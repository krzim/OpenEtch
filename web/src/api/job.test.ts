// Unit tests for api/job.ts: job status parsing, the run and frame request bodies, live-framing keys and the preview
// timeline's time maths.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  DEFAULT_FRAME_OPTIONS,
  DEFAULT_RUN_OPTIONS,
  IDLE_JOB,
  canPause,
  canResume,
  clockText,
  frameKeyEdit,
  frameRequestBody,
  iterationText,
  jobStateKey,
  jobStatus,
  normalizeFrameOptions,
  normalizeRunOptions,
  progressPct,
  repeating,
  runOptionsSummary,
  runRequestBody,
  segmentTimes,
  segmentsDoneAt,
  startHereSegment,
  stopAtReached,
  timeOfSegments,
} from "./job.ts";
import type { Preview } from "./types.ts";

test("job status from GET /api/job (the engine's idle answer and a running one)", () => {
  const idle = jobStatus({
    active: false,
    completed: 0,
    count: 3,
    elapsed_s: 0,
    end_after_current: false,
    estimated_time_s: 0,
    iteration: 0,
    iterations: null,
    packets_sent: 0,
    packets_total: 0,
    part: 0,
    parts: 0,
    paused: false,
    progress: 0,
    remaining_s: 0,
    run_id: null,
    state: "idle",
    stop_at: null,
  });
  assert.equal(idle.active, false);
  assert.equal(idle.count, 3);
  assert.equal(jobStateKey(idle).key, "job.idle");
  assert.equal(canPause(idle), false);
  assert.equal(canResume(idle), false);

  const run = jobStatus({ active: true, state: "running", iteration: 2, iterations: 5, progress: 0.4237, remaining_s: 12, count: 7, stop_at: 7 });
  assert.equal(canPause(run), true);
  assert.equal(repeating(run), true);
  assert.equal(iterationText(run), "Run 2 of 5");
  assert.equal(progressPct(run), 42.4);
  assert.equal(stopAtReached(run), true);

  const paused = jobStatus({ active: true, state: "paused", paused: true, iterations: null, iteration: 3 });
  assert.equal(canResume(paused), true);
  assert.equal(canPause(paused), false);
  assert.equal(jobStateKey(paused).key, "job.paused");
  assert.equal(iterationText(paused), "Run 3 (continuous)");
  assert.equal(jobStateKey(jobStatus({ active: true, state: "waiting_for_start_input" })).key, "job.waitingPedal");

  // Junk is idle; numbers are clamped.
  assert.deepEqual(jobStatus(null), IDLE_JOB);
  assert.deepEqual(jobStatus([1, 2]), IDLE_JOB);
  const odd = jobStatus({ active: true, progress: 7, remaining_s: -3, iterations: 0, stop_at: -1 });
  assert.equal(odd.progress, 1);
  assert.equal(odd.remaining_s, 0);
  assert.equal(odd.iterations, null);
  assert.equal(odd.stop_at, null);
  assert.equal(iterationText(jobStatus({ active: true, iterations: 1, iteration: 1 })), "");
});

test("run request bodies follow LightBurn's Laser window options", () => {
  assert.deepEqual(runRequestBody(DEFAULT_RUN_OPTIONS), { origin: "ui" });
  assert.deepEqual(runRequestBody({ ...DEFAULT_RUN_OPTIONS, repeat: 5 }), { origin: "ui", repeat: 5 });
  // Continuous wins over repeat.
  assert.deepEqual(runRequestBody({ ...DEFAULT_RUN_OPTIONS, repeat: 5, continuous: true }), { origin: "ui", continuous: true });
  // Full pulse is the engine's default, so only "off" is sent.
  assert.deepEqual(runRequestBody({ ...DEFAULT_RUN_OPTIONS, startInput: true }), { origin: "ui", start_input: true });
  assert.deepEqual(runRequestBody({ ...DEFAULT_RUN_OPTIONS, startInput: true, fullPulse: false }), { origin: "ui", start_input: true, full_pulse: false });
  assert.deepEqual(runRequestBody({ ...DEFAULT_RUN_OPTIONS, cutSelected: true }, { selection: ["obj-1", "obj-2"] }), { origin: "ui", selection: ["obj-1", "obj-2"] });
  assert.throws(() => runRequestBody({ ...DEFAULT_RUN_OPTIONS, cutSelected: true }, { selection: [] }), /nothing is selected/);
  // Start Here never sends the selection (the engine refuses both together).
  assert.deepEqual(runRequestBody({ ...DEFAULT_RUN_OPTIONS, cutSelected: true }, { selection: ["obj-1"], fromSegment: 12 }), { origin: "ui", from_segment: 12 });
  assert.throws(() => runRequestBody(DEFAULT_RUN_OPTIONS, { fromSegment: -1 }), /segment/);
  assert.match(runOptionsSummary({ ...DEFAULT_RUN_OPTIONS, repeat: 3, cutSelected: true }, 2), /3 times.*2 selected objects/);
});

test("stored run options are clamped", () => {
  assert.deepEqual(normalizeRunOptions(null), DEFAULT_RUN_OPTIONS);
  assert.equal(normalizeRunOptions({ repeat: 99999 }).repeat, 10000);
  assert.equal(normalizeRunOptions({ repeat: -4 }).repeat, 1);
  assert.equal(normalizeRunOptions({ fullPulse: false }).fullPulse, false);
});

test("frame request bodies leave out options that do not apply", () => {
  assert.deepEqual(frameRequestBody(DEFAULT_FRAME_OPTIONS), { mode: "hull", continuous: true, live: true });
  const o = { ...DEFAULT_FRAME_OPTIONS, style: "outline" as const, individually: true, outsideOnly: true, pointer: "both" as const, blanking: false, live: true, speedMmS: 1500, continuous: false };
  assert.deepEqual(frameRequestBody(o), { mode: "outline", outside_only: true, continuous: false, speed_mm_s: 1500, pointer: "both", blanking: false, live: true });
  assert.deepEqual(frameRequestBody({ ...DEFAULT_FRAME_OPTIONS, style: "hull", individually: true, outsideOnly: true }), { mode: "hull", individually: true, continuous: true, live: true });
  assert.deepEqual(frameRequestBody({ ...DEFAULT_FRAME_OPTIONS, selectionOnly: true, toolLayersOnly: true }, ["obj-3"]), { mode: "hull", ids: ["obj-3"], continuous: true, live: true });
  assert.deepEqual(frameRequestBody({ ...DEFAULT_FRAME_OPTIONS, toolLayersOnly: true }), { mode: "hull", tool_layers_only: true, continuous: true, live: true });
  assert.throws(() => frameRequestBody({ ...DEFAULT_FRAME_OPTIONS, selectionOnly: true }, []), /Select shapes/);
  assert.equal(normalizeFrameOptions({ style: "contour" }).style, "outline");
  assert.equal(normalizeFrameOptions({ style: "star", pointer: "laser" }).pointer, "red");
  assert.equal(normalizeFrameOptions({ speedMmS: -2 }).speedMmS, null);
});

test("live framing keys match LightBurn's", () => {
  const n = { shift: false, ctrl: false };
  assert.deepEqual(frameKeyEdit("ArrowRight", n), { translate: [5, 0] });
  assert.deepEqual(frameKeyEdit("ArrowUp", { shift: true, ctrl: false }), { translate: [0, 20] });
  assert.deepEqual(frameKeyEdit("ArrowDown", { shift: false, ctrl: true }), { translate: [0, -0.5] });
  assert.deepEqual(frameKeyEdit(".", n), { rotate_deg: -15 });
  assert.deepEqual(frameKeyEdit(",", { shift: true, ctrl: false }), { rotate_deg: 1.5 });
  assert.deepEqual(frameKeyEdit(",", { shift: false, ctrl: true }), { rotate_deg: 45 });
  const up = frameKeyEdit("PageUp", n);
  const down = frameKeyEdit("PageDown", n);
  assert.ok(up && "scale" in up && up.scale > 1);
  assert.ok(down && "scale" in down && down.scale < 1);
  assert.equal(frameKeyEdit("a", n), null);
});

const seg = (kind: "jump" | "mark", x0: number, x1: number, t_s?: number) => ({ kind, from: [x0, 0] as [number, number], to: [x1, 0] as [number, number], ...(t_s === undefined ? {} : { t_s }) });

test("timeline: segment times, time to segment count and back", () => {
  const pv: Preview = { segments: [seg("jump", 0, 1, 0.01), seg("mark", 1, 11, 0.03), seg("mark", 11, 21, 0.05), seg("jump", 21, 0, 0.06)] };
  const times = segmentTimes(pv);
  assert.deepEqual(times, [0.01, 0.03, 0.05, 0.06]);
  assert.equal(segmentsDoneAt(times, 0), 0);
  assert.equal(segmentsDoneAt(times, 0.01), 1);
  assert.equal(segmentsDoneAt(times, 0.04), 2);
  assert.equal(segmentsDoneAt(times, 1), 4);
  assert.equal(timeOfSegments(times, 0), 0);
  assert.equal(timeOfSegments(times, 2), 0.03);
  assert.equal(timeOfSegments(times, 99), 0.06);
  // Never decreasing, even if the engine's rounding is not.
  assert.deepEqual(segmentTimes({ segments: [seg("mark", 0, 1, 0.2), seg("mark", 1, 2, 0.1)] }), [0.2, 0.2]);
  // Older engines without t_s: the estimate is spread by length.
  const old = segmentTimes({ segments: [seg("mark", 0, 10), seg("mark", 10, 30)], stats: { estimated_time_s: 3 } });
  assert.equal(old.length, 2);
  assert.ok(Math.abs(old[1] - 3) < 1e-9);
  assert.ok(old[0] > 0.9 && old[0] < 1.1);
  assert.deepEqual(segmentTimes(null), []);
});

test("Start Here starts at the first segment not shown", () => {
  const pv: Preview = { segments: [seg("jump", 0, 1, 1), seg("mark", 1, 2, 2), seg("mark", 2, 3, 3)] };
  assert.equal(startHereSegment(pv, 0), 0);
  assert.equal(startHereSegment(pv, 2), 2);
  assert.equal(startHereSegment(pv, 3), null);
  assert.equal(startHereSegment({ segments: [] }, 0), null);
});

test("clock text", () => {
  assert.equal(clockText(7.44), "0:07.4");
  assert.equal(clockText(0), "0:00.0");
  assert.equal(clockText(723), "12:03");
  assert.equal(clockText(3723), "1:02:03");
  assert.equal(clockText(Number.NaN), "0:00.0");
});
