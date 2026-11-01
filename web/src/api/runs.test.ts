// Unit tests for api/runs.ts: run lists, the approval gate and error texts, stats rows, and the toasts for
// run_finished, STOP and undo/redo answers.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  APPROVAL_CODE_TEXT,
  activeRuns,
  approvalErrorText,
  approvalHowText,
  approvalNotice,
  approveBlocker,
  footCycle,
  footCycleText,
  footSwitch,
  footSwitchToast,
  formatCountdown,
  formatDuration,
  isStale,
  noPinMode,
  lockSecondsLeft,
  okError,
  pendingRuns,
  pinHint,
  recentRuns,
  runFinishedToast,
  runStatRows,
  stateLabel,
  stopToast,
  validPin,
  withApprovalLock,
} from "./runs.ts";
import type { MachineStatus, PendingRun } from "./types.ts";

const run = (over: Partial<PendingRun> = {}): PendingRun => ({
  id: "run-1",
  state: "pending",
  origin: { kind: "agent", name: "claude" },
  revision: 7,
  stale: false,
  summary: "2 layers",
  stats: { mark_length_mm: 1234.5, jump_length_mm: 56.78, estimated_time_s: 125, segment_count: 900, polyline_count: 4 },
  packet_count: 3,
  ...over,
});

const machine = (over: Partial<MachineStatus> = {}): MachineStatus => ({
  connected: true,
  approval: { pin_set: true, physical_confirm: null },
  ...over,
});

test("pending and active runs are split by state", () => {
  const m = machine({
    runs: [
      run({ id: "a", state: "pending" }),
      run({ id: "b", state: "approved" }),
      run({ id: "c", state: "awaiting_confirm" }),
      run({ id: "d", state: "running" }),
      run({ id: "e", state: "finished" }),
      run({ id: "f", state: "aborted" }),
      { bogus: true } as unknown as PendingRun,
    ],
  });
  assert.deepEqual(pendingRuns(m).map((r) => r.id), ["a"]);
  assert.deepEqual(activeRuns(m).map((r) => r.id), ["b", "c", "d"]);
  // pending_runs wins when present.
  assert.deepEqual(pendingRuns(machine({ pending_runs: [run({ id: "x" })], runs: [run({ id: "a" })] })).map((r) => r.id), ["x"]);
  assert.deepEqual(pendingRuns(null), []);
  assert.deepEqual(activeRuns(null), []);
});

test("stale runs, unset PINs and non-pending runs cannot be approved", () => {
  assert.equal(approveBlocker(run(), machine()), null);
  assert.equal(approveBlocker(run({ stale: true }), machine()), "Document changed - request a new run.");
  assert.match(approveBlocker(run(), machine({ approval: { pin_set: false, physical_confirm: null } })) ?? "", /openetch set-pin/);
  assert.notEqual(approveBlocker(run({ state: "running" }), machine()), null);
  // An engine that does not report `approval` does not block the UI; the engine still checks the PIN.
  assert.equal(approveBlocker(run(), machine({ approval: undefined })), null);
});

test("a pending run planned from another revision than the document shown is stale", () => {
  // The engine has not flagged it yet (its stale flag lags until the next machine refresh).
  assert.equal(isStale(run({ revision: 7 }), 8), true);
  assert.equal(approveBlocker(run({ revision: 7 }), machine(), { docRevision: 8 }), "Document changed - request a new run.");
  assert.equal(approveBlocker(run({ revision: 7 }), machine(), { docRevision: 7 }), null);
  // Unknown revisions (no document yet, or a run without one) do not block.
  assert.equal(approveBlocker(run({ revision: 7 }), machine(), { docRevision: null }), null);
  assert.equal(approveBlocker(run({ revision: 7 }), machine(), { docRevision: -1 }), null);
  assert.equal(approveBlocker(run({ revision: undefined }), machine(), { docRevision: 8 }), null);
  assert.equal(isStale(run({ stale: true }), 7), true);
});

test("a changed PIN file blocks approvals until the engine restarts", () => {
  const m = machine({ approval: { pin_set: true, physical_confirm: null, pin_changed: true } });
  assert.equal(approveBlocker(run(), m), "PIN file changed - restart the engine.");
  assert.equal(approvalNotice(m), "PIN file changed - restart the engine.");
  assert.equal(approvalErrorText("pin_changed", "x"), "PIN file changed - restart the engine.");
  assert.equal(approvalNotice(machine({ approval: { pin_set: true, physical_confirm: null, pin_changed: false } })), null);
});

test("a PIN lockout blocks approvals with a countdown until the engine reports it unlocked", () => {
  const now = 1_000_000;
  const locked = machine({ approval: { pin_set: true, physical_confirm: null, locked_until_unix_s: now + 125 } });
  assert.equal(lockSecondsLeft(locked, now), 125);
  assert.equal(approveBlocker(run(), locked, { nowUnixS: now }), "Too many wrong PINs. Approvals unlock in 2 min 05 s.");
  assert.equal(approvalNotice(locked, now + 100), "Too many wrong PINs. Approvals unlock in 25 s.");
  // Past the deadline, or null from the engine: unlocked.
  assert.equal(lockSecondsLeft(locked, now + 125), 0);
  assert.equal(approveBlocker(run(), locked, { nowUnixS: now + 126 }), null);
  const unlocked = machine({ approval: { pin_set: true, physical_confirm: null, locked_until_unix_s: null } });
  assert.equal(approveBlocker(run(), unlocked, { nowUnixS: now }), null);
  // A clock far behind the engine's never counts past the 1 h cap.
  const far = machine({ approval: { pin_set: true, physical_confirm: null, locked_until_unix_s: now + 86_400 } });
  assert.equal(lockSecondsLeft(far, now), 3600);
  // A stale run says so first.
  assert.equal(approveBlocker(run({ stale: true }), locked, { nowUnixS: now }), "Document changed - request a new run.");
  assert.equal(formatCountdown(0.2), "1 s");
  assert.equal(formatCountdown(60), "1 min 00 s");
});

test("an approval_locked event updates the machine status at once", () => {
  const m = machine();
  const locked = withApprovalLock(m, 1234);
  assert.equal(locked?.approval?.locked_until_unix_s, 1234);
  assert.equal(m.approval?.locked_until_unix_s, undefined, "the old status is not mutated");
  assert.equal(withApprovalLock(m, "soon"), m);
  assert.equal(withApprovalLock(machine({ approval: undefined }), 1234)?.approval, undefined);
  assert.equal(withApprovalLock(null, 1234), null);
});

test("PIN must be 6-12 digits", () => {
  assert.ok(validPin("123456"));
  assert.ok(validPin("123456789012"));
  assert.ok(!validPin("1234"));
  assert.ok(!validPin("12345"));
  assert.ok(!validPin("1234567890123"));
  assert.ok(!validPin("12a456"));
  assert.ok(!validPin(" 123456"));
  assert.ok(!validPin(""));
  assert.equal(pinHint(""), null);
  assert.equal(pinHint("12345"), "At least 6 digits.");
  assert.equal(pinHint("123456"), null);
});

test("approval error codes have specific texts", () => {
  assert.match(approvalErrorText("pin_not_set", "x"), /Run 'openetch set-pin' on the engine machine once/);
  assert.equal(approvalErrorText("pin_invalid", "x"), "Wrong PIN.");
  assert.match(approvalErrorText("pin_locked", "x"), /Too many wrong PINs/);
  assert.match(approvalErrorText("pin_required", "x"), /Enter the operator PIN/);
  assert.equal(approvalErrorText("something_new", "engine says no"), "engine says no");
  assert.equal(approvalErrorText(undefined, "engine says no"), "engine says no");
});

test("stats rows show what will fire", () => {
  const rows = Object.fromEntries(runStatRows(run()));
  assert.deepEqual(rows, {
    "Est. time": "2 min 05 s",
    Mark: "1.23 m",
    Jump: "56.8 mm",
    Segments: "900",
    Packets: "3",
    Revision: "r7",
  });
  // plan::Stats has no layer count, so there is no Layers row; missing fields are left out rather than shown as NaN.
  assert.deepEqual(runStatRows({ id: "r", stats: { mark_length_mm: 2 } }), [["Mark", "2.0 mm"]]);
  assert.deepEqual(runStatRows({ id: "r" }), []);
  assert.deepEqual(runStatRows({ id: "r", stats: { mark_length_mm: 1270, jump_length_mm: 25.4 } }, "in"), [
    ["Mark", "50.00 in"],
    ["Jump", "1.00 in"],
  ]);
});

test("the packet count shows only while a run is pending", () => {
  const packets = (state: string | undefined) => Object.fromEntries(runStatRows(run({ state })))["Packets"];
  assert.equal(packets("pending"), "3");
  assert.equal(packets(undefined), "3");
  // The engine drops the packets once a run is approved, and reports 0 from then on.
  for (const state of ["approved", "awaiting_confirm", "running", "finished", "cancelled"]) {
    assert.equal(packets(state), undefined, state);
  }
});

test("ended runs, cancelled ones included, are listed newest first", () => {
  const m = machine({
    runs: [
      run({ id: "old", state: "finished", created_unix_s: 100 }),
      run({ id: "p", state: "pending", created_unix_s: 500 }),
      run({ id: "c", state: "cancelled", created_unix_s: 300 }),
      run({ id: "a", state: "aborted", created_unix_s: 200 }),
      run({ id: "r", state: "running", created_unix_s: 400 }),
    ],
  });
  assert.deepEqual(recentRuns(m).map((r) => r.id), ["c", "a", "old"]);
  assert.deepEqual(recentRuns(m, 1).map((r) => r.id), ["c"]);
  assert.deepEqual(recentRuns(null), []);
  assert.equal(stateLabel("cancelled"), "Cancelled");
});

test("durations and state labels", () => {
  assert.equal(formatDuration(12.44), "12.4 s");
  assert.equal(formatDuration(3725), "1 h 02 min");
  assert.equal(formatDuration(Number.NaN), "?");
  assert.equal(stateLabel("awaiting_confirm"), "Waiting for the foot pedal");
  assert.equal(stateLabel(undefined), "Waiting for approval");
  assert.equal(stateLabel("weird"), "weird");
});

test("run_finished toasts errors for failures and aborts", () => {
  assert.deepEqual(runFinishedToast({ type: "run_finished", id: "run-2", ok: true, state: "finished", error: null }), {
    msg: "Run run-2 finished.",
    kind: "info",
  });
  const failed = runFinishedToast({ type: "run_finished", id: "run-2", ok: false, state: "failed", error: "usb timeout" });
  assert.equal(failed.kind, "error");
  assert.equal(failed.msg, "Run run-2 failed: usb timeout");
  const aborted = runFinishedToast({ type: "run_finished", run_id: "run-3", ok: false, state: "aborted", error: null });
  assert.deepEqual(aborted, { msg: "Run run-3 was aborted.", kind: "error" });
  const cancelled = runFinishedToast({ type: "run_finished", id: "run-4", ok: false, state: "cancelled", error: "stopped" });
  assert.deepEqual(cancelled, { msg: "Run run-4 was cancelled: stopped", kind: "error" });
});

test("STOP answer: connected, not connected, cancelled runs", () => {
  assert.equal(stopToast({ ok: true }), "Stop sent.");
  assert.equal(stopToast({ ok: true, connected: false }), "No laser connected; nothing to stop.");
  assert.equal(stopToast({ ok: true, cancelled_runs: ["a", "b"] }), "Stop sent. 2 runs cancelled before starting.");
  assert.equal(stopToast({ ok: true, connected: true, cancelled_runs: 1 }), "Stop sent. 1 run cancelled before starting.");
  assert.equal(stopToast(undefined), "Stop sent.");
});

test("200 {ok:false} answers (undo with nothing to undo) are errors", () => {
  assert.equal(okError({ ok: false, error: "nothing to undo" }, "x"), "nothing to undo");
  assert.equal(okError({ ok: false }, "nothing to redo"), "nothing to redo");
  assert.equal(okError({ ok: true, revision: 3 }, "x"), null);
  assert.equal(okError(undefined, "x"), null);
});

test("no-PIN mode: no PIN notices, one-click Start, the foot switch cycle and its toasts", () => {
  const click = machine({
    approval: { pin_set: false, physical_confirm: null, pin_changed: true, mode: "click", foot_switch: { input_bit: 14, active_low: false } },
  });
  assert.equal(noPinMode(click), true);
  // No PIN file is needed: neither "no PIN set" nor "PIN file changed" blocks an approval.
  assert.equal(approvalNotice(click), null);
  assert.equal(approveBlocker(run(), click), null);
  assert.deepEqual(footSwitch(click), { input_bit: 14, active_low: false });
  assert.equal(approvalHowText(click), "No PIN on this engine: Start burns at once; the foot switch (input 14) frames the job, then burns it.");
  const noFoot = machine({ approval: { pin_set: false, physical_confirm: null, mode: "click", foot_switch: null } });
  assert.equal(footSwitch(noFoot), null);
  assert.equal(approvalHowText(noFoot), "No PIN on this engine: Start burns at once.");
  // The PIN mode, and an engine that does not report a mode, keep the PIN.
  const pinModes = [
    machine({ approval: { pin_set: false, physical_confirm: null, mode: "pin", foot_switch: { input_bit: 14, active_low: false } } }),
    machine({ approval: { pin_set: true, physical_confirm: null } }),
  ];
  for (const m of pinModes) {
    assert.equal(noPinMode(m), false);
    assert.equal(footSwitch(m), null);
    assert.match(approvalHowText(m), /^Approval needs the operator PIN/);
  }
  assert.equal(approvalNotice(machine({ approval: { pin_set: false, physical_confirm: null, mode: "pin" } })), APPROVAL_CODE_TEXT.pin_not_set);
  assert.match(approvalErrorText("browser_required", "x"), /localhost or https/);

  // The foot switch cycle: what a press does now, by phase; none without a foot switch or in the PIN mode.
  const cycle = (phase: "off" | "framing" | "burning") =>
    machine({
      approval: { pin_set: false, physical_confirm: null, mode: "click", foot_switch: { input_bit: 14, active_low: true } },
      foot_cycle: { available: true, phase, revision: phase === "off" ? null : 4, run_id: null, since_unix_s: null, idle_timeout_s: 600, notice: null },
    });
  assert.equal(footCycleText(cycle("off")), "Foot switch (input 14): press to frame the job, press again to burn it.");
  assert.equal(footCycleText(cycle("framing")), "Framing. Press the foot switch to burn.");
  assert.match(footCycleText(cycle("burning")) ?? "", /^Burning\. Framing resumes/);
  assert.equal(footCycle(cycle("framing"))?.revision, 4);
  const unavailable = machine({
    approval: { pin_set: false, physical_confirm: null, mode: "click", foot_switch: null },
    foot_cycle: { available: false, phase: "off", revision: null, run_id: null, since_unix_s: null, idle_timeout_s: 600, notice: null },
  });
  assert.equal(footCycleText(unavailable), null);
  assert.equal(footCycle(machine({ approval: { pin_set: true, physical_confirm: null, mode: "pin" }, foot_cycle: cycle("framing").foot_cycle })), null);
  assert.equal(footCycleText(noFoot), null, "an engine without foot_cycle shows no cycle line");

  // The engine's notices: an error when nothing was burned, else information.
  assert.deepEqual(footSwitchToast("foot_cycle_notice", "Foot switch: not burned: the door interlock (input 15) is open. Still framing."), {
    msg: "Foot switch: not burned: the door interlock (input 15) is open. Still framing.",
    kind: "error",
  });
  assert.equal(footSwitchToast("foot_cycle_notice", "Foot switch: the job changed since it was framed, so it was not burned. Framing the new version: press again to burn it.")?.kind, "error");
  assert.equal(footSwitchToast("foot_cycle_notice", "Framing was stopped, so the foot switch cycle ended. Press the foot switch to frame again.")?.kind, "info");
  assert.equal(footSwitchToast("foot_cycle_notice", ""), null);
  assert.equal(footSwitchToast("foot_switch_approved", "run-7"), null, "the old single-run events are gone");
  assert.equal(footSwitchToast("run_approved", "run-7"), null);
});
