// Unit tests for api/drop.ts: routing dropped/picked files by extension, refusals, the size limit, several files in
// one drop, the overlay counter and label, base64 and the summary toast.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  IMPORT_ACCEPT,
  bytesToBase64,
  carriesFiles,
  classifyFile,
  createdIds,
  dragDepth,
  dropSummary,
  fileExt,
  importName,
  inlineLimit,
  overlayLabel,
  planIsEmpty,
  routeDroppedFiles,
} from "./drop.ts";

const f = (name: string, size = 100) => ({ name, size });
const all = { maxBytes: 1000, has: () => true };

test("classify by extension, case-insensitively", () => {
  assert.equal(classifyFile("a.svg"), "svg");
  assert.equal(classifyFile("A.SVG"), "svg");
  assert.equal(classifyFile("jig.DXF"), "dxf");
  assert.equal(classifyFile("photo.JPG"), "image");
  assert.equal(classifyFile("photo.jpeg"), "image");
  assert.equal(classifyFile("p.png"), "image");
  assert.equal(classifyFile("x.LBRN2"), "lbrn2");
  assert.equal(classifyFile("x.lbrn"), "lbrn-old");
  assert.equal(classifyFile("coin.lbt"), "lbrn-old");
  assert.equal(classifyFile("x.openetch"), "openetch");
  assert.equal(classifyFile("logo.ai"), "engine");
  assert.equal(classifyFile("logo.pdf"), "engine");
  assert.equal(classifyFile("part.STL"), "engine");
  assert.equal(classifyFile("path.gcode"), "not-yet");
  assert.equal(classifyFile("readme.txt"), "unknown");
  assert.equal(classifyFile("noext"), "unknown");
  assert.equal(classifyFile(".svg"), "unknown");
  assert.equal(fileExt("C:\\dir.v2\\name"), "");
  assert.equal(fileExt("a.b.SVG"), "svg");
});

test("each format goes to its own path", () => {
  const p = routeDroppedFiles([f("a.svg"), f("b.dxf"), f("c.png"), f("d.jpg")], all);
  assert.deepEqual(
    p.imports.map((i) => [i.file.name, i.kind, i.command, i.mime]),
    [
      ["a.svg", "svg", "art.import_svg", undefined],
      ["b.dxf", "dxf", "art.import_dxf", undefined],
      ["c.png", "image", "art.import_image", "image/png"],
      ["d.jpg", "image", "art.import_image", "image/jpeg"],
    ],
  );
  assert.equal(p.refused.length, 0);
  assert.deepEqual(routeDroppedFiles([f("p.lbrn2")], all).lbrn2.map((x) => x.name), ["p.lbrn2"]);
  const o = routeDroppedFiles([f("p.OPENETCH")], all);
  assert.equal(o.open?.name, "p.OPENETCH");
  assert.equal(planIsEmpty(o), false);
});

test("a project file in a drop of several is refused; LightBurn files are all kept", () => {
  const p = routeDroppedFiles([f("a.svg"), f("p.openetch"), f("x.lbrn2"), f("y.lbrn2")], all);
  assert.equal(p.open, null);
  assert.deepEqual(p.refused, [{ name: "p.openetch", reason: "drop a project file on its own to open it" }]);
  assert.deepEqual(p.lbrn2.map((x) => x.name), ["x.lbrn2", "y.lbrn2"]);
  assert.deepEqual(p.imports.map((x) => x.file.name), ["a.svg"]);
});

test("unknown, old LightBurn and engine-only formats are refused with a reason (older engines)", () => {
  const p = routeDroppedFiles([f("r.txt"), f("c.lbt"), f("l.ai"), f("t.TIFF"), f("noext"), f("p.gcode")], all);
  assert.ok(planIsEmpty(p));
  assert.deepEqual(
    p.refused.map((r) => r.reason),
    [
      ".txt files cannot be imported",
      "LightBurn .lbrn/.lbt files need a newer OpenEtch engine; save as .lbrn2",
      "Adobe Illustrator (.ai) import needs a newer OpenEtch engine",
      "TIFF (.tiff) import needs a newer OpenEtch engine",
      "not a file type OpenEtch can import",
      "G-code (.gcode) import is not supported yet",
    ],
  );
});

test("files over the size limit and empty files are refused before reading", () => {
  const p = routeDroppedFiles([f("big.svg", 1001), f("ok.svg", 1000), f("big.lbrn2", 5000), f("e.svg", 0)], all);
  assert.deepEqual(p.imports.map((x) => x.file.name), ["ok.svg"]);
  assert.deepEqual(
    p.refused.map((r) => r.name),
    ["big.svg", "big.lbrn2", "e.svg"],
  );
  assert.match(p.refused[0].reason, /larger than/);
});

test("an engine without an importer refuses that format", () => {
  const p = routeDroppedFiles([f("a.dxf"), f("b.png"), f("c.svg")], { maxBytes: 1000, has: (c) => c === "art.import_svg" });
  assert.deepEqual(p.imports.map((x) => x.file.name), ["c.svg"]);
  assert.deepEqual(
    p.refused.map((r) => r.reason),
    ["DXF import is not supported by this engine yet", "image import is not supported by this engine yet"],
  );
});

test("blocked drops refuse every file", () => {
  const p = routeDroppedFiles([f("a.svg"), f("b.openetch")], { ...all, blocked: "Finish the open dialog first" });
  assert.ok(planIsEmpty(p));
  assert.equal(p.refused.length, 2);
  assert.equal(p.refused[1].reason, "Finish the open dialog first");
});

test("overlay counter and label", () => {
  let d = 0;
  d = dragDepth(d, "enter");
  d = dragDepth(d, "enter");
  d = dragDepth(d, "leave");
  assert.equal(d, 1);
  d = dragDepth(d, "leave");
  d = dragDepth(d, "leave");
  assert.equal(d, 0);
  assert.equal(dragDepth(5, "drop"), 0);
  assert.equal(overlayLabel([{ type: "image/png" }]), "Drop to import");
  assert.equal(overlayLabel([{ type: "" }]), "Drop to open or import");
  assert.equal(overlayLabel([{ type: "" }, { type: "" }]), "Drop to import 2 files");
  assert.ok(carriesFiles(["text/plain", "Files"]));
  assert.ok(!carriesFiles(["text/plain", "text/uri-list"]));
  assert.ok(!carriesFiles(null));
});

test("helpers: created ids, base64, names, accept list", () => {
  assert.deepEqual(createdIds({ created: ["obj-2", 3, "obj-4"] }), ["obj-2", "obj-4"]);
  assert.deepEqual(createdIds(null), []);
  assert.equal(bytesToBase64(new Uint8Array([104, 105])), "aGk=");
  const big = new Uint8Array(100_000).fill(65);
  assert.equal(bytesToBase64(big), Buffer.from(big).toString("base64"));
  assert.equal(importName("C:\\x\\Logo.final.svg"), "Logo.final");
  assert.ok(inlineLimit("image") * (4 / 3) < 32 * 1024 * 1024, "base64 bitmaps fit the engine's JSON body limit");
  assert.ok(inlineLimit("svg") < 32 * 1024 * 1024);
  for (const e of [".svg", ".dxf", ".png", ".jpg", ".lbrn2", ".openetch"]) assert.ok(IMPORT_ACCEPT.split(",").includes(e));
});

test("summary toast", () => {
  assert.deepEqual(dropSummary({ imported: ["a.svg"], objects: 1, refused: [] }), { msg: "Imported a.svg.", kind: "info" });
  assert.equal(dropSummary({ imported: ["a.svg"], objects: 3, refused: [] }).msg, "Imported a.svg (3 objects).");
  const s = dropSummary({ imported: ["a.svg", "b.dxf"], objects: 4, refused: [{ name: "c.ai", reason: "nope" }] });
  assert.equal(s.msg, "Imported 2 files (4 objects). c.ai: nope.");
  assert.equal(s.kind, "info");
  assert.deepEqual(dropSummary({ imported: [], objects: 0, refused: [] }), { msg: "Nothing imported.", kind: "error" });
  assert.equal(dropSummary({ imported: [], objects: 0, refused: [{ name: "x", reason: "y" }, { name: "z", reason: "w" }] }).kind, "error");
  assert.equal(dropSummary({ imported: [], objects: 0, refused: [], pending: 2 }).kind, "info");
});
