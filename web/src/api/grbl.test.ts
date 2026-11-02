// Unit tests for api/grbl.ts: the Move window's coordinates and jogs, the console log and history, macros and the
// Save G-code file name.

import assert from "node:assert/strict";
import { test } from "node:test";
import * as G from "./grbl.ts";

const status = {
  profile: { work_area_mm: [400, 300], origin: "bottom_left", homing: true, frame_feed_mm_min: 3000, max_feed_mm_min: 6000, dynamic_power: true, port: "COM9" },
  board: { version: "1.1h", status: { state: "Idle", mpos: [50, 250, 0] }, alarm: null, unsafe_startup: [], spindle_off: true },
  position: { machine: [50, 250], field: [-150, 100] },
  running: false,
  framing: false,
};

test("GET /api/machine/grbl is read defensively", () => {
  const s = G.parseGrblStatus(status);
  assert.ok(s);
  assert.deepEqual(s.profile.work_area_mm, [400, 300]);
  assert.equal(s.profile.homing, true);
  assert.equal(s.state, "Idle");
  assert.deepEqual(s.machine, [50, 250]);
  assert.deepEqual(s.field, [-150, 100]);
  assert.equal(G.parseGrblStatus({ ...status, profile: { work_area_mm: [0, 1] } }), null);
  assert.equal(G.parseGrblStatus(null), null);
  const bare = G.parseGrblStatus({ profile: { work_area_mm: [200, 100], frame_feed_mm_min: 9000, max_feed_mm_min: 5000 }, board: null, position: null });
  assert.ok(bare);
  assert.equal(bare.machine, null);
  assert.equal(bare.profile.origin, "bottom_left");
  assert.equal(bare.profile.frame_feed_mm_min, 5000, "never above the feed limit");
  assert.equal(G.isGrblMachine({ connected: true, driver: "grbl" }), true);
  assert.equal(G.isGrblMachine({ connected: true, driver: "lmc" }), false);
  assert.equal(G.isGrblMachine(null), false);
  assert.deepEqual(G.gantryOf({ connected: true, driver: "grbl", grbl: { profile: { ...status.profile, dynamic_power: false } } }), { dynamic: false });
  assert.deepEqual(G.gantryOf({ connected: true, driver: "grbl" }), { dynamic: true });
  assert.equal(G.gantryOf({ connected: true, driver: "lmc" }), null);
});

test("machine and canvas coordinates follow the origin corner like the engine's", () => {
  const p = G.parseGrblProfile(status.profile)!;
  // The engine's own examples (GrblProfile::to_machine): bottom-left (0, 0) is the canvas's front-left corner.
  assert.deepEqual(G.fieldToMachine(p, [0, 0]), [200, 150]);
  assert.deepEqual(G.fieldToMachine(p, [-200, -150]), [0, 0]);
  assert.deepEqual(G.machineToField(p, [50, 250]), [-150, 100]);
  const rearLeft = { ...p, origin: "top_left" as const };
  assert.deepEqual(G.fieldToMachine(rearLeft, [-190, 140]), [10, 10]);
  const rearRight = { ...p, origin: "top_right" as const };
  assert.deepEqual(G.machineToField(rearRight, [0, 0]), [200, 150], "the origin corner is the rear right");
  for (const q of [p, rearLeft, rearRight, { ...p, origin: "bottom_right" as const }]) {
    const back = G.machineToField(q, G.fieldToMachine(q, [12.5, -40]));
    assert.ok(Math.abs(back[0] - 12.5) < 1e-9 && Math.abs(back[1] + 40) < 1e-9);
  }
  assert.equal(G.positionProblem(p, [400, 300]), null);
  assert.match(G.positionProblem(p, [401, 0]) ?? "", /0…400 × 0…300/);
  assert.match(G.positionProblem(p, [Number.NaN, 0]) ?? "", /both/);
});

test("jogs: screen directions, keys, limits", () => {
  assert.deepEqual(G.jogDelta("n", 10), { dx: 0, dy: 10 });
  assert.deepEqual(G.jogDelta("sw", 0.1), { dx: -0.1, dy: -0.1 });
  assert.equal(Object.is(G.jogDelta("e", 5).dy, -0), false, "no -0 in the body");
  assert.deepEqual(G.jogKey({ key: "ArrowLeft" }, 1), { dir: "w", step: 1 });
  assert.deepEqual(G.jogKey({ key: "ArrowUp", shiftKey: true }, 1), { dir: "n", step: 10 });
  assert.deepEqual(G.jogKey({ key: "ArrowDown", altKey: true }, 1), { dir: "s", step: 0.1 });
  assert.equal(G.jogKey({ key: "ArrowUp", ctrlKey: true }, 1), null, "Ctrl+arrows stay with the canvas");
  assert.equal(G.jogKey({ key: "a" }, 1), null);
  const p = G.parseGrblProfile(status.profile)!;
  assert.equal(G.jogProblem(p, 10, 3000), null);
  assert.match(G.jogProblem(p, 0, 3000) ?? "", /distance/);
  assert.match(G.jogProblem(p, 500, 3000) ?? "", /400/);
  assert.match(G.jogProblem(p, 10, 7000) ?? "", /6000 mm\/min/);
});

test("console log, replies and history", () => {
  const log = G.appendLog([], [{ kind: "sent", text: "$$" }, ...G.replyEntries({ line: "$$", lines: ["$30=1000", "$32=1"], ok: true, error: null })]);
  assert.deepEqual(log.map((e) => e.kind), ["sent", "reply", "reply", "ok"]);
  assert.deepEqual(G.replyEntries({ ok: false, error: "GRBL refused \"G5\": error 20" }), [{ kind: "error", text: "GRBL refused \"G5\": error 20" }]);
  assert.equal(G.appendLog(log, log, 5).length, 5, "kept to the newest lines");
  let h = G.historyPush([], "$$");
  h = G.historyPush(h, "$$");
  h = G.historyPush(h, " G0 X10 ");
  assert.deepEqual(h, ["$$", "G0 X10"]);
  assert.equal(G.historyPush(Array.from({ length: 100 }, (_, i) => `G0 X${i}`), "M8").length, 100);
  assert.deepEqual(G.historyStep(h, h.length, -1), { index: 1, text: "G0 X10" });
  assert.deepEqual(G.historyStep(h, 0, -1), { index: 0, text: "$$" });
  assert.deepEqual(G.historyStep(h, 1, 1), { index: 2, text: "" });
});

test("macros: lines, laser-on hints, the engine's limits and the prefs patch", () => {
  assert.deepEqual(G.macroLines("  $H \n\n; park\n(comment)\nG0 X0 Y0\r\nM8"), ["$H", "G0 X0 Y0", "M8"]);
  for (const on of ["M3 S1000", "m4", "M03", "M 3", "G1 X1 F100 S5", "s 0.001", "G1X1F100S1E3", "M3.0"]) assert.equal(G.looksLaserOn(on), true, on);
  for (const off of ["M5", "M8", "M30", "G1 X10 F500 S0", "$SLP", "$J=G91 X1 F100", "G0 X3 Y4", "(M3) M5"]) assert.equal(G.looksLaserOn(off), false, off);
  const ok: G.Macro[] = [{ name: "Air on", lines: ["M8"] }, { name: "Park", lines: ["$J=G90 G21 X0 Y0 F3000"] }];
  assert.equal(G.macrosProblem(ok), null);
  assert.match(G.macrosProblem([{ name: "Fire", lines: ["M3 S100"] }]) ?? "", /laser on/);
  assert.match(G.macrosProblem([{ name: " ", lines: ["M8"] }]) ?? "", /name/);
  assert.match(G.macrosProblem([{ name: "x", lines: [] }]) ?? "", /no lines/);
  assert.match(G.macrosProblem([{ name: "x", lines: ["?"] }]) ?? "", /realtime/);
  assert.match(G.macrosProblem(Array.from({ length: 33 }, () => ok[0])) ?? "", /32/);
  assert.deepEqual(G.macrosPatch([{ name: " Air on ", lines: ["M8"] }]), { console_macros: [{ name: "Air on", lines: ["M8"] }] });
  assert.deepEqual(G.parseMacros({ device_prefs: { console_macros: [{ name: "A", lines: ["M8", 3] }, { name: 2 }] } }), [{ name: "A", lines: ["M8"] }]);
  assert.deepEqual(G.parseMacros({ device_prefs: {} }), []);
});

test("Save G-code file names", () => {
  assert.equal(G.gcodeFileName("coaster.gcode", "x"), "coaster.gcode");
  assert.equal(G.gcodeFileName(null, "coaster.lbrn2"), "coaster.gcode");
  assert.equal(G.gcodeFileName("weird.bin", "a/b:c"), "a_b_c.gcode");
  assert.equal(G.gcodeFileName(null, null), "job.gcode");
});
