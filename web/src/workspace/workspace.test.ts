// Unit tests for the workspace layer's pure logic: preferences, units, theme colours, i18n, hotkeys, palette, the
// Cuts/Layers table and the numeric edits maths.

import { test } from "node:test";
import assert from "node:assert/strict";
import type { CutSettings, Layer } from "../api/types.ts";
import {
  ACTIONS,
  bindChord,
  chordOf,
  chordsFor,
  conflictsOf,
  eventInitOf,
  hintFor,
  normalizeChord,
  parseChord,
  resetAction,
  resolveKey,
  unbindChord,
} from "./hotkeys.ts";
import { interpolate, languages, registerLocale, translate, translateDynamic } from "./i18n.ts";
import { copyLayerSettings, dropIndex, invertAll, layerRows, onlyThis, pasteLayerSettings, setAll, settingsPatch, stepIndex } from "./layersTable.ts";
import { anchorPoint, getAnchor, moveTo, parseAnchor, resizeTo, rotationDelta, scalePercent, setAnchor, subscribeAnchor } from "./numeric.ts";
import { assignPlan, entryForLayer, layerForEntry, normColor, PALETTE } from "./palette.ts";
import { DEFAULT_PREFS, sanitizePrefs } from "./prefs.ts";
import { canvasColor, contrast, FIELD_BG, parseHex, resolveTheme } from "./theme.ts";
import { coord, fixedLength, fixedNumber, formatLength, formatSpeed, lengthFromDisplay, lengthText, lengthToDisplay, speedFromDisplay, speedToDisplay, unitConversion, zoomPercent } from "./units.ts";

// --- prefs ----------------------------------------------------------------------------------------------------------------

test("sanitizePrefs keeps valid values and drops garbage", () => {
  assert.deepEqual(sanitizePrefs(null), DEFAULT_PREFS);
  assert.deepEqual(sanitizePrefs("nope"), DEFAULT_PREFS);
  const p = sanitizePrefs({
    theme: "dark",
    units: "in",
    speedUnit: "mm/min",
    gridMm: 2.5,
    snapTolPx: 500,
    snapKinds: { midpoint: false, bogus: false },
    nudge: { normal: 2, shift: -1 },
    uiScale: 1.2,
    language: "de",
    hotkeys: { "edit.copy": ["Ctrl+K"], bad: "Ctrl+J", worse: [1, 2] },
    docks: { cuts: true, laser: "x" },
    extra: 1,
  });
  assert.equal(p.theme, "dark");
  assert.equal(p.units, "in");
  assert.equal(p.speedUnit, "mm/min");
  assert.equal(p.gridMm, 2.5);
  assert.equal(p.snapTolPx, DEFAULT_PREFS.snapTolPx, "out of range falls back");
  assert.equal(p.snapKinds.midpoint, false);
  assert.equal(p.snapKinds.center, true);
  assert.equal(p.nudge.normal, 2);
  assert.equal(p.nudge.shift, DEFAULT_PREFS.nudge.shift);
  assert.equal(p.uiScale, 1.2);
  assert.equal(p.language, "de");
  assert.deepEqual(p.hotkeys, { "edit.copy": ["Ctrl+K"] });
  assert.deepEqual(p.docks, { cuts: true });
  assert.equal("extra" in p, false);
  assert.equal(sanitizePrefs({ theme: "purple", language: "../../x" }).theme, "system");
  assert.equal(sanitizePrefs({ language: "../../x" }).language, "en");
});

// --- units ----------------------------------------------------------------------------------------------------------------

test("lengths and speeds convert both ways", () => {
  assert.equal(lengthToDisplay(25.4, "in"), 1);
  assert.equal(lengthFromDisplay(2, "in"), 50.8);
  assert.equal(lengthToDisplay(12, "mm"), 12);
  assert.equal(speedToDisplay(100, "mm/min"), 6000);
  assert.equal(speedFromDisplay(6000, "mm/min"), 100);
  assert.ok(Math.abs(speedToDisplay(254, "in/s") - 10) < 1e-12);
  assert.ok(Math.abs(speedFromDisplay(10, "in/s") - 254) < 1e-12);
  assert.equal(speedToDisplay(100, "mm/s"), 100);
});

test("unitConversion converts only mm and mm/s fields", () => {
  const inch = unitConversion("mm", { units: "in", speedUnit: "mm/s" });
  assert.equal(inch.unit, "in");
  assert.equal(inch.to(25.4), 1);
  assert.equal(inch.from(1), 25.4);
  assert.equal(inch.extraDigits, 1);
  const mm = unitConversion("mm", { units: "mm", speedUnit: "mm/s" });
  assert.equal(mm.unit, "mm");
  assert.equal(mm.to(3), 3);
  const sp = unitConversion("mm/s", { units: "mm", speedUnit: "mm/min" });
  assert.equal(sp.unit, "mm/min");
  assert.equal(sp.to(10), 600);
  assert.equal(sp.from(600), 10);
  const deg = unitConversion("°", { units: "in", speedUnit: "in/s" });
  assert.equal(deg.unit, "°");
  assert.equal(deg.to(45), 45);
  assert.equal(unitConversion(undefined, { units: "in", speedUnit: "in/s" }).unit, undefined);
});

test("formatting and zoom", () => {
  assert.equal(formatLength(12.3456, "mm"), "12.35 mm");
  assert.equal(formatLength(25.4, "in"), "1 in");
  assert.equal(formatSpeed(1000, "mm/s"), "1000 mm/s");
  assert.equal(formatSpeed(1000, "mm/min"), "60000 mm/min");
  assert.equal(coord(-0.0001, "mm"), "-0.000");
  assert.equal(coord(25.4, "in"), "1.0000");
  assert.ok(Math.abs(zoomPercent(96 / 25.4) - 100) < 1e-9);
});

// --- theme ----------------------------------------------------------------------------------------------------------------

test("resolveTheme follows the system only for Match system", () => {
  assert.equal(resolveTheme("system", true), "dark");
  assert.equal(resolveTheme("system", false), "light");
  assert.equal(resolveTheme("light", true), "light");
  assert.equal(resolveTheme("dark", false), "dark");
});

test("canvasColor keeps readable colours and lifts invisible ones", () => {
  const darkBg = parseHex(FIELD_BG.dark) as [number, number, number];
  const lightBg = parseHex(FIELD_BG.light) as [number, number, number];
  // Black on the dark field gets lighter until it reads; red stays red.
  const black = canvasColor("#000000", "dark");
  assert.notEqual(black, "#000000");
  assert.ok(contrast(parseHex(black) as [number, number, number], darkBg) >= 3);
  assert.equal(canvasColor("#ff0000", "dark"), "#ff0000");
  assert.notEqual(canvasColor("#0000ff", "dark"), "#0000ff", "pure blue is lifted on the dark field");
  // On the light field black is fine, a pale yellow is darkened.
  assert.equal(canvasColor("#000000", "light"), "#000000");
  const pale = canvasColor("#ffdb66", "light");
  assert.notEqual(pale, "#ffdb66");
  assert.ok(contrast(parseHex(pale) as [number, number, number], lightBg) >= 1.5);
  assert.equal(canvasColor("#ff0000", "light"), "#ff0000");
  // Non-hex colours pass through; none gives a neutral.
  assert.equal(canvasColor("rebeccapurple", "dark"), "rebeccapurple");
  assert.equal(canvasColor(undefined, "light"), "#333333");
  assert.deepEqual(parseHex("#abc"), [0xaa, 0xbb, 0xcc]);
  assert.equal(parseHex("red"), null);
});

// --- i18n -----------------------------------------------------------------------------------------------------------------

test("translate falls back to English and interpolates", () => {
  assert.equal(translate("en", "common.close"), "Close");
  assert.equal(translate("xx", "common.close"), "Close");
  registerLocale("de", "Deutsch", { "common.close": "Schließen", "layers.copied": "Einstellungen von „{name}“ kopiert." });
  assert.equal(translate("de", "common.close"), "Schließen");
  assert.equal(translate("de-AT", "common.close"), "Schließen", "region falls back to the language");
  assert.equal(translate("de", "common.cancel"), "Cancel", "missing key falls back to English");
  assert.equal(translate("de", "layers.copied", { name: "C01" }), "Einstellungen von „C01“ kopiert.");
  assert.equal(interpolate("{a} and {b}", { a: 1 }), "1 and {b}");
  assert.equal(translateDynamic("en", "hk.unknown", "Fallback {x}", { x: 2 }), "Fallback 2");
  assert.equal(languages()[0].code, "en");
  assert.ok(languages().some((l) => l.code === "de"));
});

// --- hotkeys --------------------------------------------------------------------------------------------------------------

const key = (k: string, m: { ctrl?: boolean; alt?: boolean; shift?: boolean; meta?: boolean; code?: string } = {}) => ({
  key: k,
  code: m.code,
  ctrlKey: !!m.ctrl,
  metaKey: !!m.meta,
  altKey: !!m.alt,
  shiftKey: !!m.shift,
});

test("chordOf names key presses like LightBurn", () => {
  assert.equal(chordOf(key("h", { ctrl: true, shift: true })), "Ctrl+Shift+H");
  assert.equal(chordOf(key("H", { ctrl: true, shift: true })), "Ctrl+Shift+H");
  assert.equal(chordOf(key("z", { meta: true })), "Ctrl+Z", "Cmd counts as Ctrl");
  assert.equal(chordOf(key("ArrowLeft", { alt: true })), "Alt+Left");
  assert.equal(chordOf(key("PageUp")), "PgUp");
  assert.equal(chordOf(key("Delete")), "Del");
  assert.equal(chordOf(key("?", { shift: true })), "Shift+?");
  assert.equal(chordOf(key("+", { alt: true, shift: true })), "Alt+Shift++");
  assert.equal(chordOf(key("F1")), "F1");
  assert.equal(chordOf(key("Shift", { shift: true })), null);
  // Option+V on a Mac types "√": the physical key names it.
  assert.equal(chordOf(key("√", { alt: true, code: "KeyV" })), "Alt+V");
});

test("parseChord and normalizeChord", () => {
  assert.deepEqual(parseChord("Ctrl+Shift++"), { ctrl: true, alt: false, shift: true, key: "+" });
  assert.deepEqual(parseChord("Alt+-"), { ctrl: false, alt: true, shift: false, key: "-" });
  assert.equal(normalizeChord("shift+ctrl+h"), "Ctrl+Shift+H");
  assert.equal(normalizeChord("cmd+z"), "Ctrl+Z");
  assert.equal(normalizeChord("Hyper+Z"), null);
  assert.equal(normalizeChord(""), null);
  assert.equal(normalizeChord("+"), "+");
});

test("every default chord is canonical and app actions list native chords", () => {
  for (const a of ACTIONS) {
    for (const c of a.defaults) if (!a.fixed) assert.equal(normalizeChord(c), c, `${a.id}: ${c}`);
    if (a.native) for (const c of a.native) assert.ok(a.defaults.includes(c), `${a.id} native ${c} is a default`);
  }
  const ids = ACTIONS.map((a) => a.id);
  assert.equal(new Set(ids).size, ids.length, "unique ids");
});

test("eventInitOf builds what the app handlers test", () => {
  assert.deepEqual(eventInitOf("Ctrl+Shift+H"), { key: "H", code: "KeyH", ctrlKey: true, altKey: false, shiftKey: true });
  assert.deepEqual(eventInitOf("R"), { key: "r", code: "KeyR", ctrlKey: false, altKey: false, shiftKey: false });
  assert.deepEqual(eventInitOf("Del"), { key: "Delete", code: "Delete", ctrlKey: false, altKey: false, shiftKey: false });
  assert.equal(eventInitOf("Alt+-")?.code, "Minus");
  assert.equal(eventInitOf("Ctrl+`")?.code, "Backquote");
  assert.equal(eventInitOf("Alt+Left")?.key, "ArrowLeft");
});

test("resolveKey: defaults pass, extras translate, workspace keys run", () => {
  assert.deepEqual(resolveKey("Ctrl+G", {}), { kind: "pass" });
  assert.deepEqual(resolveKey("Ctrl+R", {}), { kind: "translate", action: "tool.rect", to: "R" });
  assert.deepEqual(resolveKey("F1", {}), { kind: "ws", action: "help.hotkeys" });
  assert.deepEqual(resolveKey("Ctrl+0", {}), { kind: "ws", action: "view.zoomPage" });
  assert.deepEqual(resolveKey("Ctrl+K", {}), { kind: "pass" }, "unbound keys are left alone");
});

test("rebinding moves keys, translates, and swallows the old native key", () => {
  let o = bindChord({}, "arrange.group", "Ctrl+K");
  assert.ok(o);
  o = unbindChord(o, "arrange.group", "Ctrl+G");
  assert.deepEqual(chordsFor(ACTIONS.find((a) => a.id === "arrange.group")!, o), ["Ctrl+K"]);
  assert.deepEqual(resolveKey("Ctrl+K", o), { kind: "translate", action: "arrange.group", to: "Ctrl+G" });
  assert.deepEqual(resolveKey("Ctrl+G", o), { kind: "block" });
  assert.equal(hintFor("arrange.group", o), "Ctrl+K");
  // Taking a key from another action.
  const o2 = bindChord({}, "edit.duplicate", "Ctrl+G");
  assert.ok(o2);
  assert.deepEqual(conflictsOf("Ctrl+G", "edit.duplicate", {}), ["arrange.group"]);
  assert.deepEqual(resolveKey("Ctrl+G", o2), { kind: "translate", action: "edit.duplicate", to: "Ctrl+D" });
  assert.deepEqual(chordsFor(ACTIONS.find((a) => a.id === "arrange.group")!, o2), []);
  // A key that also has a fixed meaning is never swallowed.
  const o3 = unbindChord({}, "tool.rect", "R");
  assert.deepEqual(resolveKey("R", o3), { kind: "pass" });
  // Reserved and fixed keys cannot be bound.
  assert.equal(bindChord({}, "edit.copy", "Ctrl+W"), null);
  assert.equal(bindChord({}, "arrange.nudge", "K"), null);
  // Reset drops the override; binding back to the defaults simplifies away.
  assert.deepEqual(resetAction(o, "arrange.group"), {});
  assert.deepEqual(bindChord(unbindChord({}, "edit.copy", "Ctrl+C"), "edit.copy", "Ctrl+C"), {});
});

// --- palette --------------------------------------------------------------------------------------------------------------

const settings = (over: Partial<CutSettings> = {}): CutSettings => ({
  mode: "line",
  power_pct: 20,
  speed_mm_s: 1000,
  frequency_khz: 30,
  pulse_width_ns: null,
  passes: 1,
  hatch: { interval_mm: 0.05, angle_deg: 0, angle_increment_deg: 0, cross_hatch: false, bidirectional: true },
  timing: null,
  ...over,
});
const layer = (id: string, name: string, color: string, over: Partial<Layer> = {}): Layer => ({
  id,
  name,
  color,
  visible: true,
  output: true,
  settings: settings(),
  sub_layers: [],
  ...over,
});

test("palette has LightBurn's 30 colours and two tool layers", () => {
  assert.equal(PALETTE.length, 32);
  assert.equal(PALETTE[0].name, "C00");
  assert.equal(PALETTE[1].color, "#0000ff");
  assert.equal(PALETTE[29].name, "C29");
  assert.deepEqual([PALETTE[30].name, PALETTE[31].name], ["T1", "T2"]);
  assert.ok(PALETTE[30].tool && !PALETTE[0].tool);
  assert.equal(normColor("#F00"), "#ff0000");
});

test("palette swatches find, reuse or create layers", () => {
  const layers = [layer("l1", "Engrave", "#0000FF"), layer("l2", "C02", "#ff0000"), layer("l3", "Other red", "#ff0000")];
  const objects = [
    { id: "a", layer: "l1" },
    { id: "b", layer: "l2" },
  ];
  assert.equal(layerForEntry(layers, PALETTE[1])?.id, "l1");
  assert.equal(layerForEntry(layers, PALETTE[2])?.id, "l2", "prefers the layer named like the swatch");
  assert.equal(entryForLayer(layers[0])?.name, "C01");
  assert.deepEqual(assignPlan(layers, objects, [], PALETTE[1]), { kind: "none", reason: "no-selection" });
  assert.deepEqual(assignPlan(layers, objects, ["a"], PALETTE[1]), { kind: "none", reason: "already" });
  assert.deepEqual(assignPlan(layers, objects, ["a", "b"], PALETTE[1]), { kind: "existing", layer: "l1" });
  assert.deepEqual(assignPlan(layers, objects, ["a"], PALETTE[5]), { kind: "create", name: "C05", color: "#ff8000", output: true });
  assert.deepEqual(assignPlan(layers, objects, ["a"], PALETTE[30]), { kind: "create", name: "T1", color: "#f36926", output: false });
});

// --- Cuts / Layers --------------------------------------------------------------------------------------------------------

test("layerRows: numbers, counts and Multi for sub-layers", () => {
  const doc = {
    layers: [layer("l1", "A", "#000000"), layer("l2", "B", "#ff0000", { sub_layers: [settings({ name: "Clean", passes: 2, cleanup: { every_n_passes: 1 } })] })],
    objects: [
      { id: "o1", layer: "l1" },
      { id: "o2", layer: "l1" },
    ],
  } as unknown as Parameters<typeof layerRows>[0];
  const rows = layerRows(doc);
  assert.deepEqual(
    rows.map((r) => [r.number, r.objects, r.multi, r.passes.length]),
    [
      [1, 2, false, 1],
      [2, 0, true, 2],
    ],
  );
  assert.equal(rows[1].passes[1].name, "Clean");
  assert.equal(rows[1].passes[1].cleanup, true);
});

test("bulk toggles send only what changes", () => {
  const ls = [layer("a", "A", "#000"), layer("b", "B", "#111", { output: false }), layer("c", "C", "#222", { visible: false })];
  assert.deepEqual(
    setAll(ls, "output", true).map((i) => i.params),
    [{ id: "b", output: true }],
  );
  assert.deepEqual(
    invertAll(ls, "visible").map((i) => i.params),
    [
      { id: "a", visible: false },
      { id: "b", visible: false },
      { id: "c", visible: true },
    ],
  );
  assert.deepEqual(
    onlyThis(ls, "c", "visible").map((i) => i.params),
    [
      { id: "a", visible: false },
      { id: "b", visible: false },
      { id: "c", visible: true },
    ],
  );
});

test("copy and paste layer settings as one batch", () => {
  const withImage = { ...settings({ mode: "fill", power_pct: 55 }), image: { dither: "threshold", interval_mm: 0.1, angle_deg: 0, angle_increment_deg: 0, bidirectional: true, negative: false, pass_through: false, dot_width_mm: 0, min_power_pct: 0, halftone_cells_per_inch: 50, halftone_angle_deg: 22.5, unknown_field: 1 } } as unknown as CutSettings;
  const src = layer("s", "Source", "#000", { settings: withImage, sub_layers: [settings({ name: "Second", power_pct: 10, cleanup: { every_n_passes: 2 } })] });
  const patch = settingsPatch(withImage) as Record<string, unknown>;
  assert.equal(patch.power_pct, 55);
  assert.equal(patch.timing, null);
  assert.equal("name" in patch, false);
  assert.equal("unknown_field" in (patch.image as object), false, "only keys layer.update accepts");
  const clip = copyLayerSettings(src);
  const target = layer("t", "Target", "#f00", { sub_layers: [settings(), settings()] });
  const invs = pasteLayerSettings(target, clip, true);
  assert.deepEqual(
    invs.map((i) => i.id),
    ["layer.update", "layer.sub_delete", "layer.sub_delete", "layer.sub_add"],
  );
  assert.deepEqual(
    invs.slice(1, 3).map((i) => i.params.index),
    [1, 0],
    "sub-layers are deleted from the end",
  );
  assert.equal((invs[3].params as { name?: string }).name, "Second");
  assert.deepEqual((invs[3].params as { cleanup?: unknown }).cleanup, { every_n_passes: 2 });
  assert.equal((invs[0].params as { name?: unknown }).name, undefined, "the target keeps its name");
  assert.deepEqual(
    pasteLayerSettings(target, clip, false).map((i) => i.id),
    ["layer.update"],
  );
});

// --- numeric edits --------------------------------------------------------------------------------------------------------

const box = { x0: 0, y0: 0, x1: 20, y1: 10 };

test("anchor points and moves", () => {
  assert.deepEqual(anchorPoint(box, "nw"), [0, 10]);
  assert.deepEqual(anchorPoint(box, "c"), [10, 5]);
  assert.deepEqual(anchorPoint(box, "se"), [20, 0]);
  assert.deepEqual(moveTo(box, "sw", 5, 5), [5, 5]);
  assert.deepEqual(moveTo(box, "c", 10, 5), null);
});

test("resize about the anchor, with and without lock", () => {
  assert.deepEqual(resizeTo(box, "sw", 40, null, true), { scale: [2, 2], about: [0, 0] });
  assert.deepEqual(resizeTo(box, "c", null, 20, false), { scale: [1, 2], about: [10, 5] });
  assert.equal(resizeTo(box, "c", 20, null, true), null, "unchanged");
  assert.equal(resizeTo(box, "c", 0, null, true), null, "invalid");
  const line = { x0: 5, y0: 0, x1: 5, y1: 10 };
  assert.deepEqual(resizeTo(line, "c", null, 20, true), { scale: [2, 2], about: [5, 5] }, "a vertical line scales by its height");
  assert.deepEqual(scalePercent(box, "c", 50, null, true), { scale: [0.5, 0.5], about: [10, 5] });
  assert.deepEqual(scalePercent(box, "c", null, 150, false), { scale: [1, 1.5], about: [10, 5] });
});

test("rotation: absolute for one object, relative for several", () => {
  assert.equal(rotationDelta(30, 45, true), 15);
  assert.equal(rotationDelta(170, -170, true), 20);
  assert.equal(rotationDelta(0, 90, false), 90);
  assert.equal(rotationDelta(10, 10, true), null);
});

test("layer reorder: drop position and up/down steps give layer.move's to_index", () => {
  // Rows A B C D; drag B (1).
  assert.equal(dropIndex(1, 0, false, 4), 0, "before A");
  assert.equal(dropIndex(1, 0, true, 4), null, "after A is where it is");
  assert.equal(dropIndex(1, 1, false, 4), null, "onto itself");
  assert.equal(dropIndex(1, 2, false, 4), null, "before C is where it is");
  assert.equal(dropIndex(1, 2, true, 4), 2, "after C");
  assert.equal(dropIndex(1, 3, true, 4), 3, "after D: last");
  assert.equal(dropIndex(3, 0, false, 4), 0, "last to first");
  assert.equal(dropIndex(3, 1, true, 4), 2);
  assert.equal(dropIndex(0, 3, false, 4), 2, "before D");
  assert.equal(dropIndex(5, 0, false, 4), null, "unknown row");
  assert.equal(dropIndex(0, 4, false, 4), null);
  assert.equal(stepIndex(0, -1, 3), null);
  assert.equal(stepIndex(0, 1, 3), 1);
  assert.equal(stepIndex(2, 1, 3), null);
  assert.equal(stepIndex(2, -1, 3), 1);
});

test("one 9-point anchor shared by the numeric toolbar and Properties", () => {
  assert.equal(parseAnchor("ne"), "ne");
  assert.equal(parseAnchor("middle"), "c");
  assert.equal(parseAnchor(null), "c");
  const seen: string[] = [];
  const off = subscribeAnchor(() => seen.push(getAnchor()));
  const before = getAnchor();
  setAnchor("sw");
  setAnchor("sw");
  assert.equal(getAnchor(), "sw");
  assert.deepEqual(seen, before === "sw" ? [] : ["sw"], "every user is told once per change");
  off();
  setAnchor(before);
});

test("readout lengths follow the display unit", () => {
  assert.equal(fixedLength(12.345, "mm"), "12.35 mm");
  assert.equal(fixedLength(25.4, "in"), "1.000 in");
  assert.equal(fixedNumber(-0.001, "mm"), "0.00", "no -0");
  assert.equal(lengthText(512.34, "mm"), "512.3 mm");
  assert.equal(lengthText(512.34, "in", 0), "20.2 in");
});
