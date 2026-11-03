// Unit tests of the production model (production/model.ts): the cockpit's big panel, the counter, slots and the
// ledger as rows, and the checks of the Settings drafts.

import assert from "node:assert/strict";
import { test } from "node:test";
import { IDLE_JOB } from "../api/job.ts";
import {
  clock,
  cockpitPhase,
  counterText,
  extractorProblem,
  isCockpitHash,
  ledgerRows,
  newTarget,
  nextValues,
  reminderProblem,
  shortDuration,
  slotLine,
  targetProblem,
  toggleEmpty,
  type ProductionStatus,
  type Slot,
} from "./model.ts";

function slot(n: number, over: Partial<Slot> = {}): Slot {
  return {
    number: n,
    offset: n - 1,
    label: `Pocket ${n}`,
    objects: [`obj-${n}`],
    empty: false,
    past_end: false,
    value: 6 + n,
    values: [{ object: `obj-${n}`, mode: "serial", shown: `SN 00${6 + n}` }],
    ...over,
  };
}

function status(over: Partial<ProductionStatus> = {}): ProductionStatus {
  return {
    revision: 3,
    document: "Discs",
    project: null,
    device: "b6",
    approval_mode: "click",
    lock: { locked: false, holder: null, label: null, since_unix_s: null, lease_s: 0 },
    job: { ...IDLE_JOB },
    foot_cycle: { available: true, phase: "off", revision: null, run_id: null, since_unix_s: null, idle_timeout_s: 600, notice: null },
    slots: { slots: [slot(1), slot(2), slot(3)], current: 7, start: 1, end: 100, stop_at_end: false, auto_advance: true, advance: 3, exhausted: false },
    slots_refusal: null,
    layer_pause: { waiting: false, run_id: null, layer: null, since_unix_s: null, foot_switch: false },
    cycle: { last_burn_s: 20, last_cycle_s: 40, average_cycle_s: 40, count: 3, stop_at: 10, left: 7, eta_s: 280 },
    maintenance: { device: "b6", hours: 1, measured_hours: 1, estimated_hours: 0, runs: 3, since_unix_s: 1, due: [], reminders: [] },
    extractor: { configured: false, config: { output: { kind: "none" }, auto: false, run_on_s: 0, gate: { kind: "none" } }, state: { on: false, since_unix_s: null, off_at_unix_s: null, last_error: null }, busy_output: null },
    ledger: [],
    notifications: { targets: 0, last: null },
    ...over,
  };
}

test("clocks and durations", () => {
  assert.equal(clock(42), "0:42");
  assert.equal(clock(220), "3:40");
  assert.equal(clock(3723), "1:02:03");
  assert.equal(clock(-5), "0:00");
  assert.equal(shortDuration(23.44), "23.4 s");
  assert.equal(shortDuration(95), "1:35");
  assert.equal(shortDuration(null), "–");
});

test("the big panel: the most urgent state first", () => {
  assert.equal(cockpitPhase(null, true, null).kind, "offline");
  assert.equal(cockpitPhase(status(), false, null).kind, "offline");
  const idle = cockpitPhase(status(), true, { connected: true });
  assert.equal(idle.kind, "idle");
  assert.equal(idle.seconds, 280, "the time left to Stop At");
  // A layer pause wins over a running job and counts the wait.
  const layer = cockpitPhase(
    status({ layer_pause: { waiting: true, run_id: "run-1", layer: "Frost", since_unix_s: 100, foot_switch: true }, job: { ...IDLE_JOB, active: true, state: "running" } }),
    true,
    null,
    142,
  );
  assert.equal(layer.kind, "layer");
  assert.equal(layer.seconds, 42);
  assert.ok(layer.waitingForYou && layer.detail.includes("Frost") && layer.detail.includes("foot switch"), layer.detail);
  const running = cockpitPhase(status({ job: { ...IDLE_JOB, active: true, state: "running", progress: 0.426, remaining_s: 75 } }), true, null);
  assert.deepEqual([running.kind, running.pct, running.seconds], ["running", 42, 75]);
  const paused = cockpitPhase(status({ job: { ...IDLE_JOB, active: true, state: "paused", paused: true } }), true, null);
  assert.equal(paused.kind, "paused");
  const foot = cockpitPhase(status({ foot_cycle: { available: true, phase: "framing", revision: 3, run_id: null, since_unix_s: 1000, idle_timeout_s: 600, notice: null } }), true, null, 1100);
  assert.deepEqual([foot.kind, foot.seconds, foot.waitingForYou], ["ready_foot", 500, true]);
  const target = cockpitPhase(status({ cycle: { ...status().cycle, count: 10, stop_at: 10 } }), true, null);
  assert.equal(target.kind, "target");
  assert.equal(cockpitPhase(status({ slots_refusal: "every value is used" }), true, null).kind, "exhausted");
  assert.equal(cockpitPhase(status(), true, { framing: true }).kind, "framing");
  assert.equal(cockpitPhase(status(), true, { connected: false }).kind, "offline");
});

test("counter, slots and next values", () => {
  assert.deepEqual(counterText(status().cycle), { text: "3 / 10", share: 0.3 });
  assert.deepEqual(counterText({ ...status().cycle, stop_at: null }), { text: "3", share: null });
  const plan = status().slots;
  assert.equal(nextValues(plan), "SN 007 – SN 009");
  assert.deepEqual(toggleEmpty(plan, 1), [1]);
  const withEmpty = { ...plan, slots: [slot(1), slot(2, { empty: true }), slot(3, { past_end: true })] };
  assert.deepEqual(toggleEmpty(withEmpty, 1), []);
  assert.equal(nextValues(withEmpty), "SN 007");
  assert.equal(slotLine(withEmpty.slots[1]).state, "empty");
  assert.equal(slotLine(withEmpty.slots[2]).state, "past_end");
  assert.equal(nextValues({ ...plan, slots: [] }), "–");
  // A tag's text and its QR code carry the same number: the slot shows it once.
  const twice = slot(1, { values: [{ object: "t1", mode: "serial", shown: "SN 0001" }, { object: "q1", mode: "serial", shown: "SN 0001" }] });
  assert.equal(slotLine(twice).value, "SN 0001");
});

test("ledger rows: one per slot, newest run first", () => {
  const rows = ledgerRows([
    { time_unix_s: 1, run_id: "run-1", state: "finished", device: "b6", project: null, document: "Discs", completed: 1, counter: 7, slots: [{ number: 1, offset: 0, label: "P1", value: 7, values: [{ object: "o", mode: "serial", shown: "SN 007" }], marked: true }] },
    {
      time_unix_s: 2,
      run_id: "run-2",
      state: "finished",
      device: "b6",
      project: "coins",
      document: "Discs",
      completed: 1,
      counter: 7,
      slots: [{ number: 1, offset: 0, label: "P1", value: 7, values: [], marked: true }],
      remark_of: { run_id: "run-1", slot: 1 },
    },
  ]);
  assert.deepEqual(
    rows.map((r) => [r.run_id, r.project, r.shown, r.remarkOf]),
    [
      ["run-2", "coins", "7", "run-1 #1"],
      ["run-1", "Discs", "SN 007", null],
    ],
  );
});

test("drafts are checked before saving", () => {
  const ntfy = newTarget("ntfy", 1);
  assert.ok(targetProblem(ntfy, false), "no topic yet");
  assert.equal(targetProblem({ ...ntfy, topic: "laser-shop" }, false), null);
  assert.ok(targetProblem({ ...ntfy, topic: "x", url: "ftp://x" }, false));
  const ha = newTarget("home_assistant", 1);
  assert.ok(targetProblem(ha, false), "needs a token");
  assert.equal(targetProblem(ha, true), null, "a stored token is kept");
  assert.ok(targetProblem({ ...ha, token: "a b" }, false));
  assert.equal(reminderProblem({ name: "Clean the lens", every_hours: 40 }), null);
  assert.ok(reminderProblem({ name: "Clean the lens" }));
  assert.ok(reminderProblem({ name: " ", every_runs: 5 }));
  assert.ok(reminderProblem({ name: "x", every_runs: 0.5 }));
  const ex = { output: { kind: "none" as const }, auto: false, run_on_s: 30, gate: { kind: "none" as const } };
  assert.equal(extractorProblem(ex), null);
  assert.ok(extractorProblem({ ...ex, run_on_s: 5000 }));
  assert.ok(extractorProblem({ ...ex, gate: { kind: "switched_on" } }));
  assert.ok(extractorProblem({ ...ex, output: { kind: "home_assistant", target: "t", entity_id: "Fan" } }));
  assert.equal(extractorProblem({ ...ex, output: { kind: "home_assistant", target: "t", entity_id: "switch.fan" } }), null);
  assert.ok(extractorProblem({ ...ex, gate: { kind: "input", bit: 16 } }));
});

test("the cockpit's route", () => {
  assert.ok(isCockpitHash("#/cockpit"));
  assert.ok(isCockpitHash("#/cockpit?x=1"));
  assert.ok(!isCockpitHash("#/cockpits") && !isCockpitHash(""));
});
