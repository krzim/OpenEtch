// Unit tests for text/textModel.ts: font lists and the missing-font check, stored defaults, command builders (text,
// variable text, barcodes), Apply Path to Text selection, and the editor's placement.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { DocObject, GDocument, TextGeometry } from "../api/types.ts";
import { geometryCmds } from "../geometry.ts";
import { shapeKindName } from "../snapping.ts";
import {
  BARCODE_KEY,
  DEFAULT_TEXT,
  applyPathPair,
  barcodeCreate,
  barcodeParams,
  barcodeRedraw,
  cssFontFamily,
  defaultsFrom,
  defaultsToFields,
  editorTranslate,
  fontGroups,
  fontMissing,
  normalizeText,
  parseFontList,
  parseRowPreview,
  parseTextDefaults,
  previewRow,
  rotationOf,
  scaleOf,
  serialProblem,
  shapeText,
  textMode,
  textUpdate,
  usesVariables,
  variablesOf,
  DEFAULT_VARIABLES,
} from "./textModel.ts";

const tg = (over: Partial<TextGeometry> = {}): TextGeometry => ({
  kind: "text",
  text: "Hello",
  font: { family: "Arial", kind: "system" },
  height_mm: 5,
  h_spacing: 0,
  v_spacing: 0,
  align_h: "middle",
  align_v: "middle",
  bold: false,
  italic: false,
  upper: false,
  welded: true,
  bend_deg: 0,
  distort: false,
  max_width_mm: 0,
  squeeze: false,
  ignore_empty_vars: false,
  outline: [],
  ...over,
});

const obj = (id: string, geometry: DocObject["geometry"], extras?: Record<string, string>): DocObject => ({
  id,
  name: id,
  layer: "L1",
  geometry,
  transform: [1, 0, 0, 1, 0, 0],
  extras,
});

const doc = (objects: DocObject[], over: Partial<GDocument> = {}): GDocument => ({
  revision: 1,
  name: "d",
  stock: null,
  layers: [],
  objects,
  next_id: 9,
  ...over,
});

const FONTS = parseFontList([
  { family: "Arial", kind: "system", bold: true, italic: true },
  { family: "Hershey Sans", kind: "stroke", bold: false, italic: false },
  { family: "romans", kind: "shx" },
  { family: "Bahnschrift", kind: "system", bold: false, italic: false },
  { family: 3, kind: "system" },
  { family: "X", kind: "woff" },
]);

test("font list: checked, grouped by kind, searched", () => {
  assert.equal(FONTS.length, 4);
  const g = fontGroups(FONTS, "");
  assert.deepEqual(
    g.map((x) => [x.kind, x.fonts.map((f) => f.family)]),
    [
      ["system", ["Arial", "Bahnschrift"]],
      ["stroke", ["Hershey Sans"]],
      ["shx", ["romans"]],
    ],
  );
  assert.deepEqual(
    fontGroups(FONTS, "  SANS ").map((x) => x.fonts.map((f) => f.family)),
    [["Hershey Sans"]],
  );
  assert.deepEqual(fontGroups(FONTS, "zzz"), []);
});

test("missing font: by family (any case) and kind; unknown while the list is not loaded", () => {
  assert.equal(fontMissing({ family: "arial", kind: "system" }, FONTS), false);
  assert.equal(fontMissing({ family: "Arial", kind: "stroke" }, FONTS), true);
  assert.equal(fontMissing({ family: "Comic Sans MS", kind: "system" }, FONTS), true);
  assert.equal(fontMissing({ family: "Comic Sans MS", kind: "system" }, null), false);
});

test("css font family quotes installed fonts only", () => {
  assert.equal(cssFontFamily({ family: 'Ar"ial', kind: "system" }), '"Arial", sans-serif');
  assert.match(cssFontFamily({ family: "Hershey Sans", kind: "stroke" }), /^ui-sans-serif/);
});

test("stored defaults are checked field by field", () => {
  assert.deepEqual(parseTextDefaults(null), DEFAULT_TEXT);
  const d = parseTextDefaults({ font: { family: "Arial", kind: "system" }, height_mm: 8, align_h: "left", align_v: "sideways", bold: true, h_spacing: 5000, welded: "yes" });
  assert.deepEqual(d.font, { family: "Arial", kind: "system" });
  assert.equal(d.height_mm, 8);
  assert.equal(d.align_h, "left");
  assert.equal(d.align_v, "middle");
  assert.equal(d.bold, true);
  assert.equal(d.h_spacing, 0);
  assert.equal(d.welded, true);
  assert.equal(parseTextDefaults({ height_mm: 0 }).height_mm, 5);
  assert.equal(parseTextDefaults({ font: { family: " ", kind: "system" } }).font, null);
});

test("shape.text carries the defaults, the rounded point and the layer", () => {
  const inv = shapeText("A\nB", [1.234567, -2], { ...DEFAULT_TEXT, font: { family: "Arial", kind: "system" } }, "L2");
  assert.equal(inv.id, "shape.text");
  assert.deepEqual(inv.params, {
    text: "A\nB",
    center: [1.2346, -2],
    height_mm: 5,
    h_spacing: 0,
    v_spacing: 0,
    align_h: "middle",
    align_v: "middle",
    bold: false,
    italic: false,
    upper: false,
    welded: true,
    font: { family: "Arial", kind: "system" },
    layer: "L2",
  });
  // Without a chosen font the engine picks one (Arial, else Hershey Sans).
  assert.equal("font" in (shapeText("A", [0, 0], DEFAULT_TEXT, null).params as object), false);
});

test("text.update and defaults from a text", () => {
  assert.deepEqual(textUpdate("obj-2", { bold: true, variable: { mode: "serial", offset: 2 } }), {
    id: "text.update",
    params: { id: "obj-2", bold: true, variable: { mode: "serial", offset: 2 } },
  });
  const g = tg({ height_mm: 7, italic: true });
  const d = defaultsFrom(g);
  assert.equal(d.height_mm, 7);
  assert.equal(d.italic, true);
  assert.deepEqual(defaultsToFields({ font: null, height_mm: 3, upper: undefined }), { height_mm: 3 });
  assert.equal(textMode(g), "normal");
  assert.equal(textMode(tg({ variable: { mode: "csv", offset: 0 } })), "csv");
});

test("variables: defaults, validation, preview answers", () => {
  assert.deepEqual(variablesOf(null), DEFAULT_VARIABLES);
  assert.equal(variablesOf(doc([], { variables: { start: 5, end: 9, current: 6, advance_by: 2, auto_advance: true } })).current, 6);
  const v = variablesOf(null);
  assert.equal(serialProblem(v, { start: 10, end: 3 }), "Start (10) is after End (3)");
  assert.equal(serialProblem(v, { advance_by: 1.5 }), "advance by must be a whole number");
  assert.equal(serialProblem(v, { current: 4 }), null);
  assert.deepEqual(previewRow(), { id: "var.preview_row", params: {} });
  assert.deepEqual(previewRow(3), { id: "var.preview_row", params: { value: 3 } });
  assert.deepEqual(parseRowPreview({ value: 3, items: [{ id: "a", shown: "SN-0003" }, { id: 1 }] }), { value: 3, items: [{ id: "a", shown: "SN-0003" }] });
  assert.equal(parseRowPreview("x"), null);
});

test("barcodes: kept parameters, create and redraw send only what shape.barcode takes", () => {
  const kept = '{"kind":"qr","data":"abc","module_mm":null,"height_mm":null,"quiet_zone":null,"invert":false,"error_correction":null,"force_square":null,"show_text":false,"variable":null,"center":[-20.0,0.0],"rotation_deg":0.0,"layer":"L1","name":null}';
  const o = obj("obj-3", { kind: "path", path: [] }, { [BARCODE_KEY]: kept });
  const p = barcodeParams(o);
  assert.ok(p);
  assert.equal(p.kind, "qr");
  assert.equal(barcodeParams(obj("x", { kind: "path", path: [] }, { [BARCODE_KEY]: "{oops" })), null);
  assert.equal(barcodeParams(obj("x", { kind: "path", path: [] })), null);
  // Redraw keeps the object's layer, name and placement: none of those are sent.
  assert.deepEqual(barcodeRedraw("obj-3", { ...p, module_mm: 0.3, variable: { mode: "normal" } }), {
    id: "shape.barcode",
    params: { kind: "qr", data: "abc", module_mm: 0.3, invert: false, show_text: false, id: "obj-3" },
  });
  assert.deepEqual(barcodeCreate({ kind: "code128", data: "12", show_text: true, variable: { mode: "serial", offset: 1 } }, [1, 2], "L3"), {
    id: "shape.barcode",
    params: { kind: "code128", data: "12", show_text: true, variable: { mode: "serial", offset: 1 }, center: [1, 2], layer: "L3" },
  });
});

test("uses variables: variable text or a variable barcode", () => {
  assert.equal(usesVariables(doc([obj("t", tg())])), false);
  assert.equal(usesVariables(doc([obj("t", tg({ variable: { mode: "date_time", offset: 0 } }))])), true);
  const b = obj("b", { kind: "path", path: [] }, { [BARCODE_KEY]: '{"kind":"qr","data":"d","variable":{"mode":"serial","offset":0}}' });
  assert.equal(usesVariables(doc([b])), true);
});

test("apply path pair: one text and one vector shape", () => {
  const d = doc([obj("t", tg()), obj("p", { kind: "ellipse", rx_mm: 5, ry_mm: 5 }), obj("i", { kind: "image", width_mm: 1, height_mm: 1, mime: "image/png", data_base64: "" }), obj("t2", tg())]);
  assert.deepEqual(applyPathPair(d, ["p", "t"]), { text: "t", path: "p" });
  assert.equal(applyPathPair(d, ["t", "i"]), null);
  assert.equal(applyPathPair(d, ["t", "t2"]), null);
  assert.equal(applyPathPair(d, ["t"]), null);
  assert.equal(applyPathPair(d, ["t", "zz"]), null);
});

test("editor placement and transform helpers", () => {
  assert.deepEqual(editorTranslate("left", "top"), [0, 0]);
  assert.deepEqual(editorTranslate("middle", "middle"), [-50, -50]);
  assert.deepEqual(editorTranslate("right", "bottom"), [-100, -100]);
  const o = obj("t", tg());
  o.transform = [0, 2, -2, 0, 5, 6];
  assert.equal(Math.round(rotationOf(o)), 90);
  assert.equal(scaleOf(o), 2);
  assert.equal(normalizeText("a\r\nb\rc"), "a\nb\nc");
});

test("text draws its outline; text and barcodes are named in Properties", () => {
  const g = tg({ outline: [{ MoveTo: { x: 0, y: 0 } }, { LineTo: { x: 1, y: 0 } }, "ClosePath"] });
  assert.equal(geometryCmds(g).length, 3);
  assert.equal(shapeKindName(obj("t", g)), "Text");
  assert.equal(shapeKindName(obj("b", { kind: "path", path: [] }, { [BARCODE_KEY]: "{}" })), "Barcode");
  assert.equal(shapeKindName(obj("p", { kind: "path", path: [] })), "Path");
});

test("barcode kinds follow the engine's shape.barcode schema", async () => {
  const m = await import("./textModel.ts");
  const today = { properties: { kind: { type: "string", description: "qr, data_matrix, aztec, pdf417, code128, code39, code93, codabar, ean8, ean13, upc_a, upc_e, itf, telepen." } } };
  assert.deepEqual(m.barcodeKindsFrom(today).map((k) => k.id), m.BARCODE_KINDS.map((k) => k.id));
  const later = { properties: { kind: { type: "string", description: "qr, micro_qr, data_matrix, code128, plessey, apriltag_36h11, zz_new: see the docs" } } };
  const k = m.barcodeKindsFrom(later);
  assert.deepEqual(k.map((x) => x.id), ["qr", "data_matrix", "code128", "micro_qr", "plessey", "apriltag_36h11", "zz_new"]);
  assert.deepEqual(k.find((x) => x.id === "micro_qr"), { id: "micro_qr", label: "QR Micro", twoD: true });
  assert.equal(k.find((x) => x.id === "plessey")?.twoD, false);
  assert.equal(k.find((x) => x.id === "zz_new")?.label, "zz_new");
  assert.deepEqual(m.barcodeKindsFrom({ properties: { kind: { enum: ["qr", "vin"] } } }).map((x) => x.label), ["QR Code", "VIN"]);
  assert.equal(m.barcodeKindsFrom(null), m.BARCODE_KINDS, "no schema: the built-in list");
  const ec = m.barcodeKindsFrom({ properties: { kind: { enum: ["qr", "han_xin", "grid_matrix", "vin"] } } });
  assert.deepEqual(ec.find((x) => x.id === "han_xin")?.ec?.map((e) => e[0]), ["1", "2", "3", "4"]);
  assert.deepEqual(ec.find((x) => x.id === "grid_matrix")?.ec?.map((e) => e[0]), ["1", "2", "3", "4", "5"]);
  assert.equal(ec.find((x) => x.id === "vin")?.ec, undefined);
  m.setEngineBarcodeKinds(k);
  assert.equal(m.barcodeKind("plessey")?.label, "Plessey");
  m.setEngineBarcodeKinds(null);
});
