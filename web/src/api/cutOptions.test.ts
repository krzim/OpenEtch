// Unit tests for api/cutOptions.ts: cut options, timing override, optimization settings, rotary and shape properties
// as the engine stores them, and the command builders.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  DEFAULT_OPTIMIZATION,
  DEFAULT_ROTARY,
  activeOptionCount,
  circumference,
  deviceTiming,
  globalPasses,
  globalPassesUpdate,
  intervalToLpi,
  layerTiming,
  lpiPatch,
  moveOrder,
  objectOptions,
  optimizationOf,
  optimizationPatch,
  optionsPatch,
  resolveOptions,
  rotaryOf,
  rotaryProblems,
  setCutProperties,
  setOptimization,
  setRotary,
  sharedOptions,
  stepsPerObjectTurn,
  stripCount,
  timingPatch,
} from "./cutOptions.ts";
import type { CutSettings, GDocument, Layer } from "./types.ts";

const settings = (extra: Record<string, unknown> = {}): CutSettings =>
  ({
    mode: "fill",
    power_pct: 20,
    speed_mm_s: 1000,
    frequency_khz: 30,
    pulse_width_ns: null,
    passes: 1,
    hatch: { interval_mm: 0.05, angle_deg: 0, angle_increment_deg: 0, cross_hatch: false, bidirectional: true },
    timing: null,
    ...extra,
  }) as CutSettings;

test("cut options resolve with the engine's defaults", () => {
  const r = resolveOptions(settings({ options: { fill_grouping: "all_at_once" } }));
  assert.equal(r.output, true);
  assert.equal(r.fill_grouping, "all_at_once");
  assert.equal(r.dot_mode, null);
  assert.equal(activeOptionCount(r), 0, "a new layer's options");
  // Options saved without a fill grouping fill shapes individually, which differs from a new layer.
  const old = resolveOptions(settings({ options: {} }));
  assert.equal(old.fill_grouping, "individually");
  assert.equal(activeOptionCount(old), 1);
  const s = resolveOptions(settings({ options: { output: false, pause_after_ms: 20, wobble: { step_mm: 0.2, size_mm: 0.5 }, tabs: { size_mm: 1, count: 3 }, fill_grouping: "by_group" } }));
  assert.equal(s.output, false);
  assert.deepEqual(s.wobble, { step_mm: 0.2, size_mm: 0.5 });
  assert.deepEqual(s.tabs, { count: 3, spacing_mm: 0, size_mm: 1 });
  assert.equal(activeOptionCount(s), 5);
  // The GRBL layer options: read like the others, counted only for a gantry (a galvo never uses them).
  const g = resolveOptions(settings({ options: { constant_power: true, air_assist: true, overscan_mm: 2.5, overcut_mm: 1 } }));
  assert.deepEqual([g.constant_power, g.air_assist, g.overscan_mm, g.overcut_mm], [true, true, 2.5, 1]);
  assert.equal(activeOptionCount(g), 1, "only the missing fill grouping, on a galvo");
  assert.equal(activeOptionCount(g, true), 5);
  assert.deepEqual([r.constant_power, r.air_assist, r.overscan_mm, r.overcut_mm], [false, false, 0, 0]);
  assert.deepEqual(optionsPatch({ layer: "layer-1", sub: 0 }, { air_assist: true }), {
    id: "layer.sub_update",
    params: { layer: "layer-1", index: 0, settings: { options: { air_assist: true } } },
  });
  // Settings from an engine without options.
  assert.equal(resolveOptions(settings()).output, true);
});

test("option, LPI, timing and global-passes commands", () => {
  assert.deepEqual(optionsPatch({ layer: "layer-1", sub: null }, { wobble: null }), { id: "layer.update", params: { id: "layer-1", settings: { options: { wobble: null } } } });
  assert.deepEqual(optionsPatch({ layer: "layer-1", sub: 2 }, { output: false }), { id: "layer.sub_update", params: { layer: "layer-1", index: 2, settings: { options: { output: false } } } });
  assert.deepEqual(lpiPatch({ layer: "l", sub: null }, 254), { id: "layer.update", params: { id: "l", settings: { hatch: { lines_per_inch: 254 } } } });
  assert.ok(Math.abs(intervalToLpi(0.1) - 254) < 1e-9);
  assert.equal(intervalToLpi(0), 0);
  assert.deepEqual(globalPassesUpdate("l", 3.4), { id: "layer.update", params: { id: "l", global_passes: 3 } });
  assert.equal(globalPasses({ global_passes: 4 } as unknown as Layer), 4);
  assert.equal(globalPasses({} as Layer), 1);
  assert.deepEqual(timingPatch({ layer: "l", sub: null }, null), { id: "layer.update", params: { id: "l", settings: { timing: null } } });
});

test("the device's timing becomes a layer override with the layer's key names", () => {
  const t = deviceTiming({ laser_on_delay_us: -200, jump_delay_min_us: 9, jump_delay_max_us: 250, jump_speed_mm_s: 3000 });
  assert.equal(t.laser_on_delay_us, -200);
  assert.equal(t.min_jump_delay_us, 9);
  assert.equal(t.max_jump_delay_us, 250);
  assert.equal(t.jump_speed_mm_s, 3000);
  assert.equal(t.end_delay_us, 300, "missing values come from the fallback");
  assert.equal(layerTiming(settings()), null);
  assert.equal(layerTiming(settings({ timing: { ...t } }))?.max_jump_delay_us, 250);
});

test("optimization settings: defaults, patch of the changed keys, order list", () => {
  assert.deepEqual(optimizationOf(null), DEFAULT_OPTIMIZATION);
  const doc = { optimization: { inner_first: true, order_by: ["priority", "layer", "bogus"], direction_order: "left_to_right" } } as unknown as GDocument;
  const o = optimizationOf(doc);
  assert.equal(o.inner_first, true);
  assert.deepEqual(o.order_by, ["priority", "layer"]);
  assert.equal(o.direction_order, "left_to_right");
  assert.equal(o.best_start, true);
  assert.equal(optimizationPatch(o, o), null);
  assert.deepEqual(optimizationPatch(o, { ...o, order_by: ["layer", "priority"], cuts_last: true }), { order_by: ["layer", "priority"], cuts_last: true });
  assert.deepEqual(setOptimization({ enabled: false }), { id: "job.set_optimization", params: { optimization: { enabled: false } } });
  assert.deepEqual(moveOrder(["layer", "group", "priority"], 2, -1), ["layer", "priority", "group"]);
  assert.deepEqual(moveOrder(["layer"], 0, 1), ["layer"]);
});

test("rotary setup: defaults, checks, derived values", () => {
  assert.deepEqual(rotaryOf(null), DEFAULT_ROTARY);
  const r = rotaryOf({ rotary: { enabled: true, kind: "roller", object_diameter_mm: 70, taper: { end_diameter_mm: 60, length_mm: 100 } } } as unknown as GDocument);
  assert.equal(r.kind, "roller");
  assert.equal(r.steps_per_rotation, 3200);
  assert.deepEqual(r.taper, { end_diameter_mm: 60, length_mm: 100 });
  assert.ok(Math.abs(circumference(r) - Math.PI * 70) < 1e-9);
  assert.ok(Math.abs(stepsPerObjectTurn(r) - (3200 * 70) / 30) < 1e-9);
  assert.equal(stepsPerObjectTurn(DEFAULT_ROTARY), 3200);
  assert.equal(stripCount(DEFAULT_ROTARY, 25), 3);
  assert.equal(stripCount(DEFAULT_ROTARY, 20), 2);
  assert.deepEqual(rotaryProblems(DEFAULT_ROTARY), {});
  assert.ok(rotaryProblems({ ...DEFAULT_ROTARY, split_width_mm: 500 }).split_width_mm);
  assert.ok(rotaryProblems({ ...DEFAULT_ROTARY, steps_per_rotation: 0 }).steps_per_rotation);
  assert.ok((rotaryProblems({ ...DEFAULT_ROTARY, taper: { end_diameter_mm: -1, length_mm: 10 } }) as Record<string, string>)["taper.end_diameter_mm"]);
  assert.deepEqual(setRotary(null), { id: "job.set_rotary", params: { rotary: null } });
});

test("shape cut properties", () => {
  const doc = { object_options: { "obj-1": { priority: 2, power_scale_pct: 50 }, "obj-2": { power_scale_pct: 50 } } } as unknown as GDocument;
  assert.deepEqual(objectOptions(doc, "obj-1"), { priority: 2, power_scale_pct: 50 });
  assert.deepEqual(objectOptions(doc, "obj-9"), { priority: null, power_scale_pct: 100 });
  assert.deepEqual(sharedOptions(doc, ["obj-1", "obj-2"]), { priority: "mixed", power_scale_pct: 50 });
  assert.deepEqual(setCutProperties(["obj-1"], { power_scale_pct: 140 }), { id: "object.set_cut_properties", params: { ids: ["obj-1"], power_scale_pct: 100 } });
  assert.deepEqual(setCutProperties(["obj-1"], { priority: null }), { id: "object.set_cut_properties", params: { ids: ["obj-1"], priority: null } });
});
