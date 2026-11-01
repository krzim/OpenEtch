import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument, Layer } from "../api/types.ts";
import type { NodeList } from "../nodes.ts";
import {
  approvalRefusalText,
  batchBody,
  checklistComplete,
  checklistFromText,
  checklistProblem,
  checklistTicks,
  cutEditProblem,
  editCutSettings,
  entryMetaProblem,
  filterTree,
  ioPorts,
  jobLogTotals,
  layerLink,
  libraryNameFromFile,
  libraryNameProblem,
  linkedLayers,
  nearestStartNode,
  originText,
  parseJobLog,
  parseLibraryView,
  parsePrefs,
  parseScan,
  parseUpdateCheck,
  printCutProblem,
  simPinActive,
  startMarks,
  tableBody,
  tableRole,
  treeEntries,
  updateText,
  wizardDeviceName,
} from "./model.ts";
import {
  DEFAULT_FOCUS_TEST,
  DEFAULT_INTERVAL_TEST,
  DEFAULT_MATERIAL_TEST,
  axisValues,
  focusHeights,
  focusTestParams,
  focusTestPreview,
  focusTestProblem,
  intervalTestParams,
  intervalTestProblem,
  intervalValues,
  materialTestFromParams,
  materialTestParams,
  materialTestPreview,
  materialTestProblem,
  previewViewBox,
  show,
  testPresetList,
  testPresetNameProblem,
} from "./testgen.ts";

const VIEW = {
  selected: "Fiber",
  libraries: [
    { name: "Fiber", entries: 3, materials: 2, dirty: true },
    { name: "Other", entries: 0, materials: 0, dirty: false },
  ],
  tree: {
    materials: [
      {
        name: "Steel",
        index: 0,
        groups: [
          {
            title: "1 mm",
            entries: [
              {
                id: "m0.e0",
                material: "Steel",
                thickness_mm: 1,
                title: "",
                group: "1 mm",
                description: "Black mark",
                path: "Steel/1 mm/Black mark",
                summary: { mode: "fill", power_pct: 40, speed_mm_s: 800, frequency_khz: 40, pulse_width_ns: 200, passes: 2, interval_mm: 0.02, sub_layers: 1 },
              },
            ],
          },
          { title: "Engrave", entries: [{ id: "m0.e1", material: "Steel", thickness_mm: null, title: "Engrave", group: "Engrave", description: "Deep", path: "Steel/Engrave/Deep", summary: null }] },
        ],
      },
      { name: "Wood", index: 1, groups: [{ title: "3 mm", entries: [{ id: "m1.e0", thickness_mm: 3, description: "Cut", path: "Wood/3 mm/Cut" }] }] },
    ],
    entries: 3,
  },
};

const layer = (id: string, extra: Partial<Layer> & { link?: unknown } = {}): Layer =>
  ({
    id,
    name: id,
    color: "#ff0000",
    visible: true,
    output: true,
    settings: { mode: "fill", power_pct: 1, speed_mm_s: 1, frequency_khz: 1, pulse_width_ns: null, passes: 1, hatch: { interval_mm: 0.1, angle_deg: 0, angle_increment_deg: 0, cross_hatch: false, bidirectional: false }, timing: null },
    sub_layers: [],
    ...extra,
  }) as Layer;

test("library view: libraries, tree, entries, search", () => {
  const v = parseLibraryView(VIEW);
  assert.equal(v.selected, "Fiber");
  assert.deepEqual(
    v.libraries.map((l) => [l.name, l.dirty]),
    [
      ["Fiber", true],
      ["Other", false],
    ],
  );
  const all = treeEntries(v.tree);
  assert.deepEqual(
    all.map((e) => e.path),
    ["Steel/1 mm/Black mark", "Steel/Engrave/Deep", "Wood/3 mm/Cut"],
  );
  assert.equal(all[0].summary?.sub_layers, 1);
  assert.equal(all[1].summary, null);
  assert.equal(all[2].material, "Wood", "the material falls back to the material node");
  assert.deepEqual(treeEntries(filterTree(v.tree, "deep")).map((e) => e.id), ["m0.e1"]);
  assert.deepEqual(treeEntries(filterTree(v.tree, "wood")).map((e) => e.id), ["m1.e0"]);
  assert.deepEqual(treeEntries(filterTree(v.tree, "1 MM")).map((e) => e.id), ["m0.e0"]);
  assert.equal(treeEntries(filterTree(v.tree, "")).length, 3);
  assert.deepEqual(parseLibraryView(null), { selected: null, libraries: [], tree: [] });
});

test("library names and entry descriptions follow the engine's rules", () => {
  assert.equal(libraryNameFromFile("C:\\libs\\parameters-60w150x150mm.clb"), "parameters-60w150x150mm");
  assert.equal(libraryNameFromFile("we?ird:.CLB"), "we_ird_");
  assert.equal(libraryNameProblem("", []), "Give the library a name.");
  assert.match(libraryNameProblem("a/b", []) ?? "", /cannot hold/);
  assert.match(libraryNameProblem("fiber", ["Fiber"]) ?? "", /already loaded/);
  assert.equal(libraryNameProblem("FIBER", ["Fiber"], "Fiber"), null, "renaming a library to another case of its own name");
  assert.deepEqual(entryMetaProblem({ material: "Steel", thickness_mm: 1, title: "", description: "Mark" }), {});
  assert.equal(entryMetaProblem({ material: "", thickness_mm: 1, title: "", description: "Mark" }).material, "Required");
  assert.equal(entryMetaProblem({ material: "Steel", thickness_mm: null, title: "", description: "Mark" }).title, "Required");
  assert.ok(entryMetaProblem({ material: "St/eel", thickness_mm: 1, title: "", description: "x" }).material);
  assert.ok(entryMetaProblem({ material: "Steel", thickness_mm: 0, title: "", description: "x" }).thickness_mm);
  assert.ok(entryMetaProblem({ material: "Steel", thickness_mm: 1, title: "", description: "x".repeat(201) }).description);
});

test("linked layers and Edit Cut", () => {
  const linked = layer("a", { link: { library: "fiber", path: "Steel/1 mm/Black mark" } });
  const doc = { layers: [linked, layer("b")], objects: [] } as unknown as GDocument;
  assert.deepEqual(layerLink(linked), { library: "fiber", path: "Steel/1 mm/Black mark" });
  assert.equal(layerLink(layer("b")), null);
  assert.deepEqual(linkedLayers(doc, "Fiber", "Steel/1 mm/Black mark").map((l) => l.id), ["a"], "library names compare ignoring case");
  assert.equal(linkedLayers(doc, "Fiber", "Steel/Engrave/Deep").length, 0);
  const edit = { mode: "line", power_pct: 50, speed_mm_s: 100, frequency_khz: 30, pulse_width_ns: null, passes: 2.4, interval_mm: 0.05 };
  const full = editCutSettings({ mode: "fill", power_pct: 1, hatch: { interval_mm: 1, angle_deg: 45 }, options: { wobble: { step_mm: 1, size_mm: 1 } } }, edit);
  assert.equal(full.passes, 2);
  assert.deepEqual(full.hatch, { interval_mm: 0.05, angle_deg: 45 });
  assert.deepEqual(full.options, { wobble: { step_mm: 1, size_mm: 1 } }, "the other options of the linked layer are kept");
  assert.equal(editCutSettings(null, edit).mode, "line");
  assert.equal(cutEditProblem({ ...edit, passes: 2 }), null);
  assert.match(cutEditProblem({ ...edit, passes: 2, power_pct: 120 }) ?? "", /Power/);
  assert.match(cutEditProblem({ ...edit, passes: 2, mode: "fill", interval_mm: 0 }) ?? "", /interval/);
});

test("preferences, the Job Checklist and approval texts", () => {
  const p = parsePrefs({
    device: "b6",
    device_prefs: { require_framing: true, play_sound: true, checklist: { enabled: true, items: ["Door", "Goggles"] }, layer_defaults: { "#ff0000": {}, "*": {} }, optimization_defaults: { enabled: true } },
    debug_log: true,
    check_for_updates: false,
  });
  assert.equal(p.require_framing, true);
  assert.deepEqual(p.checklist.items, ["Door", "Goggles"]);
  assert.deepEqual(p.layer_defaults, ["#ff0000", "*"]);
  assert.equal(p.has_optimization_defaults, true);
  assert.equal(p.debug_log, true);
  assert.equal(parsePrefs(null).require_framing, false);
  assert.equal(checklistProblem({ enabled: true, items: [] }), "An enabled checklist needs at least one item.");
  assert.match(checklistProblem({ enabled: false, items: ["  "] }) ?? "", /1 to 300/);
  assert.match(checklistProblem({ enabled: false, items: Array(31).fill("x") }) ?? "", /At most 30/);
  assert.equal(checklistProblem({ enabled: true, items: ["ok"] }), null);
  assert.deepEqual(checklistFromText(" a \n\n b\r\n"), ["a", "b"]);
  assert.equal(checklistTicks({ enabled: false, items: ["a"] }, []), null);
  assert.deepEqual(checklistTicks(p.checklist, [true]), [true, false]);
  assert.equal(checklistComplete(p.checklist, [true]), false);
  assert.equal(checklistComplete(p.checklist, [true, true]), true);
  assert.equal(checklistComplete({ enabled: false, items: [] }, []), true);
  assert.match(approvalRefusalText("Require Framing Before Start is on: frame the job (since its last edit) before approving it") ?? "", /frame the job/);
  assert.match(approvalRefusalText("the Job Checklist has 2 item(s); tick every one") ?? "", /Tick every item/);
  assert.match(approvalRefusalText("the door interlock (input 3) is open; close the enclosure") ?? "", /input 3/);
  assert.match(approvalRefusalText("another run is in progress") ?? "", /Another run/);
  assert.equal(approvalRefusalText("something else"), null);
  assert.equal(simPinActive({ connected: true, approval: { pin_set: true, physical_confirm: null, sim_pin: true } as never }), true);
  assert.equal(simPinActive({ connected: true }), false);
  assert.equal(simPinActive(null), false);
});

test("IO ports and the repeat table", () => {
  const io = ioPorts({ io: { door_protect: { bit: 3, active_low: true }, busy_light: 2, red_dot_always_on: true } });
  assert.deepEqual(io.door_protect, { bit: 3, active_low: true });
  assert.equal(io.start_framing, null);
  assert.equal(io.busy_light, 2);
  assert.equal(io.done_pulse_ms, 500);
  assert.equal(io.red_dot_always_on, true);
  assert.equal(ioPorts(null).door_protect, null);
  assert.equal(tableRole({ axis: { role: "linear_table" } }), "linear_table");
  assert.equal(tableRole({ axis: { role: "z" } }), null);
  assert.equal(tableRole(null), null);
  const body = tableBody({ origin: "ui", continuous: true, selection: ["a"] }, { increment: 10, settle_ms: 250.4, return_to_start: true, repeat: 3 });
  assert.deepEqual(body, { origin: "ui", selection: ["a"], repeat: 3, table: { increment: 10, settle_ms: 250, return_to_start: true } });
  assert.throws(() => tableBody({}, { increment: 0, settle_ms: 0, return_to_start: false, repeat: 3 }), /not 0/);
  assert.throws(() => tableBody({}, { increment: 1, settle_ms: 0, return_to_start: false, repeat: 1 }), /2 to 10000/);
  assert.deepEqual(batchBody({ origin: "ui", selection: ["x"], from_segment: 3, repeat: 2 }, ["a", "b"]), { origin: "ui", repeat: 2, queue: ["a", "b"] });
  assert.throws(() => batchBody({}, []), /at least one/);
  assert.throws(() => batchBody({}, Array(51).fill("p")), /At most 50/);
});

test("job log, updates, USB scan and the wizard name", () => {
  const rows = parseJobLog({
    entries: [
      { time_unix_s: 10, run_id: "r1", state: "finished", origin: "ui", device: "b6", project: null, summary: "s", duration_s: 2, completed: 1, error: null },
      { time_unix_s: 20, run_id: "r2", state: "failed", origin: "agent:claude", device: "b6", project: "p", summary: "", duration_s: 1.5, completed: 0, error: "boom" },
      "junk",
    ],
  });
  assert.deepEqual(rows.map((r) => r.run_id), ["r2", "r1"], "newest first, junk dropped");
  assert.deepEqual(jobLogTotals(rows), { runs: 2, finished: 1, failed: 1, seconds: 3.5 });
  assert.equal(originText("agent:claude"), "claude");
  assert.equal(originText("ui"), "You");
  const u = parseUpdateCheck({ current: "0.1.0", latest: "0.2.0", newer: true, url: "https://github.com/krzim/OpenEtch/releases/tag/v0.2.0", checked_unix_s: 5 });
  assert.ok(u);
  assert.match(updateText(u), /0\.2\.0 is available/);
  assert.equal(parseUpdateCheck({ current: "1", url: "https://evil.example/x" })?.url, null, "only GitHub links are offered");
  assert.match(updateText({ current: "1", latest: null, newer: false, url: null, error: "offline", checked_unix_s: 0 }), /failed: offline/);
  const s = parseScan({ usb_support: true, boards: [{ vid: 0x9588, pid: 0x9899, location: "1-2", serial: "", supported: true, note: "" }] });
  assert.equal(s.boards[0].serial, null);
  assert.equal(parseScan({ usb_support: false, boards: [], note: "no usb" }).note, "no usb");
  assert.equal(wizardDeviceName("markcfg7", "B6_150.cor"), "Galvo B6_150");
  assert.equal(wizardDeviceName("markcfg7", null), "Galvo");
  assert.equal(wizardDeviceName("MyLaser.cfg", null), "MyLaser");
});

test("start points: nearest closed node and the marks", () => {
  const nl = {
    id: "o",
    kind: "path",
    converts_on_edit: false,
    nodes: [
      { index: 0, subpath: 0, point: [0, 0], type: "corner", handle_in: null, handle_out: null, segment_in: null, segment_out: 0, endpoint: false },
      { index: 1, subpath: 0, point: [10, 0], type: "corner", handle_in: null, handle_out: null, segment_in: 0, segment_out: 1, endpoint: false },
      { index: 2, subpath: 1, point: [20, 0], type: "corner", handle_in: null, handle_out: null, segment_in: null, segment_out: 2, endpoint: true },
    ],
    segments: [],
    subpaths: [
      { index: 0, closed: true, first_node: 0, node_count: 2 },
      { index: 1, closed: false, first_node: 2, node_count: 1 },
    ],
  } as unknown as NodeList;
  assert.equal(nearestStartNode(nl, [9, 0.5], 2), 1);
  assert.equal(nearestStartNode(nl, [5, 0], 2), null, "too far from any node");
  assert.equal(nearestStartNode(nl, [20, 0], 2), null, "open subpaths take no start point");
  const doc = {
    objects: [{ id: "o", name: "o", layer: "l", geometry: { kind: "rect", width_mm: 1, height_mm: 1, corner_radius_mm: 0 }, transform: [1, 0, 0, 1, 5, 7] }],
    object_options: { o: { start: { node: 1, local: [1, 2], reverse: true } } },
  } as unknown as GDocument;
  assert.deepEqual(startMarks(doc), [{ id: "o", at: [6, 9], reverse: true }]);
  assert.deepEqual(startMarks({ objects: [] } as unknown as GDocument), []);
});

test("Print and Cut checks", () => {
  assert.equal(printCutProblem([null, "b"], [null, null], [null, null]), "Pick both targets.");
  assert.match(printCutProblem(["a", "a"], [null, null], [null, null]) ?? "", /different/);
  assert.match(printCutProblem(["a", "b"], [[0, 0], null], [null, null]) ?? "", /Set both/);
  assert.match(printCutProblem(["a", "b"], [[0, 0], [10, 0]], [[0, 0], [1, 0]]) ?? "", /5 mm apart/);
  assert.match(printCutProblem(["a", "b"], [[0, 0], [1, 0]], [[0, 0], [10, 0]]) ?? "", /positions/);
  assert.equal(printCutProblem(["a", "b"], [[0, 0], [10, 0]], [[0, 0], [10, 0]]), null);
});

test("material test: values, checks, params and the preview's burn order", () => {
  assert.deepEqual(axisValues({ param: "power", min: 10, max: 100, count: 10 }).map(show), ["10", "20", "30", "40", "50", "60", "70", "80", "90", "100"]);
  assert.deepEqual(axisValues({ param: "passes", min: 1, max: 4, count: 3 }), [1, 3, 4]);
  assert.deepEqual(axisValues({ param: "speed", min: 5, max: 9, count: 1 }), [5]);
  const t = { ...DEFAULT_MATERIAL_TEST, rows: { param: "speed" as const, min: 500, max: 1500, count: 3 }, columns: { param: "power" as const, min: 10, max: 30, count: 3 } };
  assert.equal(materialTestProblem(t), null);
  assert.match(materialTestProblem({ ...t, columns: { ...t.columns, param: "speed" } }) ?? "", /different/);
  assert.equal(materialTestProblem({ ...t, columns: { ...t.columns, count: 30 } }), null, "30 per axis, as LightBurn's presets use");
  assert.match(materialTestProblem({ ...t, columns: { ...t.columns, count: 31 } }) ?? "", /1 to 30/);
  assert.match(materialTestProblem({ ...t, columns: { param: "passes", min: 0, max: 3, count: 4 } }) ?? "", /passes are 1/);
  const pw = { ...t, columns: { param: "pulse_width" as const, min: 2, max: 200, count: 3 } };
  assert.equal(materialTestProblem(pw, [2, 101, 200]), null, "values on the table pass");
  assert.match(materialTestProblem(pw, [2, 200]) ?? "", /no 101 ns/);
  const params = materialTestParams({ ...t, title: " Steel ", border: true });
  assert.equal(params.title, "Steel");
  assert.deepEqual(params.border_settings, { mode: "line", power_pct: 30, speed_mm_s: 1000 });
  assert.deepEqual((params.base as { hatch: unknown }).hatch, { interval_mm: 0.05 });
  assert.equal("pulse_width_ns" in (params.base as object), false, "source default sends no pulse width");
  const pv = materialTestPreview(t);
  assert.equal(pv.boxes.length, 9);
  // Highest speed (bottom row) and lowest power (left column) first: row 2, column 0 is the first box.
  const first = pv.boxes.find((b) => b.order === 1)!;
  assert.match(first.label, /Speed \(mm\/s\) 1500|Speed 1500/);
  assert.match(first.label, /Power 10/);
  const last = pv.boxes.find((b) => b.order === 9)!;
  assert.match(last.label, /Speed 500 \/ Power 30/);
  assert.ok(last.shade > first.shade, "slow and strong burns darker");
  assert.deepEqual(new Set(pv.boxes.map((b) => b.order)).size, 9);
  assert.match(previewViewBox(pv), /^-?[\d.]+ -?[\d.]+ [\d.]+ [\d.]+$/);
});

test("material test presets: params back to the dialog, .lbmt extras kept, list and names", () => {
  const m = { ...DEFAULT_MATERIAL_TEST, rows: { param: "frequency" as const, min: 20, max: 200, count: 30 }, title: "Steel", border: true, border_power_pct: 12 };
  const back = materialTestFromParams(materialTestParams(m));
  assert.deepEqual({ ...back, hatch_extra: undefined }, { ...m, hatch_extra: undefined });
  // A preset imported from LightBurn's .lbmt: the engine's settings patch with hatch fields the dialog does not show.
  const lbmt = {
    columns: { param: "pulse_width", min: 2, max: 200, count: 6 },
    rows: { param: "interval", min: 0.005, max: 0.05, count: 10 },
    cell_width_mm: 4,
    cell_height_mm: 4,
    center: [-10, 5],
    base: { mode: "fill", power_pct: 50, speed_mm_s: 2000, frequency_khz: 60, pulse_width_ns: null, passes: 1, hatch: { interval_mm: 0.02, angle_deg: 45, cross_hatch: true, bidirectional: true } },
    label_settings: { mode: "line", power_pct: 20, speed_mm_s: 1500 },
    labels: true,
  };
  const f = materialTestFromParams(lbmt);
  assert.equal(f.columns.param, "pulse_width");
  assert.deepEqual(f.center, [-10, 5]);
  assert.equal(f.base.pulse_width_ns, null);
  assert.equal(f.base.interval_mm, 0.02);
  assert.equal(f.label_power_pct, 20);
  assert.equal(f.spacing_mm, DEFAULT_MATERIAL_TEST.spacing_mm, "not in the file: the default");
  assert.deepEqual((materialTestParams(f).base as { hatch: unknown }).hatch, { interval_mm: 0.02, angle_deg: 45, cross_hatch: true, bidirectional: true });
  assert.deepEqual(materialTestFromParams({ rows: { param: "bogus" } }).rows, DEFAULT_MATERIAL_TEST.rows);
  assert.deepEqual(
    testPresetList({ presets: [{ name: "b", params: {} }, { name: 3 }, { name: "a", params: {} }] }).map((p) => p.name),
    ["a", "b"],
  );
  assert.equal(testPresetNameProblem("MOPA steel"), null);
  assert.match(testPresetNameProblem("  ") ?? "", /name/);
  assert.match(testPresetNameProblem("x".repeat(101)) ?? "", /100/);
});

test("interval and focus tests", () => {
  assert.deepEqual(intervalValues(DEFAULT_INTERVAL_TEST), [0.08, 0.1, 0.12, 0.14, 0.16]);
  assert.equal(intervalTestProblem(DEFAULT_INTERVAL_TEST), null);
  assert.match(intervalTestProblem({ ...DEFAULT_INTERVAL_TEST, steps: 1 }) ?? "", /Steps/);
  assert.deepEqual(intervalTestParams({ ...DEFAULT_INTERVAL_TEST, pulse_width_ns: 100 }).base, { frequency_khz: 30, pulse_width_ns: 100 });
  assert.deepEqual(focusHeights({ ...DEFAULT_FOCUS_TEST, start_z_mm: -1, end_z_mm: 1, steps: 4 }), [-1, -0.5, 0, 0.5, 1]);
  assert.equal(focusTestProblem(DEFAULT_FOCUS_TEST), null);
  assert.match(focusTestProblem({ ...DEFAULT_FOCUS_TEST, steps: 0 }) ?? "", /Steps/);
  assert.deepEqual(focusTestParams(DEFAULT_FOCUS_TEST).base, { frequency_khz: 30 });
  const pv = focusTestPreview(DEFAULT_FOCUS_TEST);
  assert.equal(pv.lines.length, 9);
  assert.equal(pv.lines[1].x1 - pv.lines[0].x1, 2.5);
});
