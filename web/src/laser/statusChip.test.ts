// Unit tests for laser/statusChip.ts (`chipState(machine, link, job)` and the banner rule) and laser/escFraming.ts
// (Esc is free to stop framing only when nothing else took it).

import assert from "node:assert/strict";
import { test } from "node:test";
import { IDLE_JOB, jobStatus } from "../api/job.ts";
import type { MachineStatus } from "../api/types.ts";
import { escFreeForFraming } from "./escFraming.ts";
import { BANNER_GRACE_MS, bannerVisible, chipState } from "./statusChip.ts";

function machine(over: Partial<MachineStatus> = {}): MachineStatus {
  return { connected: true, busy: false, framing: false, running: false, runs: [], pending_runs: [], ...over } as MachineStatus;
}

const RUNNING = jobStatus({ active: true, state: "running", progress: 0.4237, remaining_s: 192, elapsed_s: 100 });

test("offline and connecting", () => {
  assert.equal(chipState(null, "closed").kind, "offline");
  assert.equal(chipState(null, "closed").tone, "bad");
  assert.equal(chipState(machine(), "closed").kind, "offline", "a closed link is offline even with an old status");
  assert.equal(chipState(null, "connecting").kind, "connecting");
  assert.equal(chipState(null, "connecting", null, "engine unreachable").kind, "offline");
  // Reconnecting while GET /api/machine still answers: the chip keeps the machine state.
  assert.equal(chipState(machine(), "connecting").kind, "idle");
  assert.equal(chipState(null, "open").kind, "connecting");
});

test("idle, simulator, no laser, busy, pending approvals", () => {
  const idle = chipState(machine(), "open");
  assert.equal(idle.kind, "idle");
  assert.equal(idle.text, "Idle");
  assert.equal(idle.action, null);
  assert.equal(chipState(machine({ sim: true }), "open").text, "Idle (simulator)");
  assert.equal(chipState(machine({ connected: false }), "open").kind, "noLaser");
  assert.equal(chipState(machine({ busy: true }), "open").kind, "busy");
  const pend = chipState(machine({ pending_runs: [{ id: "r1", state: "pending" }] }), "open");
  assert.equal(pend.pending, 1);
  assert.equal(pend.tone, "armed");
});

test("framing: pulse and an inline Stop frame", () => {
  const f = chipState(machine({ framing: true }), "open");
  assert.equal(f.kind, "framing");
  assert.equal(f.action, "stopFrame");
  assert.equal(f.pulse, true);
  assert.equal(f.tone, "armed");
});

test("running 42% · 3:12 with Pause; paused with Resume", () => {
  const r = chipState(machine({ running: true }), "open", RUNNING);
  assert.equal(r.kind, "running");
  assert.equal(r.text, "Running 42% · 3:12");
  assert.equal(r.pct, 42);
  assert.equal(r.action, "pause");
  const p = chipState(machine({ running: true }), "open", { ...RUNNING, paused: true });
  assert.equal(p.kind, "paused");
  assert.equal(p.action, "resume");
  assert.match(p.text, /Paused 42%/);
  // Finishing: no "0:00 left".
  assert.equal(chipState(machine(), "open", { ...RUNNING, finishing: true }).text, "Running 42%");
  // The job from GET /api/machine when no job status is given.
  assert.equal(chipState(machine({ job: { active: true, state: "running", progress: 0.5, remaining_s: 61 } }), "open").text, "Running 50% · 1:01");
  // An approved run that has not reported progress yet.
  assert.equal(chipState(machine({ runs: [{ id: "r", state: "approved" }] }), "open", IDLE_JOB).kind, "running");
  assert.equal(chipState(machine(), "open", { ...RUNNING, state: "waiting_for_start_input" }).kind, "pedal");
});

test("framing wins over a stale job line", () => {
  assert.equal(chipState(machine({ framing: true }), "open", IDLE_JOB).kind, "framing");
});

test("banner: at once when closed, after the grace while reconnecting, never while online", () => {
  assert.equal(bannerVisible(true, "closed", 99999), false);
  assert.equal(bannerVisible(false, "closed", 0), true);
  assert.equal(bannerVisible(false, "connecting", 0), false);
  assert.equal(bannerVisible(false, "connecting", BANNER_GRACE_MS), true);
  assert.ok(BANNER_GRACE_MS < 3000, "shows within 3 s of losing the engine");
});

test("Esc stops framing only when nothing else took it", () => {
  const key = (over: Record<string, unknown> = {}) =>
    ({ key: "Escape", defaultPrevented: false, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, target: null, ...over }) as unknown as KeyboardEvent;
  const none = { querySelector: () => null };
  const some = { querySelector: () => ({}) as Element };
  assert.equal(escFreeForFraming(key(), none), true);
  assert.equal(escFreeForFraming(key({ key: "Enter" }), none), false);
  assert.equal(escFreeForFraming(key({ defaultPrevented: true }), none), false);
  assert.equal(escFreeForFraming(key({ ctrlKey: true }), none), false);
  assert.equal(escFreeForFraming(key({ target: { tagName: "INPUT" } }), none), false);
  assert.equal(escFreeForFraming(key(), some), false, "an open menu, dialog or popover takes Esc first");
});
