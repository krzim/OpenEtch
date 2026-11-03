import assert from "node:assert/strict";
import { test } from "node:test";
import { sourceFields } from "./laser.ts";
import { layerHasImages, overscanApplies } from "./cutOptions.ts";
import type { GDocument } from "./types.ts";

test("cut settings show the source settings of the active device type, as LightBurn does", () => {
  // A galvo MOPA: Frequency and Q-Pulse.
  assert.deepEqual(sourceFields({ connected: true, driver: "lmc", pulse_width_enabled: true }), { frequency: true, pulseWidth: true });
  // A Q-switched galvo (or Q-pulse off): no pulse width.
  assert.deepEqual(sourceFields({ connected: true, driver: "lmc", pulse_width_enabled: false }), { frequency: true, pulseWidth: false });
  // GRBL and Ruida gantries: neither.
  assert.deepEqual(sourceFields({ connected: true, driver: "grbl", pulse_width_enabled: false }), { frequency: false, pulseWidth: false });
  assert.deepEqual(sourceFields({ connected: true, driver: "ruida" }), { frequency: false, pulseWidth: false });
  // An older engine that does not say, or no machine: everything.
  assert.deepEqual(sourceFields({ connected: true }), { frequency: true, pulseWidth: true });
  assert.deepEqual(sourceFields(null), { frequency: true, pulseWidth: true });
});

test("the overscan field shows where the planner overscans: fill hatches and images, not Offset Fill rings", () => {
  assert.equal(overscanApplies("fill", false), true);
  assert.equal(overscanApplies("fill_then_line", false), true);
  assert.equal(overscanApplies("offset_fill", false), false);
  assert.equal(overscanApplies("line", false), false);
  assert.equal(overscanApplies("line", true), true);
  assert.equal(overscanApplies("offset_fill", true), true);
  const doc = {
    layers: [],
    objects: [
      { id: "obj-1", name: "a", layer: "layer-1", geometry: { kind: "image", width_mm: 1, height_mm: 1, mime: "image/png", data_base64: "" }, transform: [1, 0, 0, 1, 0, 0] },
      { id: "obj-2", name: "b", layer: "layer-2", geometry: { kind: "rect", width_mm: 1, height_mm: 1, corner_radius_mm: 0 }, transform: [1, 0, 0, 1, 0, 0] },
    ],
  } as unknown as GDocument;
  assert.equal(layerHasImages(doc, "layer-1"), true);
  assert.equal(layerHasImages(doc, "layer-2"), false);
  assert.equal(layerHasImages(null, "layer-1"), false);
});
