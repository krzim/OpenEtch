import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument } from "../api/types.ts";
import { DEFAULT_TRACE } from "../files/fileFormats.ts";
import {
  DEFAULT_SLOTS,
  addTab,
  dogboneInvocation,
  dogboneProblem,
  groupUserTools,
  importTracedSvgs,
  isWebUrl,
  moveTab,
  nearestTab,
  parseSteps,
  parseTraceFiles,
  parseUserTools,
  removeTab,
  slotsInvocation,
  slotsProblem,
  tabHitPx,
  tabMarks,
  traceFilesInvocation,
  traceManyInvocation,
  userToolInvocations,
  userToolProblem,
  userToolUrl,
  userToolsBody,
  type UserTool,
} from "./toolsApi.ts";

const doc = {
  objects: [
    { id: "a", name: "A", layer: "l", geometry: { type: "rect" }, transform: [1, 0, 0, 1, 10, 0] },
    { id: "b", name: "B", layer: "l", geometry: { type: "rect" }, transform: [0, 1, -1, 0, 0, 0] },
    { id: "c", name: "C", layer: "l", geometry: { type: "rect" }, transform: [1, 0, 0, 1, 0, 0] },
  ],
  object_options: { a: { tabs: [[0, -5], [5, 0]] }, b: { tabs: [[1, 0], ["x", 1]] }, c: { power_scale_pct: 50 } },
} as unknown as GDocument;

test("hand-placed tabs are shown in the field frame and found near the pointer", () => {
  const marks = tabMarks(doc);
  assert.deepEqual(marks, [
    { id: "a", index: 0, at: [10, -5] },
    { id: "a", index: 1, at: [15, 0] },
    { id: "b", index: 0, at: [0, 1] },
  ]);
  assert.equal(tabMarks(null).length, 0);
  assert.deepEqual(nearestTab(marks, [14.6, 0.2], 1), marks[1]);
  assert.equal(nearestTab(marks, [30, 30], 1), null);
  assert.deepEqual(addTab("a", [1.234567, 2], 3), { id: "tabs.add", params: { id: "a", points: [[1.2346, 2]], max_distance_mm: 3 } });
  assert.deepEqual(moveTab(marks[0], [9, -5], "a"), { id: "tabs.move", params: { id: "a", index: 0, to: [9, -5] } });
  assert.deepEqual(moveTab(marks[0], [9, -5], "c").params.to_id, "c", "dragged onto another shape");
  assert.deepEqual(removeTab(marks[2]).params, { ids: ["b"], near: [0, 1], within_mm: 0.001 });
});

test("dogbone and resize slots parameters", () => {
  assert.equal(dogboneProblem(null), "dog.badDiameter");
  assert.equal(dogboneProblem(0), "dog.badDiameter");
  assert.equal(dogboneProblem(3.175), null);
  assert.deepEqual(dogboneInvocation(["a"], 3.175, "dogbone").params, { ids: ["a"], diameter_mm: 3.175 });
  assert.equal(dogboneInvocation(["a"], 1, "tbone_longer").params.style, "tbone_longer");
  assert.equal(slotsProblem(DEFAULT_SLOTS), null);
  assert.equal(slotsProblem({ ...DEFAULT_SLOTS, toleranceMm: 3 }), "slots.badThickness");
  assert.equal(slotsProblem({ ...DEFAULT_SLOTS, newMm: null }), "slots.badThickness");
  assert.equal(slotsProblem({ ...DEFAULT_SLOTS, depth: false }), "slots.none");
  assert.deepEqual(slotsInvocation(["a"], { ...DEFAULT_SLOTS, newMm: 4, tabHeight: true }).params, {
    ids: ["a"],
    old_thickness_mm: 3,
    new_thickness_mm: 4,
    tolerance_mm: 0.1,
    adjust_slot_depth: true,
    adjust_slot_width: false,
    adjust_tab_height: true,
  });
});

test("multi-file trace sends one set of settings", () => {
  const o = { ...DEFAULT_TRACE, threshold: 100, centerline: true, deleteImage: true, boundary: [0, 0, 1, 1] as [number, number, number, number] };
  assert.deepEqual(traceManyInvocation(["a", "b"], o, true, null).params, { ids: ["a", "b"], settings: { threshold: 100, centerline: true }, delete_images: true });
  assert.deepEqual(traceFilesInvocation([{ name: "x.png", data_base64: "AA==" }], DEFAULT_TRACE, 300, true).params, {
    files: [{ name: "x.png", data_base64: "AA==" }],
    settings: {},
    dpi: 300,
    zip: true,
  });
  const r = parseTraceFiles({ files: [{ name: "x.png", svg_name: "x.svg", svg: "<svg/>" }, { name: "y.png", error: "bad" }], traced: 1, zip_base64: "UEs=", zip_name: "traced.zip" });
  assert.equal(r.zip, "UEs=");
  assert.deepEqual(r.files[1], { name: "y.png", svgName: null, svg: null, error: "bad" });
  assert.deepEqual(importTracedSvgs(r.files, "layer-2"), [{ id: "art.import_svg", params: { svg: "<svg/>", name: "x", layer: "layer-2" } }]);
});

test("user tools: parsing, checks, the menu and what they run", () => {
  const raw = {
    user_tools: [
      { name: "Box maker", category: "Generators", kind: "url", url: "https://example.com/box" },
      { name: "Tag", kind: "commands", steps: [{ id: "shape.rect", params: { width_mm: 60 } }, { id: "doc.inspect" }] },
      { name: "Bad", kind: "run_job" },
      { name: "Puzzle", category: "Generators", kind: "url", url: "http://example.org" },
    ],
  };
  const tools = parseUserTools(raw);
  assert.equal(tools.length, 3);
  assert.deepEqual(tools[1], { name: "Tag", category: "", kind: "commands", steps: [{ id: "shape.rect", params: { width_mm: 60 } }, { id: "doc.inspect", params: {} }] });
  assert.deepEqual(
    groupUserTools(tools).map((g) => [g.category, g.tools.map((t) => t.name)]),
    [
      ["", ["Tag"]],
      ["Generators", ["Box maker", "Puzzle"]],
    ],
  );
  assert.deepEqual(userToolsBody(tools)[1], { name: "Tag", kind: "commands", steps: [{ id: "shape.rect", params: { width_mm: 60 } }, { id: "doc.inspect", params: {} }] });
  assert.deepEqual(userToolsBody(tools)[0], { name: "Box maker", category: "Generators", kind: "url", url: "https://example.com/box" });
  assert.deepEqual(userToolInvocations(tools[1]), [{ id: "shape.rect", params: { width_mm: 60 } }, { id: "doc.inspect", params: {} }]);
  assert.deepEqual(userToolInvocations(tools[0]), []);
  for (const u of ["javascript:alert(1)", "data:text/html,x", "https://", "ftp://x", "https://a b", ""]) assert.equal(isWebUrl(u), false, u);
  assert.equal(isWebUrl("HTTPS://example.com/x?y=1"), true);
  const url = (u: string): UserTool => ({ name: "X", category: "", kind: "url", url: u });
  assert.equal(userToolProblem(url("https://a")), null);
  assert.match(userToolProblem(url("javascript:x")) ?? "", /http/);
  assert.match(userToolProblem({ ...url("https://a"), name: " " }) ?? "", /name/);
  assert.deepEqual(parseSteps('[{"id": "shape.rect", "params": {"width_mm": 5}}]'), { steps: [{ id: "shape.rect", params: { width_mm: 5 } }] });
  for (const bad of ["", "{}", "[]", '[{"params": {}}]', '[{"id": "x", "params": [1]}]']) assert.ok("error" in parseSteps(bad), bad);
  assert.equal(parseUserTools(null).length, 0);
});

test("the menu opens only http(s) pages, whatever preferences.json holds", () => {
  const tools = parseUserTools({
    user_tools: [
      { name: "Ok", kind: "url", url: " https://example.com/box " },
      { name: "Script", kind: "url", url: "javascript:alert(1)" },
      { name: "Data", kind: "url", url: "data:text/html,hi" },
      { name: "Seq", kind: "commands", steps: [{ id: "shape.rect" }] },
    ],
  });
  assert.deepEqual(tools.map(userToolUrl), ["https://example.com/box", null, null, null]);
});

test("a tab mark is picked within 8 px of the mouse and a finger's reach on touch", () => {
  assert.equal(tabHitPx("mouse"), 8);
  assert.equal(tabHitPx(undefined), 8);
  assert.equal(tabHitPx("touch"), 18);
  assert.equal(tabHitPx("pen"), 10);
});
