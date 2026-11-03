// Unit tests of the MOPA materials model (materials/model.ts): finishes as the engine sends them, the finish form and
// its patch, photo scaling, the palette, the Result colours view, bracket tests found in a document, nudge answers
// and the colour separation body.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument, Layer } from "../api/types.ts";
import { parseTree } from "../libs/model.ts";
import {
  bracketTests,
  bySurface,
  deltaLines,
  finishForm,
  finishFormProblem,
  finishPatch,
  inkOn,
  linkedColorLookup,
  normalHex,
  paletteOf,
  parseFinish,
  parseHueModel,
  parseNudge,
  parseSeparation,
  pctText,
  resultColorOf,
  sameLine,
  scaledSize,
  selectedCell,
  separateBody,
  validDate,
  withResultColors,
  DEFAULT_SEPARATE,
  XTOOL_MODEL,
} from "./model.ts";

const layer = (id: string, color: string, extras?: Record<string, string>, link?: { library: string; path: string }): Layer =>
  ({
    id,
    name: id,
    color,
    visible: true,
    output: true,
    settings: { mode: "fill" } as Layer["settings"],
    sub_layers: [],
    extras,
    ...(link ? { link } : {}),
  }) as Layer;

const doc = (layers: Layer[], extra: Partial<GDocument> = {}): GDocument => ({ revision: 1, name: "d", stock: null, layers, objects: [], next_id: 1, ...extra });

const TREE = {
  materials: [
    {
      name: "Steel",
      groups: [
        {
          title: "Colours",
          entries: [
            { id: "m0.e0", path: "Steel/Colours/Gold", description: "Gold", finish: { color: "#C8A040", surface: "brushed", lot: "A7", date: "2026-11-02", photo: { mime: "image/jpeg", width: 640, height: 480, average: "#c8a040", taken_unix_s: 10 }, color_from_photo: true } },
            { id: "m0.e1", path: "Steel/Colours/Blue", description: "Blue", finish: { color: "#2040c0", surface: "mirror", photo_stale: true } },
            { id: "m0.e2", path: "Steel/Colours/Plain", description: "Plain" },
          ],
        },
      ],
    },
  ],
};

test("finishes parse as the engine sends them, and a tree entry carries its finish", () => {
  const tree = parseTree(TREE);
  const [gold, blue, plain] = tree[0].groups[0].entries;
  assert.equal(gold.finish?.color, "#c8a040", "colours are normalised");
  assert.equal(gold.finish?.surface, "brushed");
  assert.deepEqual(gold.finish?.photo, { mime: "image/jpeg", width: 640, height: 480, average: "#c8a040", takenUnixS: 10 });
  assert.equal(blue.finish?.photoStale, true);
  assert.equal(plain.finish, null);
  assert.equal(parseFinish({ surface: "sandblasted", color: "nope" })?.surface, null);
  assert.equal(parseFinish({ color: "nope" })?.color, null);
  assert.equal(normalHex("#AbC"), "#aabbcc");
  assert.equal(normalHex("red"), null);
});

test("the finish form keeps a picked colour, sends only changes and checks its fields", () => {
  const f = parseFinish({ color: "#c8a040", color_from_photo: true, lot: "A7", focus_mm: -1.5 });
  const base = finishForm(f);
  assert.equal(base.color, null, "a colour taken from the photo is not a picked one");
  assert.equal(base.focusMm, -1.5);
  const after = { ...base, color: "#112233", lot: " B8 ", notes: "bluer" };
  assert.deepEqual(finishPatch(base, after), { color: "#112233", lot: "B8", notes: "bluer" });
  assert.deepEqual(finishPatch(after, { ...after, color: null }), { color: null }, "null: back to the photo's average");
  assert.deepEqual(finishPatch(base, base), {});
  assert.deepEqual(finishFormProblem({ ...base, date: "2026-13-01", focusMm: 500, color: "blue" }), { date: "YYYY-MM-DD", focusMm: "-100 … 100", color: "#rrggbb" });
  assert.deepEqual(finishFormProblem(base), {});
  assert.ok(validDate("2026-02-28") && !validDate("2026-2-28") && !validDate("2026-00-10"));
});

test("photos scale to 1280 px on their long side and are never enlarged", () => {
  assert.deepEqual(scaledSize(4032, 3024), { w: 1280, h: 960 });
  assert.deepEqual(scaledSize(3024, 4032), { w: 960, h: 1280 });
  assert.deepEqual(scaledSize(800, 600), { w: 800, h: 600 });
  assert.deepEqual(scaledSize(0, 600), { w: 0, h: 0 });
});

test("text on a swatch is black on light colours and white on dark ones", () => {
  assert.equal(inkOn("#f0e0a0"), "#000000");
  assert.equal(inkOn("#102060"), "#ffffff");
});

test("the palette lists presets with a colour, grouped by surface", () => {
  const tree = parseTree(TREE);
  const sw = paletteOf(tree);
  assert.deepEqual(
    sw.map((s) => [s.id, s.color, s.surface, s.hasPhoto, s.photoStale]),
    [
      ["m0.e0", "#c8a040", "brushed", true, false],
      ["m0.e1", "#2040c0", "mirror", false, true],
    ],
  );
  assert.deepEqual(
    bySurface(sw).map((g) => [g.surface, g.swatches.length]),
    [
      ["brushed", 1],
      ["mirror", 1],
    ],
  );
  assert.deepEqual(
    bySurface(sw, "mirror").map((g) => g.surface),
    ["mirror"],
  );
  assert.deepEqual(bySurface(sw, "polished"), []);
});

test("Result colours: linked layers follow their preset, others keep the colour they were given", () => {
  const tree = parseTree(TREE);
  const lookup = linkedColorLookup("Steel-lib", tree);
  const linked = layer("a", "#ff0000", undefined, { library: "steel-LIB", path: "Steel/Colours/Blue" });
  const given = layer("b", "#00ff00", { "finish.color": "#C08040" });
  const plain = layer("c", "#0000ff");
  const elsewhere = layer("d", "#0000ff", { "finish.color": "#123456" }, { library: "other", path: "Steel/Colours/Blue" });
  assert.equal(resultColorOf(linked, lookup), "#2040c0");
  assert.equal(resultColorOf(given, lookup), "#c08040");
  assert.equal(resultColorOf(plain, lookup), null);
  assert.equal(resultColorOf(elsewhere, lookup), "#123456", "another library's link falls back to the given colour");
  const d = doc([linked, given, plain]);
  const shown = withResultColors(d, lookup);
  assert.deepEqual(
    shown.layers.map((l) => l.color),
    ["#2040c0", "#c08040", "#0000ff"],
  );
  assert.equal(d.layers[0].color, "#ff0000", "the document itself is not changed");
  const none = doc([plain]);
  assert.equal(withResultColors(none, lookup), none, "nothing to change: the same object");
});

test("bracket tests are found by their group, and a selected box by its objects", () => {
  const cells = [
    layer("l1", "#111111", { "bracket.cell": "0,0" }),
    layer("l2", "#222222", { "bracket.cell": "0,-5" }),
    layer("l3", "#333333", { "bracket.cell": "5,5" }),
    layer("labels", "#000000"),
  ];
  const objects = [
    { id: "o1", layer: "l1", group: "g1" },
    { id: "o2", layer: "l2", group: "g1" },
    { id: "o3", layer: "l3", group: "g1" },
    { id: "o4", layer: "labels", group: "g1" },
    { id: "o5", layer: "l1", group: null },
  ] as unknown as GDocument["objects"];
  const groups = [{ id: "g1", name: "Bracket test", extras: { "bracket.path": "Steel/Colours/Gold", "bracket.library": "steel", "bracket.rows": "speed", "bracket.columns": "power" } }, { id: "g2", name: "Other" }];
  const tests = bracketTests(doc(cells, { objects, groups } as Partial<GDocument>));
  assert.equal(tests.length, 1);
  assert.deepEqual(
    tests[0].cells.map((c) => [c.layer, c.rowPct, c.colPct, c.objects]),
    [
      ["l1", 0, 0, ["o1"]],
      ["l2", 0, -5, ["o2"]],
      ["l3", 5, 5, ["o3"]],
    ],
  );
  assert.equal(selectedCell(tests, ["o2"])?.cell.layer, "l2");
  assert.equal(selectedCell(tests, ["o2", "o3"]), null);
  assert.equal(selectedCell(tests, []), null);
  const [a, b, c] = tests[0].cells;
  assert.ok(sameLine(a, b) && !sameLine(a, c) && !sameLine(a, a));
  assert.equal(pctText(-5), "−5 %");
  assert.equal(pctText(10), "+10 %");
  assert.equal(pctText(0), "0 %");
  assert.deepEqual(bracketTests(null), []);
});

test("hue models and nudge answers parse with xTool's defaults", () => {
  assert.deepEqual(parseHueModel(null), XTOOL_MODEL);
  assert.equal(parseHueModel({ power_pct: 0.8, source: "bracket" }).power_pct, 0.8);
  const n = parseNudge({ settings: { power_pct: 39.5, speed_mm_s: 1000, frequency_khz: 600 }, param: "power", delta: { power_pct: -0.5, x: "no" }, warnings: ["w", 3], applied: false });
  assert.deepEqual(n.delta, { power_pct: -0.5 });
  assert.deepEqual(n.warnings, ["w"]);
  assert.deepEqual(deltaLines({ power_pct: 40, speed_mm_s: 1000, frequency_khz: 600 }, n, (v) => `${v} mm/s`), ["40 → 39.5 %"]);
  const f = parseNudge({ settings: { frequency_khz: 750 }, delta: { frequency_khz: 150 } });
  assert.deepEqual(deltaLines({ power_pct: 40, speed_mm_s: 1000, frequency_khz: 600 }, f, String), ["600 → 750 kHz"]);
});

test("colour separation: the request body and the preview answer", () => {
  const b = separateBody("obj-3", { ...DEFAULT_SEPARATE, colors: 40, library: "steel", skip: [2] }, true);
  assert.deepEqual(b, { image: "obj-3", colors: 16, background: "auto", smoothness: 1, library: "steel", link: true, max_distance: 30, skip: [2], preview: true });
  const c = separateBody("obj-3", { ...DEFAULT_SEPARATE, minSpeckPx: 12.4, deleteImage: true }, false);
  assert.equal(c.min_speck_px, 12);
  assert.equal(c.delete_image, true);
  assert.equal(c.preview, undefined);
  const pv = parseSeparation({
    colors: [
      { index: 0, color: "#FFFFFF", pixels: 900, share: 75, background: true, traced: false, swatch: null },
      { index: 1, color: "#d21e1e", pixels: 300, share: 25, background: false, traced: true, swatch: { name: "Red", color: "#d02020", distance: 2.1, link: { library: "steel", path: "Steel/Colours/Red" } } },
      { nope: true },
    ],
    png_base64: "iVBOR",
    width: 40,
    height: 30,
  });
  assert.equal(pv.colors.length, 2);
  assert.equal(pv.colors[0].color, "#ffffff");
  assert.deepEqual(pv.colors[1].swatch, { name: "Red", color: "#d02020", distance: 2.1, path: "Steel/Colours/Red" });
  assert.equal(pv.png, "iVBOR");
});
