// Unit tests for api/runs.ts: run lists, the approval gate and error texts, stats rows, and the toasts for
// run_finished, STOP and undo/redo answers.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  activeRuns,
  approvalErrorText,
  approveBlocker,
  formatDuration,
  okError,
  pendingRuns,
  runFinishedToast,
  runStatRows,
  stateLabel,
  stopToast,
  validPin,
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

test("PIN must be 4-12 digits", () => {
  assert.ok(validPin("1234"));
  assert.ok(validPin("123456789012"));
  assert.ok(!validPin("123"));
  assert.ok(!validPin("1234567890123"));
  assert.ok(!validPin("12a4"));
  assert.ok(!validPin(" 1234"));
  assert.ok(!validPin(""));
});

test("approval error codes have specific texts", () => {
  assert.match(approvalErrorText("pin_not_set", "x"), /Run 'openetch set-pin' on the engine machine once/);
  assert.equal(approvalErrorText("pin_invalid", "x"), "Wrong PIN.");
  assert.match(approvalErrorText("pin_locked", "x"), /locked for 60 s/);
  assert.match(approvalErrorText("pin_required", "x"), /Enter the operator PIN/);
  assert.equal(approvalErrorText("something_new", "engine says no"), "engine says no");
  assert.equal(approvalErrorText(undefined, "engine says no"), "engine says no");
});

test("stats rows show what will fire", () => {
  const rows = Object.fromEntries(runStatRows(run({ stats: { ...run().stats, layer_count: 2 } })));
  assert.equal(rows["Est. time"], "2 min 05 s");
  assert.equal(rows["Mark"], "1.23 m");
  assert.equal(rows["Jump"], "56.8 mm");
  assert.equal(rows["Layers"], "2");
  assert.equal(rows["Packets"], "3");
  assert.equal(rows["Revision"], "r7");
  // Layers as a list; missing fields are left out rather than shown as NaN.
  const sparse = Object.fromEntries(runStatRows({ id: "r", stats: { layers: ["a", "b", "c"] } }));
  assert.deepEqual(sparse, { Layers: "3" });
  assert.deepEqual(runStatRows({ id: "r" }), []);
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
});

test("STOP answer: connected, not connected, cancelled runs", () => {
  assert.equal(stopToast({ ok: true }), "Stop sent.");
  assert.equal(stopToast({ ok: true, connected: false }), "No laser connected; nothing to stop.");
  assert.equal(stopToast({ ok: true, cancelled_runs: ["a", "b"] }), "Stop sent. 2 pending runs cancelled.");
  assert.equal(stopToast(undefined), "Stop sent.");
});

test("200 {ok:false} answers (undo with nothing to undo) are errors", () => {
  assert.equal(okError({ ok: false, error: "nothing to undo" }, "x"), "nothing to undo");
  assert.equal(okError({ ok: false }, "nothing to redo"), "nothing to redo");
  assert.equal(okError({ ok: true, revision: 3 }, "x"), null);
  assert.equal(okError(undefined, "x"), null);
});
