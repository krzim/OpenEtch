// Unit tests for laser/machineRows.ts: friendly labels, and no snake_case key outside Advanced.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { MachineStatus } from "../api/types.ts";
import { machineRows, sentenceOf } from "./machineRows.ts";

/** GET /api/machine from the simulator (trimmed), as the audit saw it. */
const SIM = {
  approval: { foot_switch: { active_low: false, input_bit: 14 }, mode: "click", pin_set: false },
  busy: false,
  connected: true,
  device: { display_name: "ComMarker B6 MOPA 60W (EzCad2)", id: "b6", kind: "jcz_galvo" },
  driver: "lmc",
  field_height_mm: 153.75,
  field_mm: 153.75,
  focus_light: false,
  foot_cycle: { available: true, phase: "off" },
  framing: false,
  job: { active: false },
  laser_source: "JPT M7 MOPA 60W",
  last_error: null,
  link: "sim",
  max_power_pct: 100,
  pending_runs: [],
  profile: "ComMarker B6 MOPA 60W",
  running: false,
  runs: [],
  shutdown: null,
  shutting_down: false,
  sim: true,
  sim_counters: { emitting_packets: 0, packets: 12, singles: 4285 },
  simulated: true,
  state: "idle",
  pulse_widths_ns: [2, 4, 6],
  some_new_key: "x",
} as unknown as MachineStatus;

test("friendly rows: Focus light: Off, the field, the device, the approval", () => {
  const { rows } = machineRows(SIM);
  const get = (l: string) => rows.find((r) => r.label === l)?.value;
  assert.equal(get("Link"), "Connected (Simulator)");
  assert.equal(get("State"), "Idle");
  assert.equal(get("Device"), "ComMarker B6 MOPA 60W (EzCad2)");
  assert.equal(get("Laser source"), "JPT M7 MOPA 60W");
  assert.equal(get("Controller"), "LMC");
  assert.equal(get("Field"), "153.75 × 153.75 mm");
  assert.equal(get("Power limit"), "100%");
  assert.equal(get("Focus light"), "Off");
  assert.equal(get("Approval"), "Start in the app");
  assert.equal(get("Foot switch"), "input 14");
  assert.equal(get("Engine"), undefined, "shutting down only when it is");
  assert.equal(get("Last error"), undefined);
});

test("no snake_case key outside Advanced; the rest goes to Advanced with its raw key", () => {
  const { rows, advanced } = machineRows(SIM);
  for (const r of rows) assert.doesNotMatch(r.label, /_/, r.label);
  const keys = advanced.map(([k]) => k);
  assert.ok(keys.includes("some_new_key"));
  assert.ok(keys.includes("sim_counters.packets"));
  assert.ok(keys.includes("pulse_widths_ns"));
  for (const shown of ["focus_light", "field_height_mm", "shutting_down", "driver", "laser_source"]) assert.ok(!keys.includes(shown), shown);
  // Objects shown elsewhere (runs, approval, job) are not dumped.
  assert.ok(!keys.some((k) => k.startsWith("job") || k.startsWith("approval") || k.startsWith("runs")));
  assert.deepEqual(keys, [...keys].sort());
});

test("errors, shutting down, framing, USB and unknown states", () => {
  const m = { ...SIM, sim: false, simulated: false, link: "usb", connected: false, last_error: "USB timeout", shutting_down: true, state: "waiting_for_start_input" } as MachineStatus;
  const { rows } = machineRows(m);
  const row = (l: string) => rows.find((r) => r.label === l);
  assert.equal(row("Link")?.value, "Not connected (USB)");
  assert.equal(row("Link")?.error, true);
  assert.equal(row("Last error")?.value, "USB timeout");
  assert.equal(row("Engine")?.value, "Shutting down");
  assert.equal(row("State")?.value, "Waiting for start input");
  assert.equal(machineRows({ ...SIM, framing: true } as MachineStatus).rows[1].value, "Framing (red pointer)");
  assert.equal(machineRows({ ...SIM, focus_light: true } as MachineStatus).rows.find((r) => r.label === "Focus light")?.value, "On");
});

test("sentenceOf", () => {
  assert.equal(sentenceOf("waiting_for_start_input"), "Waiting for start input");
  assert.equal(sentenceOf(""), "");
});
