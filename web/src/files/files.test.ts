// Unit tests for files/fileFormats.ts: format routing and per-format import options, export / print queries, the
// engine's backup / template / bundle answers, recent templates, notes on open, pages, masks, Trace Image options,
// Convert to Bitmap and 3D Sliced state.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument, Layer } from "../api/types.ts";
import {
  DEFAULT_EXPORT,
  DEFAULT_IMPORT_PREFS,
  DEFAULT_MODEL_OPTIONS,
  DEFAULT_PRINT,
  DEFAULT_TRACE,
  ENGINE_ACCEPT,
  autosaveStatus,
  backupLists,
  bitmapParams,
  bitmapPixels,
  bundleResult,
  engineFormatOf,
  exportQuery,
  fileSettingsOf,
  fileSettingsProblem,
  importPrefsFrom,
  importQuery,
  importTargetLayer,
  liveRecent,
  maskClips,
  maskOf,
  maskPair,
  needsOptions,
  nextPageName,
  notesOf,
  openedRevision,
  pageMoveTarget,
  pagesOf,
  pathPoints,
  printPageHtml,
  printQuery,
  pushRecent,
  resultGroup,
  showNotesOnLoad,
  slice3dState,
  templateList,
  templateNameOf,
  traceParams,
  traceProblem,
  ungroupAfter,
} from "./fileFormats.ts";

const q = (s: string) => Object.fromEntries(new URLSearchParams(s));

test("extensions route to the engine's import formats", () => {
  assert.equal(engineFormatOf("a.PDF"), "pdf");
  assert.equal(engineFormatOf("logo.ai"), "ai");
  assert.equal(engineFormatOf("x.eps"), "eps");
  assert.equal(engineFormatOf("plot.plt"), "plt");
  assert.equal(engineFormatOf("plot.hpgl"), "hpgl");
  assert.equal(engineFormatOf("board-F_Cu.gtl"), "gerber");
  assert.equal(engineFormatOf("board.gbr"), "gerber");
  assert.equal(engineFormatOf("board.drl"), "drill");
  for (const m of ["stl", "obj", "off", "ply", "3mf", "amf"]) assert.equal(engineFormatOf(`part.${m}`), "model");
  assert.equal(engineFormatOf("scan.TIFF"), "tiff");
  assert.equal(engineFormatOf("scan.tga"), "tga");
  assert.equal(engineFormatOf("photo.jpg"), "image");
  assert.equal(engineFormatOf("old.lbrn"), "lbrn");
  assert.equal(engineFormatOf("jig.lbt"), "lbt");
  assert.equal(engineFormatOf("tag.ezd"), "ezd");
  assert.equal(engineFormatOf("p.lbrn2"), null);
  assert.equal(engineFormatOf("p.openetch"), null);
  assert.equal(engineFormatOf("notes.txt"), null);
  assert.ok(needsOptions("model") && needsOptions("gerber") && needsOptions("drill") && !needsOptions("pdf"));
  for (const e of [".pdf", ".ai", ".stl", ".3mf", ".gbr", ".drl", ".tif", ".lbrn", ".lbt", ".ezd"]) assert.ok(ENGINE_ACCEPT.split(",").includes(e), e);
});

test("import to tool layers (LightBurn's Allow Importing to Tool Layers, UX-034)", () => {
  const base = { mode: "merge" as const, layer: "t1", format: "svg" as const, fileName: "a.svg" };
  assert.equal(q(importQuery({ ...base, prefs: DEFAULT_IMPORT_PREFS })).tool_layers, undefined);
  assert.equal(q(importQuery({ ...base, prefs: { ...DEFAULT_IMPORT_PREFS, toolLayers: true } })).tool_layers, "true");
  assert.equal(importPrefsFrom({ toolLayers: true }).toolLayers, true);
  assert.equal(importPrefsFrom({}).toolLayers, false, "off by default, as in LightBurn");
  const layers = [{ id: "t1", tool: "T1" }, { id: "l1" }, { id: "l2", tool: null }];
  assert.equal(importTargetLayer(layers, "t1", false), "l1", "off: never onto a tool layer");
  assert.equal(importTargetLayer(layers, "t1", true), "t1");
  assert.equal(importTargetLayer(layers, "l2", false), "l2");
  assert.equal(importTargetLayer([{ id: "t1", tool: "T1" }], "t1", false), null, "only tool layers: the engine picks");
  assert.equal(importTargetLayer(layers, null, false), null);
});

test("import query: only the options each format takes", () => {
  const base = { mode: "merge" as const, layer: "layer-2", prefs: DEFAULT_IMPORT_PREFS };
  assert.deepEqual(q(importQuery({ ...base, format: "svg", fileName: "a.svg" })), { format: "svg", mode: "merge", name: "a.svg", layer: "layer-2" });
  // Not centring: keep_position where the format has it, never for SVG or images.
  const keep = { ...base, prefs: { ...DEFAULT_IMPORT_PREFS, center: false } };
  assert.equal(q(importQuery({ ...keep, format: "dxf", fileName: "a.dxf" })).keep_position, "true");
  assert.equal(q(importQuery({ ...keep, format: "svg", fileName: "a.svg" })).keep_position, undefined);
  assert.equal(q(importQuery({ ...keep, format: "image", fileName: "a.png" })).keep_position, undefined);
  assert.equal(q(importQuery({ ...base, format: "image", fileName: "a.JPG" })).mime, "image/jpeg");
  const pdf = q(importQuery({ ...base, format: "pdf", fileName: "a.pdf", page: 2, prefs: { ...DEFAULT_IMPORT_PREFS, hiddenLayers: true }, widthMm: 50 }));
  assert.deepEqual([pdf.page, pdf.hidden_layers, pdf.width_mm], ["2", "true", "50"]);
  assert.equal(q(importQuery({ ...base, format: "pdf", fileName: "a.pdf", page: 1 })).page, undefined);
  assert.equal(q(importQuery({ ...base, format: "hpgl", fileName: "a.hpgl", unitsPerMm: 1016 / 25.4 })).units_per_mm, String(1016 / 25.4));
  const g = q(importQuery({ ...base, format: "gerber", fileName: "a.gbr", gerber: { kind: "excellon", invert: true } }));
  assert.deepEqual([g.kind, g.invert], ["excellon", "true"]);
  // A drill file already says Excellon through format=drill.
  assert.equal(q(importQuery({ ...base, format: "drill", fileName: "a.drl", gerber: { kind: "gerber", invert: false } })).kind, undefined);
  const m = q(
    importQuery({
      ...base,
      format: "model",
      fileName: "part.3MF",
      model: { ...DEFAULT_MODEL_OPTIONS, rotation: [90, 0, 0], dpi: 508, widthMm: 40, outline: true, depthCutoffMm: 3, sixteenBit: true },
    }),
  );
  assert.deepEqual(
    [m.model_format, m.rotation_deg, m.dpi, m.width_mm, m.outline, m.depth_cutoff_mm, m.sixteen_bit, m.keep_position],
    ["3mf", "90,0,0", "508", "40", "true", "3", "true", undefined],
  );
  // LightBurn files take a name and the mode only; a replacing open never sends a layer.
  assert.deepEqual(q(importQuery({ ...base, format: "lbt", fileName: "jig.lbt", mode: "replace" })), { format: "lbt", mode: "replace", name: "jig.lbt" });
  assert.equal(q(importQuery({ ...base, format: "pdf", fileName: "a.pdf", mode: "replace" })).layer, undefined);
});

test("import options: defaults, parsing and ungrouping", () => {
  assert.deepEqual(importPrefsFrom(null), DEFAULT_IMPORT_PREFS);
  assert.deepEqual(importPrefsFrom({ group: false, select: "no" }), { ...DEFAULT_IMPORT_PREFS, group: false });
  assert.equal(ungroupAfter("dxf", { ...DEFAULT_IMPORT_PREFS, group: false }), true);
  assert.equal(ungroupAfter("image", { ...DEFAULT_IMPORT_PREFS, group: false }), false);
  assert.equal(ungroupAfter("dxf", DEFAULT_IMPORT_PREFS), false);
  assert.equal(resultGroup({ group: "group-3" }), "group-3");
  assert.equal(resultGroup({ group: null }), null);
});

test("export and print queries send only what differs from the engine's defaults", () => {
  assert.deepEqual(q(exportQuery(DEFAULT_EXPORT)), { format: "svg" });
  assert.deepEqual(q(exportQuery({ ...DEFAULT_EXPORT, format: "dxf", ids: ["obj-1", "obj-2"], keepCircles: false, toleranceMm: 0.05, strokeMm: 2 })), {
    format: "dxf",
    ids: "obj-1,obj-2",
    keep_circles: "false",
    tolerance_mm: "0.05",
  });
  assert.deepEqual(q(exportQuery({ ...DEFAULT_EXPORT, format: "pdf", colors: "black", area: "workspace", images: false })), {
    format: "pdf",
    colors: "black",
    area: "workspace",
    images: "false",
  });
  assert.deepEqual(q(printQuery(DEFAULT_PRINT)), { format: "svg", colors: "black" });
  assert.deepEqual(q(printQuery({ ...DEFAULT_PRINT, format: "pdf", colors: "layers", paper: [210, 297], marginMm: 5, area: "bounds", ids: ["a"] })), {
    format: "pdf",
    colors: "layers",
    paper: "210,297",
    margin_mm: "5",
    area: "bounds",
    ids: "a",
  });
  const html = printPageHtml('data:image/svg+xml;base64,AAA"<x>', 'A <b> "c"');
  assert.ok(html.includes("&lt;b&gt;") && html.includes("&quot;&lt;x&gt;") && !html.includes("<b>"));
});

test("engine lists: backups, prefs backups, templates, settings, bundles", () => {
  const b = backupLists({
    autosaves: [
      { file: "a.1.openetch", project: "a", saved_unix_s: 1, size: 10 },
      { file: "a.3.openetch", project: "a", saved_unix_s: 3, size: 10 },
      { nope: true },
    ],
  });
  assert.deepEqual(b.autosaves.map((x) => x.file), ["a.3.openetch", "a.1.openetch"]);
  assert.deepEqual(b.backups, []);
  assert.deepEqual(templateList([{ name: "Jig", kind: "lbt", modified_unix_s: 5, size: 1 }, { name: "" }, { name: "Tag" }]), [
    { name: "Jig", kind: "lbt", modifiedUnixS: 5, size: 1 },
    { name: "Tag", kind: "openetch", modifiedUnixS: 0, size: 0 },
  ]);
  assert.deepEqual(fileSettingsOf({ autosave_minutes: 0 }), { autosave_minutes: 0, autosave_keep: 10, backups_keep: 5 });
  assert.equal(fileSettingsProblem({ autosave_minutes: 5, autosave_keep: 10, backups_keep: 5 }), null);
  assert.ok(fileSettingsProblem({ autosave_minutes: 2000, autosave_keep: 10, backups_keep: 5 }));
  assert.ok(fileSettingsProblem({ autosave_minutes: 5, autosave_keep: 0, backups_keep: 5 }));
  assert.ok(fileSettingsProblem({ autosave_minutes: 5, autosave_keep: 1, backups_keep: 51 }));
  assert.deepEqual(autosaveStatus({ autosave_minutes: 5, autosave_keep: 10, backups_keep: 5 }, { file: "x", project: null, savedUnixS: 1000, size: 1 }, 1_180_000), {
    on: true,
    minutes: 5,
    agoMin: 3,
  });
  assert.equal(autosaveStatus({ autosave_minutes: 0, autosave_keep: 10, backups_keep: 5 }, null, 0).on, false);
  assert.deepEqual(bundleResult({ ok: true, imported: { prefs: true, devices: ["dev-2"], libraries: [] }, skipped: ["x"] }), {
    prefs: true,
    devices: ["dev-2"],
    libraries: [],
    skipped: ["x"],
  });
});

test("recent templates: most recent first, case-insensitive, only existing ones", () => {
  assert.deepEqual(pushRecent(["a", "B", "c"], "b"), ["b", "a", "c"]);
  assert.deepEqual(pushRecent(["a"], "  "), ["a"]);
  assert.equal(pushRecent(["1", "2", "3", "4", "5", "6", "7", "8"], "9").length, 8);
  const existing = templateList([{ name: "Jig" }, { name: "Tag" }]);
  assert.deepEqual(liveRecent(["tag", "gone", "Jig"], existing).map((t) => t.name), ["Tag", "Jig"]);
  assert.equal(templateNameOf("C:\\x\\Coaster jig.LBT"), "Coaster jig");
  assert.equal(templateNameOf("a:b.lbt"), "a_b");
});

const doc = (extra: Partial<GDocument> & Record<string, unknown> = {}): GDocument =>
  ({
    revision: 1,
    name: "d",
    stock: null,
    layers: [],
    objects: [
      { id: "img", name: "Photo", layer: "l", geometry: { kind: "image", width_mm: 10, height_mm: 10, mime: "image/png", data_base64: "" }, transform: [1, 0, 0, 1, 0, 0] },
      { id: "r", name: "Rect", layer: "l", geometry: { kind: "rect", width_mm: 4, height_mm: 4, corner_radius_mm: 0 }, transform: [1, 0, 0, 1, 1, 2] },
      { id: "e", name: "Ell", layer: "l", geometry: { kind: "ellipse", rx_mm: 1, ry_mm: 1 }, transform: [1, 0, 0, 1, 0, 0], group: "g2" },
    ],
    groups: [
      { id: "g1", name: "outer" },
      { id: "g2", name: "inner", parent: "g1" },
    ],
    next_id: 9,
    ...extra,
  }) as GDocument;

test("notes: read from the document, shown when a file opens with show on load", () => {
  assert.deepEqual(notesOf(doc()), { text: "", show_on_load: false });
  const n = notesOf(doc({ notes: { text: "Use the jig", show_on_load: true } }));
  assert.equal(showNotesOnLoad(null, "a#1", n), true);
  assert.equal(showNotesOnLoad("a#1", "a#1", n), false);
  assert.equal(showNotesOnLoad("a#1", "b#5", { ...n, show_on_load: false }), false);
  assert.equal(showNotesOnLoad("a#1", "b#5", { text: "  ", show_on_load: true }), false);
  // Opening starts a fresh timeline with one loading step.
  const step = (commands: string, revision: number) => ({ commands, revision });
  assert.equal(openedRevision({ steps: [step("doc.load", 13)] }), 13);
  assert.equal(openedRevision({ steps: [step("doc.new, art.import_pdf", 4)] }), 4);
  assert.equal(openedRevision({ steps: [step("doc.import_lbrn2", 2)] }), 2);
  assert.equal(openedRevision({ steps: [step("doc.load", 13), step("shape.rect", 14)] }), null);
  assert.equal(openedRevision({ steps: [step("shape.rect", 1)] }), null);
  assert.equal(openedRevision({ steps: [step("xdoc.loader", 1)] }), null);
  assert.equal(openedRevision({ steps: [] }), null);
});

test("pages: rows, active page counts the document's objects, moves and names", () => {
  assert.deepEqual(pagesOf(doc()), [{ id: null, name: "Page 1", objects: 3, active: true }]);
  const d = doc({ pages: [{ id: "page-1", name: "Front" }, { id: "page-2", name: "Back", objects: [{}, {}] }], active_page: "page-1" });
  assert.deepEqual(pagesOf(d), [
    { id: "page-1", name: "Front", objects: 3, active: true },
    { id: "page-2", name: "Back", objects: 2, active: false },
  ]);
  assert.equal(pageMoveTarget(2, 0, -1), null);
  assert.equal(pageMoveTarget(2, 0, 1), 1);
  assert.equal(pageMoveTarget(2, 1, 1), null);
  assert.equal(nextPageName([{ id: "a", name: "Page 2", objects: 0, active: true }]), "Page 3");
});

test("masks: the mask of an image, its clip path, the Apply Mask pair", () => {
  const d = doc({ image_masks: { img: "r" } });
  assert.equal(maskOf(d, "img"), "r");
  assert.equal(maskOf(d, "r"), null);
  const clip = maskClips(d).get("img");
  assert.ok(clip && clip.startsWith("M"));
  // A group mask (nested groups too) clips with all its members.
  assert.ok(maskClips(doc({ image_masks: { img: "g1" } })).get("img"));
  assert.equal(maskClips(doc({ image_masks: { r: "img" } })).size, 0);
  const objectOf = (id: string) => doc().objects.find((o) => o.id === id);
  assert.deepEqual(maskPair(["r", "img"], objectOf), { image: "img", mask: "r" });
  assert.deepEqual(maskPair(["img", "g1"], objectOf), { image: "img", mask: "g1" });
  assert.equal(maskPair(["r", "e"], objectOf), null);
  assert.equal(maskPair(["img"], objectOf), null);
});

test("trace options: checks, parameters and points", () => {
  assert.equal(traceProblem(DEFAULT_TRACE), null);
  assert.ok(traceProblem({ ...DEFAULT_TRACE, cutoff: 200, threshold: 100 }));
  assert.ok(traceProblem({ ...DEFAULT_TRACE, smoothness: 2 }));
  assert.deepEqual(traceParams("img", DEFAULT_TRACE, true), { id: "img" });
  assert.deepEqual(traceParams("img", { ...DEFAULT_TRACE, threshold: 90, centerline: true, deleteImage: true, boundary: [0, 1, 2, 3] }, true), {
    id: "img",
    threshold: 90,
    centerline: true,
    boundary: { min: [0, 1], max: [2, 3] },
  });
  assert.equal(traceParams("img", { ...DEFAULT_TRACE, deleteImage: true }, false).delete_image, true);
  assert.deepEqual(pathPoints("M0,0 L1,2 C1,1 2,2 3,3 Z M-1.5 -2e1 L4 5"), [
    [0, 0],
    [1, 2],
    [3, 3],
    [-1.5, -20],
    [4, 5],
  ]);
});

test("convert to bitmap parameters and size", () => {
  assert.deepEqual(bitmapParams(["a"], { dpi: 254, render: "cut_settings", gray: 128, lineWidthMm: null, keepOriginal: false }), { ids: ["a"], render: "cut_settings" });
  assert.deepEqual(bitmapParams(["a"], { dpi: 600, render: "outlines", gray: 0, lineWidthMm: 0.2, keepOriginal: true }), {
    ids: ["a"],
    render: "outlines",
    dpi: 600,
    gray: 0,
    line_width_mm: 0.2,
    keep_original: true,
  });
  assert.deepEqual(bitmapPixels(25.4, 12.7, 100), [100, 50]);
});

test("3D Sliced state: slices are passes, the cleanup sub-layer and clean whole image", () => {
  const layer = {
    id: "l",
    name: "L",
    color: "#000",
    visible: true,
    output: true,
    settings: { passes: 12, image: { dither: "slice3d", clean_whole_image: true } },
    sub_layers: [{ passes: 1 }, { passes: 1, cleanup: { every_n_passes: 4 } }],
  } as unknown as Layer;
  assert.deepEqual(slice3dState(layer), { slices: 12, cleanup: 1, everyN: 4, cleanWholeImage: true });
  const plain = { ...layer, settings: { passes: 3 }, sub_layers: [] } as unknown as Layer;
  assert.deepEqual(slice3dState(plain), { slices: 3, cleanup: null, everyN: 3, cleanWholeImage: false });
});
