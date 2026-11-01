// Unit tests for api/history.ts (timeline normalisation, marker slots) and api/laser.ts (pulse widths, frequency
// range, energy shading, out-of-field objects, friendly errors, stats rows, per-layer preview).

import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument } from "./types.ts";
import {
  afterMarkerNote,
  discardedByNextEdit,
  isApplied,
  markerForSlot,
  markerSlot,
  nearestSlot,
  normalizeHistory,
  shortCommand,
} from "./history.ts";
import {
  DEFAULT_PULSE_WIDTHS_NS,
  energyOpacity,
  formatDuration,
  frequencyRange,
  friendlyError,
  humanKey,
  layerEnergy,
  nearestPulseWidth,
  outsideField,
  previewLayers,
  pulseWidths,
  statRows,
} from "./laser.ts";

test("short command labels", () => {
  assert.equal(shortCommand("object.transform"), "transform");
  assert.equal(shortCommand("object.transform, layer.update"), "transform +1");
});

test("an older engine's log is all active with the marker at the end", () => {
  const m = normalizeHistory([
    { revision: 1, command: "shape.rect", origin: "ui", summary: "a" },
    { revision: 2, command: "object.transform", origin: { kind: "agent", name: "c" }, summary: "b" },
  ]);
  assert.equal(m.editable, false);
  assert.equal(m.steps.length, 2);
  assert.equal(m.marker, 1);
  assert.equal(markerSlot(m), 2);
  assert.equal(m.steps[1].label, "transform");
});

test("timeline steps keep their state and the marker", () => {
  const m = normalizeHistory({
    marker: 2,
    steps: [
      { index: 3, invocations: [{ id: "layer.update" }], origin: "ui", summary: "c", state: "rolled_back" },
      { index: 1, invocations: [{ id: "shape.rect" }], origin: "ui", summary: "a", state: "active" },
      { index: 2, invocations: [{ id: "object.transform" }, { id: "object.rename" }], origin: "ui", summary: "b", state: "failed", error: "gone" },
    ],
  });
  assert.equal(m.editable, true);
  assert.deepEqual(m.steps.map((s) => s.index), [1, 2, 3]);
  assert.equal(m.start, 0);
  assert.equal(m.steps[1].label, "transform +1");
  assert.equal(m.steps[1].error, "gone");
  assert.equal(markerSlot(m), 2);
  assert.ok(isApplied(m, m.steps[0]));
  assert.ok(!isApplied(m, m.steps[2]));
  assert.equal(markerForSlot(m, 0), 0, "slot 0 is before the first step");
  assert.equal(markerForSlot(m, 1), 1);
  assert.equal(markerForSlot(m, 9), 3);
  assert.deepEqual(normalizeHistory(null).steps, []);
  assert.deepEqual(normalizeHistory({ steps: "x" }).steps, []);
});

test("nearest slot to a drag position", () => {
  const lefts = [0, 100, 200];
  const rights = [90, 190, 290];
  assert.equal(nearestSlot(rights, lefts, -20), 0);
  assert.equal(nearestSlot(rights, lefts, 80), 1);
  assert.equal(nearestSlot(rights, lefts, 196), 2);
  assert.equal(nearestSlot(rights, lefts, 1000), 3);
});

test("pulse widths and frequency come from the engine when it reports them", () => {
  assert.deepEqual(pulseWidths(null), DEFAULT_PULSE_WIDTHS_NS);
  assert.deepEqual(pulseWidths({ connected: true, pulse_widths_ns: [9, 2, 2, "x"] }), [2, 9]);
  assert.deepEqual(frequencyRange(null), { min: 1, max: 4000 });
  assert.deepEqual(frequencyRange({ connected: true, frequency_khz_min: 20, frequency_khz_max: 2000 }), { min: 20, max: 2000 });
  assert.equal(nearestPulseWidth(10, DEFAULT_PULSE_WIDTHS_NS), 9);
  assert.equal(nearestPulseWidth(9999, DEFAULT_PULSE_WIDTHS_NS), 500);
});

test("energy shading: more power, more passes or slower is darker", () => {
  const s = (power_pct: number, speed_mm_s: number, passes = 1) => ({ power_pct, speed_mm_s, passes }) as never;
  assert.equal(layerEnergy(s(50, 100)), 0.5);
  assert.equal(layerEnergy(s(50, 100, 4)), 2);
  assert.equal(layerEnergy(s(50, 0)), 0);
  assert.equal(energyOpacity(2, 2), 1);
  assert.equal(energyOpacity(0, 2), 0.25);
  assert.equal(energyOpacity(1, 0), 1);
});

const doc: GDocument = {
  revision: 1,
  name: "d",
  stock: null,
  layers: [
    { id: "layer-1", name: "Cut", color: "#f00", visible: true, output: true, settings: { mode: "line", power_pct: 50, speed_mm_s: 100, passes: 1 } as never, sub_layers: [] },
    { id: "layer-2", name: "Fill", color: "#00f", visible: true, output: true, settings: { mode: "fill", power_pct: 80, speed_mm_s: 100, passes: 2 } as never, sub_layers: [] },
  ],
  objects: [
    { id: "obj-1", name: "Inside", layer: "layer-1", geometry: { kind: "rect", width_mm: 10, height_mm: 10, corner_radius_mm: 0 }, transform: [1, 0, 0, 1, 0, 0] },
    { id: "obj-5", name: "Ellipse [0,2]", layer: "layer-1", geometry: { kind: "ellipse", rx_mm: 5, ry_mm: 5 }, transform: [1, 0, 0, 1, 74, 0] },
  ],
  next_id: 6,
};

test("objects reaching the field edge are flagged", () => {
  assert.deepEqual([...outsideField(doc, 153.75)], ["obj-5"]);
  assert.equal(outsideField(doc, 200).size, 0);
  assert.equal(outsideField(null, 100).size, 0);
});

test("engine errors lose the HTTP prefix and name objects", () => {
  assert.equal(
    friendlyError("POST /api/runs: 422 objects obj-5 fall outside the ±76.875 mm field", doc),
    'objects "Ellipse [0,2]" fall outside the ±76.875 mm field',
  );
  assert.equal(friendlyError("layer-2 has no objects; obj-77 unknown", doc), 'layer "Fill" has no objects; obj-77 unknown');
  // The engine's lock refusal names the id once and bare, so it reads with plain quotes.
  assert.equal(
    friendlyError("object.transform: object obj-5 is locked; unlock it first (object.lock with locked: false)", doc),
    'object.transform: object "Ellipse [0,2]" is locked; unlock it first (object.lock with locked: false)',
  );
});

test("stats rows have labels and units", () => {
  assert.deepEqual(statRows({ mark_length_mm: 255.844, estimated_time_s: 0.343, jump_length_mm: 3, odd_thing_khz: 2, nested: {} }, ["jump_length_mm"]), [
    ["Mark length", "255.8 mm"],
    ["Estimated time", "0.3 s"],
    ["Odd thing (kHz)", "2"],
  ]);
  assert.deepEqual(statRows({ mark_length_mm: 254 }, [], "in"), [["Mark length", "10.00 in"]]);
  assert.equal(humanKey("estimated_time_s"), "Estimated time (s)");
  assert.equal(formatDuration(42.4), "42 s");
  assert.equal(formatDuration(185), "3 min 05 s");
  assert.equal(formatDuration(3720), "1 h 02 min");
});

test("per-layer preview rows: from the engine or counted, in layer order", () => {
  const segs = [
    { kind: "mark" as const, layer: "layer-2", from: [0, 0] as [number, number], to: [3, 4] as [number, number] },
    { kind: "jump" as const, layer: null, from: [0, 0] as [number, number], to: [1, 0] as [number, number] },
    { kind: "mark" as const, layer: "layer-1", from: [0, 0] as [number, number], to: [0, 2] as [number, number] },
  ];
  const counted = previewLayers({ segments: segs }, doc);
  assert.deepEqual(counted.map((r) => [r.id, r.segments, r.markLengthMm, r.mode]), [
    ["layer-1", 1, 2, "line"],
    ["layer-2", 1, 5, "fill"],
  ]);
  assert.equal(counted[1].energy, 1.6);
  const engine = previewLayers({ segments: segs, layers: [{ layer_id: "layer-2", segments: 10, mark_length_mm: 99, estimated_time_s: 2 }] }, doc);
  assert.equal(engine.find((r) => r.id === "layer-2")?.segments, 10);
  assert.equal(engine.find((r) => r.id === "layer-2")?.estimatedTimeS, 2);
  assert.deepEqual(previewLayers(null, doc), []);
});

test("steps undone with Undo are shown as discarded by the next edit; rolled-back ones as kept", () => {
  const step = (index: number, state: string) => ({ index, invocations: [{ id: "object.transform" }], origin: "ui", summary: "s", state });
  const steps = [step(1, "active"), step(2, "rolled_back"), step(3, "rolled_back"), step(4, "rolled_back")];
  // Moved with the timeline (goto): kept, a new edit goes in before them (Fusion).
  const rolled = normalizeHistory({ marker: 1, marker_moved_by: "timeline", undone_steps: 0, steps });
  assert.equal(rolled.discardThrough, 1);
  assert.ok(rolled.steps.every((s) => !discardedByNextEdit(rolled, s)));
  assert.match(afterMarkerNote(rolled, rolled.steps[1]), /kept/);

  // Moved with Ctrl+Z (verify finding 1: an edit after Undo kept the undone steps, and Redo brought them back).
  const undone = normalizeHistory({ marker: 1, marker_moved_by: "undo", undone_steps: 3, steps });
  assert.equal(undone.discardThrough, 4);
  assert.ok(!discardedByNextEdit(undone, undone.steps[0]), "applied steps stay");
  assert.ok(undone.steps.slice(1).every((s) => discardedByNextEdit(undone, s)));
  assert.match(afterMarkerNote(undone, undone.steps[1]), /next edit discards it/);
  assert.ok(!isApplied(undone, undone.steps[1]));

  // Rolled back on the timeline to 3, then Ctrl+Z twice: only the two undone steps go; step 4 is kept.
  const mixed = normalizeHistory({ marker: 1, marker_moved_by: "undo", undone_steps: 2, steps });
  assert.deepEqual(
    mixed.steps.map((s) => discardedByNextEdit(mixed, s)),
    [false, true, true, false],
  );
  assert.match(afterMarkerNote(mixed, mixed.steps[3]), /kept/);

  // A count left over from another kind of move, a bad value, an engine without the fields, or an older engine's
  // append-only log never claims a discard.
  assert.equal(normalizeHistory({ marker: 1, marker_moved_by: "timeline", undone_steps: 3, steps }).discardThrough, 1);
  assert.equal(normalizeHistory({ marker: 1, marker_moved_by: "undo", undone_steps: "3", steps }).discardThrough, 1);
  assert.equal(normalizeHistory({ marker: 1, steps }).discardThrough, 1);
  const log = normalizeHistory([{ revision: 1, command: "shape.rect", origin: "ui", summary: "a" }]);
  assert.ok(log.steps.every((s) => !discardedByNextEdit(log, s)));
});
