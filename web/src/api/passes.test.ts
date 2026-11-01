// Unit tests for api/passes.ts (a layer's main settings plus sub-layers: the Properties pass list and the preview's
// per-pass rows) and the sub-layer command builders in api/commands.ts.

import assert from "node:assert/strict";
import { test } from "node:test";
import * as C from "./commands.ts";
import * as P from "./passes.ts";
import type { CutSettings, GDocument, Layer, Preview, PreviewSegment } from "./types.ts";

const cut = (o: Partial<CutSettings> = {}): CutSettings => ({
  mode: "fill",
  power_pct: 40,
  speed_mm_s: 1000,
  frequency_khz: 30,
  pulse_width_ns: 200,
  passes: 2,
  hatch: { interval_mm: 0.05, angle_deg: 0, angle_increment_deg: 10, cross_hatch: true, bidirectional: true },
  timing: null,
  ...o,
});

const layer = (id: string, subs: CutSettings[] = [], o: Partial<Layer> = {}): Layer => ({
  id,
  name: id,
  color: "#d22020",
  visible: true,
  output: true,
  settings: cut(),
  sub_layers: subs,
  ...o,
});

const docOf = (...layers: Layer[]): GDocument => ({ revision: 1, name: "t", stock: null, layers, objects: [], next_id: 1 });

const mark = (layer: string, pass: number, from: [number, number], to: [number, number], extra: Partial<PreviewSegment> = {}): PreviewSegment => ({
  kind: "mark",
  layer,
  pass,
  from,
  to,
  ...extra,
});

test("pass entries: Main first, then sub-layers with names, cleanup labels and defaults", () => {
  const l = layer("L", [cut({ name: "Outline", mode: "line" }), cut({ cleanup: { every_n_passes: 2 } }), cut()]);
  const e = P.passEntries(l);
  assert.deepEqual(
    e.map((x) => [x.pass, x.sub, x.label, x.cleanup]),
    [
      [0, null, "Main", null],
      [1, 0, "Outline", null],
      [2, 1, "Cleanup", { every_n_passes: 2 }],
      [3, 2, "Sub-layer 3", null],
    ],
  );
  assert.equal(P.passLabel(layer("M", [], { settings: cut({ name: "Engrave" }) }), 0), "Main · Engrave");
  assert.equal(P.passLabel(layer("N", [cut({ name: "  " })]), 1), "Sub-layer 1");
  assert.equal(P.passSettings(l, 1)?.mode, "line");
  assert.equal(P.passSettings(l, 9), undefined);
});

test("open pass stays valid when sub-layers disappear", () => {
  const l = layer("L", [cut(), cut()]);
  assert.equal(P.clampPass(l, 2), 2);
  assert.equal(P.clampPass(l, 5), 2);
  assert.equal(P.clampPass(l, -1), 0);
  assert.equal(P.clampPass(l, 1.5), 0);
  assert.equal(P.clampPass(layer("E"), 3), 0);
});

test("after a delete the next sub-layer opens, else the previous, else Main", () => {
  assert.equal(P.passAfterDelete(0, 3), 1); // the old second one, now first
  assert.equal(P.passAfterDelete(1, 3), 2);
  assert.equal(P.passAfterDelete(2, 3), 2); // the last: the previous one
  assert.equal(P.passAfterDelete(0, 1), 0); // none left: Main
});

test("after a move the open pass follows its sub-layer", () => {
  // Subs A B C D; open pass n is sub n-1.
  assert.equal(P.passAfterMove(2, 1, 2), 3); // B moved down: B stays open
  assert.equal(P.passAfterMove(3, 2, 1), 2); // C moved up
  assert.equal(P.passAfterMove(3, 0, 3), 2); // A moved past C: C shifts up
  assert.equal(P.passAfterMove(3, 3, 0), 4); // D moved in front of C: C shifts down
  assert.equal(P.passAfterMove(4, 0, 1), 4); // unrelated
  assert.equal(P.passAfterMove(0, 0, 1), 0); // Main never moves
});

test("add and duplicate open the command's result index", () => {
  assert.equal(P.passFromResult(2, 9), 3);
  assert.equal(P.passFromResult({ index: 0 }, 9), 1);
  assert.equal(P.passFromResult(null, 9), 9);
  assert.equal(P.passFromResult(-1, 9), 9);
  assert.equal(P.passFromResult("2", 9), 9);
});

test("cleanup pass: same shape, one fast low-energy pass at 45°, once after the main passes", () => {
  const c = P.cleanupPass(cut({ power_pct: 50, speed_mm_s: 800, frequency_khz: 40, passes: 3 }), 60);
  assert.equal(c.name, "Cleanup");
  assert.deepEqual(c.cleanup, { every_n_passes: 3 });
  assert.equal(c.settings.mode, undefined); // the command copies the mode and interval from the main settings
  assert.equal(c.settings.hatch?.interval_mm, undefined);
  assert.equal(c.settings.power_pct, 12.5);
  assert.equal(c.settings.speed_mm_s, 1600);
  assert.equal(c.settings.frequency_khz, 60); // doubled, capped at the source's maximum
  assert.equal(c.settings.passes, 1);
  assert.deepEqual(c.settings.hatch, { angle_deg: 45, angle_increment_deg: 0, cross_hatch: false });
  assert.equal(P.cleanupPass(cut({ hatch: { ...cut().hatch, angle_deg: 160 } })).settings.hatch?.angle_deg, -155);
  assert.equal(P.cleanupPass(cut({ passes: 0 })).cleanup.every_n_passes, 1);
});

test("settings summary and mode labels", () => {
  assert.equal(P.settingsSummary(cut()), "Fill · 40% · 1000 mm/s · 30 kHz · 200 ns · ×2");
  assert.equal(P.settingsSummary(cut({ mode: "line", pulse_width_ns: null, passes: 1 })), "Line · 40% · 1000 mm/s · 30 kHz");
  assert.equal(P.settingsSummary(cut({ pulse_width_ns: null, passes: 1 }), "mm/min"), "Fill · 40% · 60000 mm/min · 30 kHz");
  assert.equal(P.settingsSummary(cut({ pulse_width_ns: null, passes: 1 }), "in/s"), "Fill · 40% · 39.37 in/s · 30 kHz");
  assert.equal(P.modeLabel("fill_then_line"), "Fill+Line");
});

test("pass colours: Main keeps the layer colour, sub-layers differ from it and from each other", () => {
  assert.equal(P.passColor("#d22020", 0), "#d22020");
  const cs = [0, 1, 2, 3, 4, 5].map((n) => P.passColor("#d22020", n));
  assert.equal(new Set(cs).size, cs.length);
  for (const c of cs) assert.match(c, /^#[0-9a-f]{6}$/);
  assert.notEqual(P.passColor("#888", 1), "#888"); // a grey layer still gets distinct passes
  assert.equal(P.passColor("red", 1), "red"); // not hex: unchanged
});

test("resolver: segment sub_layer, then the preview's passes, then counting job passes", () => {
  const l = layer("L", [cut({ passes: 1 })], { settings: cut({ passes: 2 }) });
  const d = docOf(l, layer("K"));
  // Engine reports sub_layer on the segment.
  assert.equal(P.passResolver({ segments: [mark("L", 7, [0, 0], [1, 0], { sub_layer: 1 })] }, d)(mark("L", 7, [0, 0], [1, 0], { sub_layer: 1 })), 1);
  // Engine reports the job's passes.
  const withPasses: Preview = {
    segments: [mark("L", 0, [0, 0], [1, 0]), mark("L", 1, [0, 0], [1, 0])],
    passes: [
      { layer_id: "L", sub_layer: 1 },
      { layer_id: "L", sub_layer: 0 },
    ],
  };
  const r1 = P.passResolver(withPasses, d);
  assert.equal(r1(withPasses.segments[0]), 1);
  assert.equal(r1(withPasses.segments[1]), 0);
  // Older engine: three job passes for L = main x2 + sub x1; K has no sub-layers.
  const segs = [mark("L", 0, [0, 0], [1, 0]), mark("L", 0, [1, 0], [2, 0]), mark("L", 1, [0, 0], [1, 0]), mark("L", 2, [0, 0], [1, 0]), mark("K", 3, [0, 0], [1, 0])];
  const r2 = P.passResolver({ segments: segs }, d);
  assert.deepEqual(segs.map(r2), [0, 0, 0, 1, 0]);
  assert.equal(r2({ kind: "jump", from: [0, 0], to: [1, 1] }), null);
  // A skipped (empty) pass makes the counts disagree: nothing is guessed.
  const r3 = P.passResolver({ segments: [mark("L", 0, [0, 0], [1, 0]), mark("L", 1, [0, 0], [1, 0])] }, d);
  assert.equal(r3(mark("L", 1, [0, 0], [1, 0])), null);
  assert.deepEqual(P.expectedPasses(l), [0, 0, 1]);
});

test("expected run order: cleanup after every n main passes and the last, before the other sub-layers", () => {
  const l = layer("L", [cut({ passes: 1 }), cut({ passes: 2, cleanup: { every_n_passes: 2 } })], { settings: cut({ passes: 5 }) });
  assert.deepEqual(P.expectedPasses(l), [0, 0, 2, 2, 0, 0, 2, 2, 0, 2, 2, 1]);
  // n at least the main passes: once, at the end.
  const once = layer("O", [cut({ passes: 1, cleanup: { every_n_passes: 9 } })], { settings: cut({ passes: 3 }) });
  assert.deepEqual(P.expectedPasses(once), [0, 0, 0, 1]);
  // Counting old-engine job passes uses that order.
  const segs = P.expectedPasses(once).map((_, i) => mark("O", i, [0, 0], [1, 0]));
  assert.deepEqual(segs.map(P.passResolver({ segments: segs }, docOf(once))), [0, 0, 0, 1]);
});

test("per-pass preview rows: counts, colours, shared time estimate, engine stats", () => {
  const l = layer("L", [cut({ passes: 1, speed_mm_s: 500, cleanup: { every_n_passes: 1 } })], { settings: cut({ passes: 1, speed_mm_s: 1000 }) });
  const d = docOf(l, layer("K"));
  const preview: Preview = {
    segments: [
      { kind: "jump", layer: "L", pass: 0, from: [0, 0], to: [0, 0.5] },
      mark("L", 0, [0, 0], [10, 0]),
      mark("L", 1, [0, 0], [5, 0]),
      mark("K", 2, [0, 0], [3, 0]),
    ],
    layers: [{ layer_id: "L", estimated_time_s: 2 }],
  };
  const rows = P.previewPasses(preview, d, P.passResolver(preview, d));
  assert.deepEqual([...rows.keys()], ["L"]); // only layers with sub-layers get pass rows
  const [main, sub] = rows.get("L")!;
  assert.equal(main.key, "L#0");
  assert.equal(main.segments, 1);
  assert.equal(main.markLengthMm, 10);
  assert.equal(main.jumpLengthMm, 0.5);
  assert.equal(main.jobPasses, 1);
  assert.equal(main.color, "#d22020");
  assert.notEqual(sub.color, main.color);
  assert.equal(sub.label, "Cleanup");
  // Time shared by mark time: main 10/1000, sub 5/500, equal shares of 2 s.
  assert.ok(main.approx && sub.approx);
  assert.ok(Math.abs(main.estimatedTimeS! - 1) < 1e-9 && Math.abs(sub.estimatedTimeS! - 1) < 1e-9);

  const reported: Preview = {
    ...preview,
    passes: [
      { layer_id: "L", sub_layer: 0, segments: 1, mark_length_mm: 10, estimated_time_s: 0.25 },
      { layer_id: "L", sub_layer: 1, segments: 1, mark_length_mm: 5, estimated_time_s: 0.5 },
      { layer_id: "K", sub_layer: 0, segments: 1, mark_length_mm: 3, estimated_time_s: 0.1 },
    ],
  };
  const r = P.previewPasses(reported, d, P.passResolver(reported, d)).get("L")!;
  assert.equal(r[1].estimatedTimeS, 0.5);
  assert.equal(r[1].approx, false);
  assert.equal(r[1].jobPasses, 1);
  assert.equal(P.previewPasses(null, d, () => 0).size, 0);
});

test("pass visibility: hide, solo and un-solo", () => {
  const l = layer("L", [cut(), cut()]);
  const none: P.PassVisibility = { hidden: new Set(), hiddenPasses: new Set() };
  const t = P.togglePass(none, "L", 1);
  assert.ok(P.markHidden(t, "L", 1));
  assert.ok(!P.markHidden(t, "L", 0));
  assert.ok(!P.markHidden(t, "L", null));
  assert.ok(!P.markHidden(P.togglePass(t, "L", 1), "L", 1));
  assert.ok(P.markHidden({ hidden: new Set(["L"]), hiddenPasses: new Set() }, "L", 0));

  const solo = P.soloPass({ hidden: new Set(), hiddenPasses: new Set(["L#2", "K#0"]) }, ["L", "K"], l, 2);
  assert.deepEqual([...solo.hidden], ["K"]);
  assert.deepEqual([...solo.hiddenPasses].sort(), ["K#0", "L#0", "L#1"]);
  // Soloing it again shows everything.
  const back = P.soloPass(solo, ["L", "K"], l, 2);
  assert.equal(back.hidden.size + back.hiddenPasses.size, 0);
});

test("sub-layer command builders send only the given keys", () => {
  assert.deepEqual(C.layerSubAdd("layer-1"), { id: "layer.sub_add", params: { layer: "layer-1" } });
  assert.deepEqual(C.layerSubAdd("layer-1", { index: 0, name: undefined }), { id: "layer.sub_add", params: { layer: "layer-1", index: 0 } });
  const c = P.cleanupPass(cut());
  assert.deepEqual(C.layerSubAdd("layer-1", c).params, { layer: "layer-1", ...c });
  assert.deepEqual(C.layerSubUpdate("layer-1", 2, { settings: { power_pct: 5 } }), {
    id: "layer.sub_update",
    params: { layer: "layer-1", index: 2, settings: { power_pct: 5 } },
  });
  assert.deepEqual(C.layerSubUpdate("layer-1", 0, { name: null, cleanup: null }).params, { layer: "layer-1", index: 0, name: null, cleanup: null });
  assert.deepEqual(C.layerSubDelete("layer-1", 1), { id: "layer.sub_delete", params: { layer: "layer-1", index: 1 } });
  assert.deepEqual(C.layerSubMove("layer-1", 0, 2), { id: "layer.sub_move", params: { layer: "layer-1", from: 0, to: 2 } });
  assert.deepEqual(C.layerSubDuplicate("layer-1", 1), { id: "layer.sub_duplicate", params: { layer: "layer-1", index: 1 } });
});
