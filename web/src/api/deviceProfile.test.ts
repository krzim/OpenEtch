// Unit tests for api/deviceProfile.ts: the values gathered from the engine's answers, client checks, the merge patch
// and the field an engine refusal names.

import assert from "node:assert/strict";
import { test } from "node:test";
import { allPaths, checkValues, errorField, galvoAxis, merged, profileGroups, profilePatch, profileValues } from "./deviceProfile.ts";

// GET /api/devices/{id} `profile` of the built-in B6 device, as the engine answers.
const SUMMARY = {
  cor_file: "jcz150.cor",
  field_mm: 153.75,
  frequency_khz: [1.0, 4000.0],
  laser_source: "JPT M7 MOPA 60W",
  mark_speed_mm_s: [1.0, 10000.0],
  max_jump_speed_mm_s: 4000.0,
  max_power_pct: 100.0,
  name: "ComMarker B6 MOPA 60W",
  negate_x: false,
  negate_y: false,
  pulse_width_enabled: true,
  swap_xy: false,
  timing: {
    end_delay_us: 300.0,
    jump_delay_max_us: 200.0,
    jump_delay_min_us: 8.0,
    jump_distance_limit_mm: 10.0,
    jump_speed_mm_s: 2000.0,
    laser_off_delay_us: 100.0,
    laser_on_delay_us: -300.0,
    polygon_delay_us: 100.0,
  },
};

// Its .lbdev export (GET /api/devices/{id}/lbdev), with a calibration: galvo 2 is X (not swapped).
const LBDEV = JSON.stringify({
  DeviceList: [
    {
      DisplayName: "B6",
      Settings: {
        FieldAngle: 0.5,
        FieldOffsetX: 0.2,
        FieldOffsetY: -0.1,
        FrameSpeed: 3000.0,
        Galvo_1_Scale: 1.01,
        Galvo_2_Scale: 0.99,
        Galvo_1_Bulge: 1.0,
        Galvo_2_Bulge: 1.002,
        Galvo_1_is_X: false,
        Laser_OpenMODelay: 8000.0,
        StartMarkPortBit: 14,
        MaxSpeed: 10000.0,
      },
    },
  ],
});

const INFO = { axis: null, connected: true, fiber_kind: "mopa", laser_kind: "fiber" };

test("galvo 2 is X unless the axes are swapped (LightBurn's Galvo_1_is_X)", () => {
  assert.equal(galvoAxis(2, false), "x");
  assert.equal(galvoAxis(1, false), "y");
  assert.equal(galvoAxis(1, true), "x");
  assert.equal(galvoAxis(2, true), "y");
  const groups = profileGroups(false);
  assert.equal(groups.find((g) => g.id === "galvo2")?.fields[0].path, "calibration.scale_x");
  assert.equal(profileGroups(true).find((g) => g.id === "galvo2")?.fields[0].path, "calibration.scale_y");
  // Paths are unique.
  assert.equal(new Set(allPaths()).size, allPaths().length);
});

test("current values from the summary, the .lbdev export and the controller info", () => {
  const v = profileValues(SUMMARY, LBDEV, INFO);
  assert.equal(v.field_mm, 153.75);
  assert.equal(v.frequency_khz_min, 1);
  assert.equal(v.frequency_khz_max, 4000);
  assert.equal(v["mark_speed.max_mm_s"], 10000);
  assert.equal(v["timing.jump_delay_min_us"], 8);
  assert.equal(v["calibration.offset_x_mm"], 0.2);
  assert.equal(v["calibration.angle_deg"], 0.5);
  assert.equal(v["calibration.scale_x"], 0.99, "galvo 2 is X");
  assert.equal(v["calibration.scale_y"], 1.01);
  assert.equal(v["calibration.bulge_x"], 1.002);
  assert.equal(v.open_mo_delay_us, 8000);
  assert.equal(v.start_mark_input_bit, 14);
  assert.equal(v.red_light_speed_mm_s, 3000);
  assert.equal(v.laser_kind, "fiber");
  assert.equal(v.fiber_kind, "mopa");
  assert.equal(v["axis.enabled"], false);
  assert.equal(v.red_light_port_bit, 8);
  // Not reported anywhere: unknown, not a made-up default.
  assert.equal(v["red_dot.offset_x_mm"], undefined);
  assert.equal(v.focus_light_port_bit, undefined);
  assert.equal(v.blanking, undefined);

  // A full profile (a newer engine) is read as is, and wins over the .lbdev.
  const full = profileValues({ ...SUMMARY, calibration: { scale_x: 1.5 }, red_dot: { offset_x_mm: 0.3 }, axis: { role: "z", steps_per_mm: 200, min_mm: 0, max_mm: 40, invert: false } }, LBDEV, null);
  assert.equal(full["calibration.scale_x"], 1.5);
  assert.equal(full["red_dot.offset_x_mm"], 0.3);
  assert.equal(full["axis.enabled"], true);
  assert.equal(full["axis.steps_per_mm"], 200);

  // No start bit in the .lbdev (-1) is "none".
  assert.equal(profileValues(null, JSON.stringify({ DeviceList: [{ Settings: { StartMarkPortBit: -1 } }] }), null).start_mark_input_bit, null);
  assert.deepEqual(Object.keys(profileValues(null, "not json", null)).sort(), ["laser_enable_port_bit", "red_light_port_bit"]);
});

test("the patch carries only edited fields", () => {
  const v = profileValues(SUMMARY, LBDEV, INFO);
  assert.equal(profilePatch(v, {}), null);
  assert.equal(profilePatch(v, { field_mm: 153.75 }), null, "unchanged values are not sent");
  assert.deepEqual(profilePatch(v, { "calibration.scale_x": 1.02, "timing.laser_on_delay_us": -200, max_power_pct: 80 }), {
    calibration: { scale_x: 1.02 },
    timing: { laser_on_delay_us: -200 },
    max_power_pct: 80,
  });
  // A cleared bit is null (no input); a never-reported value is sent once it is typed.
  assert.deepEqual(profilePatch(v, { start_mark_input_bit: null, focus_light_port_bit: 9, "red_dot.rest_x_mm": 5 }), {
    start_mark_input_bit: null,
    focus_light_port_bit: 9,
    red_dot: { rest_x_mm: 5 },
  });
  // The fixed port bits are never sent.
  assert.equal(profilePatch(v, { red_light_port_bit: 9 }), null);
  // The axis goes whole, or null.
  assert.deepEqual(profilePatch(v, { "axis.enabled": true, "axis.steps_per_mm": 400 }), { axis: { role: "z", steps_per_mm: 400, min_mm: 0, max_mm: 50, invert: false } });
  const withAxis = { ...v, "axis.enabled": true, "axis.role": "rotary", "axis.steps_per_mm": 0, "axis.min_mm": 0, "axis.max_mm": 0, "axis.invert": false };
  assert.deepEqual(profilePatch(withAxis, { "axis.enabled": false }), { axis: null });
});

test("client checks", () => {
  const v = profileValues(SUMMARY, LBDEV, INFO);
  assert.deepEqual(checkValues(v), {});
  const bad = merged(v, { frequency_khz_min: 5000, "calibration.angle_deg": 60, "calibration.scale_y": 3, focus_light_port_bit: 8, stop_mark_input_bit: 14, max_power_pct: 120 });
  const e = checkValues(bad);
  assert.ok(e.frequency_khz_max && e["calibration.angle_deg"] && e["calibration.scale_y"] && e.focus_light_port_bit && e.stop_mark_input_bit && e.max_power_pct);
  // Limits only go down.
  const lim = checkValues(merged(v, { max_power_pct: 90, max_jump_speed_mm_s: 5000, frequency_khz_min: 0.5 }), { max_power_pct: 100, max_jump_speed_mm_s: 4000, frequency_khz_min: 1 });
  assert.equal(lim.max_power_pct, undefined);
  assert.ok(lim.max_jump_speed_mm_s?.includes("never raised"));
  assert.ok(lim.frequency_khz_min?.includes("never widened"));
  const jump = checkValues(merged(v, { "timing.jump_speed_mm_s": 4500 }));
  assert.ok(jump["timing.jump_speed_mm_s"]);
  const axis = checkValues(merged(v, { "axis.enabled": true, "axis.role": "z", "axis.steps_per_mm": 2000, "axis.min_mm": 0, "axis.max_mm": 50 }));
  assert.ok(axis["axis.max_mm"]?.includes("65535"));
});

test("engine refusals point at the field they name", () => {
  assert.equal(errorField("max_power_pct: 60 is above the limit of 50 (limits are never raised)"), "max_power_pct");
  assert.equal(errorField("mark_speed.max_mm_s: 20000 is above the limit of 10000 (limits are never raised)"), "mark_speed.max_mm_s");
  assert.equal(errorField("invalid profile: calibration: offset_y_mm = 30 mm is beyond ±7.6 mm"), "calibration.offset_y_mm");
  assert.equal(errorField("focus_light_port_bit: 8 must be an output bit 1-15 other than the laser-enable and red-pointer bits"), "focus_light_port_bit");
  assert.equal(errorField("frequency_khz_min: 0.5 is below the limit of 1 (limits are never widened)"), "frequency_khz_min");
  assert.equal(errorField("settings: unknown field `bogus`"), null);
});
