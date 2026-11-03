import assert from "node:assert/strict";
import { test } from "node:test";
import type { DocObject, GDocument } from "../api/types.ts";
import { cmdsBounds, geometryCmds, polygonCmds, polygonCorners } from "../geometry.ts";
import {
  arrayCells,
  arrayParams,
  arrayProblem,
  artImportFormat,
  cellsOutside,
  classifyClipboard,
  clipMarkerOf,
  docGuides,
  fragmentOf,
  fragmentPieces,
  guidesToMigrate,
  handlesUnchanged,
  identityHandles,
  libraryNameProblem,
  moveHandle,
  nearestCandidate,
  pagePositionDelta,
  parseCandidates,
  pasteInvocations,
  perspectiveProblem,
  selectionSvg,
  shapeParams,
  shapePatchProblem,
  svgIn,
  symmetricHandle,
  warpMapper,
  warpParams,
  warpPreview,
  type ArrayOptions,
} from "./model.ts";

const box = { x0: 0, y0: 0, x1: 10, y1: 5 };
const near = (a: number[], b: number[], eps = 1e-9) => a.every((v, i) => Math.abs(v - b[i]) < eps);

function obj(id: string, geometry: DocObject["geometry"], t: DocObject["transform"] = [1, 0, 0, 1, 0, 0], extra: Partial<DocObject> = {}): DocObject {
  return { id, name: id, layer: "l1", geometry, transform: t, ...extra };
}
function doc(objects: DocObject[], extra: Partial<GDocument> = {}): GDocument {
  return {
    revision: 1,
    name: "t",
    stock: null,
    layers: [
      { id: "l1", name: "C00", color: "#000000", visible: true, output: true, settings: {} as never, sub_layers: [] },
      { id: "l2", name: "C01", color: "#0000ff", visible: true, output: true, settings: {} as never, sub_layers: [] },
    ],
    objects,
    next_id: 9,
    ...extra,
  };
}

test("polygon outline matches the engine: corners up, star alternates, rounded corners only where they fit", () => {
  const hex = polygonCorners(6, 10);
  assert.equal(hex.length, 6);
  assert.ok(near(hex[0], [0, 10]));
  const star = polygonCorners(5, 10, 0.5);
  assert.equal(star.length, 10);
  assert.ok(Math.abs(Math.hypot(...star[1]) - 5) < 1e-9);
  assert.deepEqual(polygonCorners(2, 10), []);
  assert.deepEqual(polygonCorners(6, 10, 1.5), []);
  const sharp = polygonCmds(6, 10);
  assert.equal(sharp.filter((c) => c.c === "L").length, 6);
  assert.equal(sharp[sharp.length - 1].c, "Z");
  // A hexagon's side is 10 mm; a 1 mm radius fits every corner (6 arcs), a huge one fits none.
  const round = polygonCmds(6, 10, null, 1);
  assert.equal(round.filter((c) => c.c === "C").length, 6);
  const b = cmdsBounds(round)!;
  assert.ok(b.y1 < 10 && b.y1 > 9.5, `rounded top ${b.y1}`);
  assert.equal(polygonCmds(6, 10, null, 1000).filter((c) => c.c === "C").length, 0);
  const g = geometryCmds({ kind: "polygon", sides: 4, radius_mm: 5, star_ratio: null, corner_radius_mm: 0 });
  assert.equal(g.length, 6);
});

test("warp maps: identity handles change nothing, corners land on handles, perspective refuses folds", () => {
  for (const kind of ["perspective", "envelope", "grid"] as const) {
    const h = identityHandles(box, kind);
    const m = warpMapper(box, kind, h)!;
    assert.ok(near(m([3, 2]), [3, 2], 1e-9), kind);
    assert.ok(handlesUnchanged(h, identityHandles(box, kind)));
  }
  const q: [number, number][] = [
    [0, 0],
    [12, 0],
    [9, 6],
    [1, 5],
  ];
  const persp = warpMapper(box, "perspective", q)!;
  assert.ok(near(persp([10, 5]), [9, 6]));
  assert.ok(near(persp([0, 5]), [1, 5]));
  const env = warpMapper(box, "envelope", q)!;
  assert.ok(near(env([5, 0]), [6, 0]));
  assert.equal(perspectiveProblem(q), null);
  assert.ok(perspectiveProblem([[0, 0], [10, 0], [0, 5], [10, 5]]));
  assert.equal(warpMapper(box, "perspective", [[0, 0], [10, 0], [0, 5], [10, 5]]), null);
  assert.equal(warpMapper({ x0: 0, y0: 0, x1: 0, y1: 5 }, "envelope", q), null);
});

test("warp symmetry moves the mirrored handle the mirrored way", () => {
  assert.equal(symmetricHandle("perspective", 0, "h"), 1);
  assert.equal(symmetricHandle("perspective", 0, "v"), 3);
  assert.equal(symmetricHandle("grid", 5, "h"), 6);
  assert.equal(symmetricHandle("grid", 5, "v"), 9);
  const base = identityHandles(box, "perspective");
  const moved = moveHandle(base, base, "perspective", 2, [12, 7], { h: true, v: false });
  assert.ok(near(moved[2], [12, 7]));
  assert.ok(near(moved[3], [-2, 7]), `mirror ${moved[3]}`);
  assert.ok(near(moved[0], [0, 0]));
  const both = moveHandle(base, base, "perspective", 2, [12, 7], { h: true, v: true });
  assert.ok(near(both[1], [12, -2]) && near(both[0], [-2, -2]), JSON.stringify(both));
  const gp = warpParams(["a"], "grid", identityHandles(box, "grid"));
  assert.equal((gp.grid as unknown[][]).length, 4);
  assert.equal((gp.grid as unknown[][])[0].length, 4);
  assert.deepEqual(warpParams(["a"], "envelope", base), { ids: ["a"], mode: "envelope", corners: base });
});

test("warp preview bends straight lines with an envelope", () => {
  const o = obj("a", { kind: "rect", width_mm: 10, height_mm: 5, corner_radius_mm: 0 }, [1, 0, 0, 1, 5, 2.5]);
  const d = warpPreview([o], box, "envelope", [
    [0, 0],
    [10, 0],
    [10, 5],
    [0, 8],
  ])!;
  assert.match(d, /^M/);
  assert.ok(d.includes("0,8"), d.slice(0, 200));
});

test("page positions follow object.move_to_page_position", () => {
  const page = { x0: -50, y0: -50, x1: 50, y1: 50 };
  const b = { x0: 0, y0: 0, x1: 10, y1: 4 };
  assert.deepEqual(pagePositionDelta(b, page, "top_left", 2), [-48, 44]);
  assert.deepEqual(pagePositionDelta(b, page, "center", 2), [-5, -2]);
  assert.deepEqual(pagePositionDelta(b, page, "bottom_right", 0), [40, -50]);
});

test("grid array cells match art.array (gap spacing, reverse, half shift, alternate mirrors)", () => {
  const rect = { x0: -2, y0: -1, x1: 2, y1: 1 };
  const o: ArrayOptions = { cols: 3, rows: 2, pitchX: 1, pitchY: 0.5, gapX: true, gapY: true, reverseX: false, reverseY: true, rowShift: "half", colShift: null, mirrorCols: null, mirrorRows: null };
  const cells = arrayCells(o, rect);
  assert.equal(cells.length, 6);
  const at = (r: number, c: number) => cells.find((x) => x.row === r && x.col === c)!.d;
  assert.deepEqual(at(0, 1), [5, 0]);
  assert.deepEqual(at(0, 2), [10, 0]);
  assert.deepEqual(at(1, 0), [2, -2.5]);
  assert.deepEqual(at(1, 2), [12, -2.5]);
  const m = arrayCells({ ...o, rows: 2, cols: 2, mirrorCols: "x", mirrorRows: "x" }, rect);
  assert.deepEqual(
    m.map((c) => c.flipX),
    [false, true, true, false],
  );
  assert.deepEqual(arrayParams(["a"], o), { ids: ["a"], cols: 3, rows: 2, pitch_x_mm: 1, pitch_y_mm: 0.5, spacing_x: "gap", spacing_y: "gap", reverse_y: true, row_shift: "half" });
  assert.equal(arrayProblem(o, rect), null);
  assert.ok(arrayProblem({ ...o, cols: 1, rows: 1 }, rect));
  assert.ok(arrayProblem({ ...o, gapX: false, pitchX: 0 }, rect));
  assert.ok(arrayProblem(o, null));
  assert.equal(cellsOutside({ ...o, pitchX: 100 }, rect, [150, 150]), 4);
});

test("clipboard fragments keep groups and layers and paste piece by piece", () => {
  const d = doc(
    [
      obj("a", { kind: "rect", width_mm: 2, height_mm: 2, corner_radius_mm: 0 }, [1, 0, 0, 1, 0, 0], { group: "g1" }),
      obj("b", { kind: "ellipse", rx_mm: 1, ry_mm: 1 }, [1, 0, 0, 1, 4, 0], { group: "g2", layer: "l2" }),
      obj("c", { kind: "image", width_mm: 2, height_mm: 2, mime: "image/png", data_base64: "AAAA" }, [1, 0, 0, 1, 10, 0]),
    ],
    { groups: [{ id: "g1", name: "outer" }, { id: "g2", name: "inner", parent: "g1" }] },
  );
  const f = fragmentOf(d, ["a", "b", "c"])!;
  assert.equal(f.format, "openetch-art");
  assert.deepEqual(f.groups.map((g) => g.id).sort(), ["g1", "g2"]);
  assert.deepEqual(f.layers.map((l) => l.id).sort(), ["l1", "l2"]);
  const pieces = fragmentPieces(f);
  assert.equal(pieces.length, 2, "the group is one piece, the image another");
  const inv = pasteInvocations(f, [100, 0]);
  assert.equal(inv.length, 2);
  assert.equal(inv[0].id, "art.insert");
  // The whole selection spans x -1..11 (centre 5): offset +95.
  const centers = inv.map((i) => i.params.center as number[]);
  assert.deepEqual(centers[0], [97, 0]);
  assert.deepEqual(centers[1], [105, 0]);
  const inPlace = pasteInvocations(f, null).map((i) => i.params.center);
  assert.deepEqual(inPlace, [[2, 0], [10, 0]]);
  assert.equal(JSON.parse(inv[0].params.content as string).objects.length, 2);
  assert.equal(fragmentOf(d, ["nope"]), null);
  assert.equal(f.object_options, undefined, "no cut properties, no key");
});

test("a copied object takes its hand-placed tabs and start point to the paste, piece by piece", () => {
  const d = doc([
    obj("a", { kind: "rect", width_mm: 2, height_mm: 2, corner_radius_mm: 0 }, [1, 0, 0, 1, 0, 0]),
    obj("b", { kind: "ellipse", rx_mm: 1, ry_mm: 1 }, [1, 0, 0, 1, 4, 0]),
  ]);
  const opts = { a: { tabs: [[1, 0]], start: { node: 0, local: [-1, -1] } }, gone: { priority: 1 } };
  const f = fragmentOf({ ...d, object_options: opts } as typeof d, ["a", "b"])!;
  assert.deepEqual(f.object_options, { a: opts.a }, "only the copied objects' properties");
  const contents = pasteInvocations(f, null).map((i) => JSON.parse(i.params.content as string));
  assert.deepEqual(
    contents.map((c) => c.object_options ?? null),
    [{ a: opts.a }, null],
  );
});

test("the selection as SVG: mm size, flipped y, layer colours, our marker", () => {
  const d = doc([obj("b", { kind: "ellipse", rx_mm: 5, ry_mm: 5 }, [1, 0, 0, 1, 10, 20], { layer: "l2", name: "A & B" })]);
  const svg = selectionSvg(d, ["b"], "abc123")!;
  assert.match(svg, /width="10mm" height="10mm"/);
  assert.match(svg, /viewBox="5 -25 10 10"/);
  assert.match(svg, /stroke="#0000ff"/);
  assert.match(svg, /<title>A &amp; B<\/title>/);
  assert.equal(clipMarkerOf(svg), "abc123");
  assert.equal(selectionSvg(d, [], null), null);
  const img = selectionSvg(doc([obj("c", { kind: "image", width_mm: 4, height_mm: 2, mime: "image/png", data_base64: "QUJD" })]), ["c"], null)!;
  assert.match(img, /<image [^>]*scale\(1 -1\)[^>]*href="data:image\/png;base64,QUJD"/);
});

test("system clipboard content is recognised in LightBurn's order", () => {
  assert.deepEqual(classifyClipboard({ text: "<!-- openetch-clip:xyz -->\n<svg></svg>" }), { kind: "internal", marker: "xyz" });
  const inkscape = '<?xml version="1.0"?>\n<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0L1 1"/></svg>';
  const c = classifyClipboard({ text: inkscape });
  assert.equal(c.kind, "svg");
  assert.equal(svgIn("junk <svg width='1'></svg> tail"), "<svg width='1'></svg>");
  assert.equal(svgIn("<p>no</p>"), null);
  assert.equal(classifyClipboard({ html: "<html><body><svg viewBox='0 0 1 1'><rect/></svg></body></html>" }).kind, "svg");
  assert.equal(classifyClipboard({ files: [{ type: "image/png", name: "x.png" }], text: "" }).kind, "image");
  assert.deepEqual(classifyClipboard({ text: "  Hello\r\nWorld " }), { kind: "text", text: "Hello\nWorld" });
  assert.deepEqual(classifyClipboard({}), { kind: "none" });
});

test("guides come from the document; browser guides migrate once without duplicates", () => {
  const d = doc([], { guides: [{ id: "guide-3", axis: "x", position_mm: 12.5 }, { id: "bad", axis: "z" as "x", position_mm: 1 }] });
  assert.deepEqual(docGuides(d), [{ id: "guide-3", axis: "x", v: 12.5 }]);
  assert.deepEqual(docGuides(null), []);
  const browser = [
    { id: "g1", axis: "x" as const, v: 12.5 },
    { id: "g2", axis: "y" as const, v: -3 },
    { id: "g3", axis: "y" as const, v: -3 },
  ];
  assert.deepEqual(guidesToMigrate(browser, docGuides(d)), [{ id: "g2", axis: "y", v: -3 }]);
});

test("Art Library formats and names", () => {
  assert.deepEqual(artImportFormat("logo.SVG"), { format: "svg" });
  assert.deepEqual(artImportFormat("a.lbrn2"), { format: "lbrn2" });
  assert.deepEqual(artImportFormat("p.openetch"), { format: "openetch" });
  assert.deepEqual(artImportFormat("photo.jpg"), { format: "image" });
  assert.deepEqual(artImportFormat("clip.lbart"), { format: "lbart" });
  assert.ok("error" in artImportFormat("readme.txt"));
  assert.equal(libraryNameProblem("Logos", ["Other"]), null);
  assert.ok(libraryNameProblem("logos", ["Logos"]));
  assert.equal(libraryNameProblem("logos", ["Logos"], "Logos"), null);
  assert.ok(libraryNameProblem("a/b", []));
  assert.ok(libraryNameProblem("  ", []));
});

test("tangent candidates: parsed and picked by the outline nearest the click", () => {
  const c = parseCandidates({ candidates: [{ index: 0, center: [0, 0], radius_mm: 5 }, { index: 1, center: [0, 0], radius_mm: 8 }, { index: 2, center: [1], radius_mm: 1 }] });
  assert.equal(c.length, 2);
  assert.equal(nearestCandidate(c, [7.6, 0], 1)?.index, 1);
  assert.equal(nearestCandidate(c, [6.5, 0], 1), null);
  assert.deepEqual(parseCandidates(null), []);
});

test("Shape Properties parameters and the engine's limits", () => {
  const r = shapeParams({ kind: "rect", width_mm: 10, height_mm: 4, corner_radius_mm: 0 })!;
  assert.equal(shapePatchProblem(r, { corner_radius_mm: 2 }), null);
  assert.ok(shapePatchProblem(r, { corner_radius_mm: 2.5 }));
  assert.ok(shapePatchProblem(r, { width_mm: 0 }));
  const pg = shapeParams({ kind: "polygon", sides: 6, radius_mm: 5, star_ratio: null, corner_radius_mm: 0 })!;
  assert.deepEqual(pg, { kind: "polygon", sides: 6, radius_mm: 5, star_ratio: null, corner_radius_mm: 0, ry_mm: null });
  assert.ok(shapePatchProblem(pg, { sides: 2 }));
  assert.ok(shapePatchProblem(pg, { sides: 3.5 }));
  assert.equal(shapePatchProblem(pg, { star_ratio: null }), null);
  assert.ok(shapePatchProblem(pg, { star_ratio: 1 }));
  assert.equal(shapeParams({ kind: "path", path: "M0 0L1 1" }), null);
});

test("grid array extras: total size counts, seed, auto-increment, results, virtual arrays and Edit Array", async () => {
  const m = await import("./model.ts");
  const rect = { x0: 0, y0: 0, x1: 10, y1: 4 };
  const o: ArrayOptions = { cols: 1, rows: 1, pitchX: 2, pitchY: 1, gapX: true, gapY: true, reverseX: false, reverseY: false, rowShift: null, colShift: null, mirrorCols: null, mirrorRows: null };
  // 50 mm wide: 10 + n * 12 <= 50 -> 4 columns; 20 mm high: 4 + n * 5 <= 20 -> 4 rows.
  const r = m.resolveCounts({ ...o, totalWidth: 50, totalHeight: 20 }, rect)!;
  assert.deepEqual([r.cols, r.rows], [4, 4]);
  // A half-row shift (5 mm) counts once there are two rows.
  assert.deepEqual(m.resolveCounts({ ...o, rows: 2, rowShift: "half", totalWidth: 50 }, rect)!.cols, 3);
  assert.equal(m.resolveCounts({ ...o, totalWidth: 5 }, rect), null, "smaller than the selection");
  assert.equal(m.resolveCounts(o, rect), o, "no totals: unchanged");
  assert.equal(m.fitCount(30, 10, 0, 0), 1);
  const p = m.arrayParams(["a"], { ...o, cols: 2, totalWidth: 50, seed: 7, autoIncrement: 1, groupResults: true });
  assert.deepEqual(p, { ids: ["a"], cols: 2, rows: 1, pitch_x_mm: 2, pitch_y_mm: 1, spacing_x: "gap", spacing_y: "gap", total_width_mm: 50, random_orientation_seed: 7, auto_increment: 1, group_results: true });
  assert.equal(m.arrayParams(["a"], { ...o, cols: 2, virtual: true, groupResults: true }).group_results, undefined, "never both");
  assert.equal(m.arrayParams(["a"], { ...o, cols: 2, virtual: true }).virtual, true);
  const e = m.editArrayParams("array-4", { ...o, cols: 3, virtual: true, groupResults: true });
  assert.deepEqual(e, { ids: [], edit_array: "array-4", cols: 3, rows: 1, pitch_x_mm: 2, pitch_y_mm: 1, spacing_x: "gap", spacing_y: "gap" });
  const doc = {
    objects: [{ id: "o1" }, { id: "o2", group: "g1" }, { id: "c1" }],
    virtual_arrays: [{ id: "array-4", sources: ["o1"], clones: ["c1"], settings: { rows: 2, cols: 3, pitch_x_mm: 5, pitch_y_mm: 5, spacing_x: "center", spacing_y: "gap", reverse_x: false, reverse_y: true, row_shift: "half", col_shift: 1.5, mirror_alternate_cols: "x", mirror_alternate_rows: null, random_orientation_seed: 3, auto_increment: 2 } }],
  } as unknown as GDocument;
  assert.equal(m.virtualArrayOf(doc, ["c1"])?.id, "array-4");
  assert.equal(m.virtualArrayOf(doc, ["o2"]), null);
  const back = m.arrayOptionsOf(doc.virtual_arrays![0]!.settings);
  assert.deepEqual([back.cols, back.rows, back.gapX, back.gapY, back.reverseY, back.rowShift, back.colShift, back.mirrorCols, back.seed, back.autoIncrement], [3, 2, false, true, true, "half", 1.5, "x", 3, 2]);
});

test("LightBurn polygons: Ry stretches, half_step stands them on an edge (as polygons.lbrn2)", () => {
  // Triangle N=3 Rx=10 Ry=9.5 with LightBurn's y flip: 17.321 x 14.25; hexagon Rx=Ry=5: 10 wide.
  const tri = cmdsBounds(geometryCmds({ kind: "polygon", sides: 3, radius_mm: 10, ry_mm: 9.5, half_step: true }));
  assert.ok(tri && Math.abs(tri.x1 - tri.x0 - 17.3205) < 1e-3 && Math.abs(tri.y1 - tri.y0 - 14.25) < 1e-9);
  const hex = cmdsBounds(geometryCmds({ kind: "polygon", sides: 6, radius_mm: 5, half_step: true }));
  assert.ok(hex && Math.abs(hex.x1 - hex.x0 - 10) < 1e-9);
  const star = cmdsBounds(geometryCmds({ kind: "polygon", sides: 5, radius_mm: 7.5, star_ratio: 0.4, half_step: true }));
  assert.ok(star && Math.abs(star.x1 - star.x0 - 14.266) < 1e-3 && Math.abs(star.y1 - star.y0 - 13.568) < 1e-3);
});
