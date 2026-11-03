import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument } from "../api/types.ts";
import { startCtxOf, startLabel, startScope } from "../laser/startFlow.ts";
import { DEFAULT_FACTS } from "../laser/laserContext.ts";
import {
  J,
  gridCentres,
  gridProblem,
  isPlacement,
  keepOutRings,
  objectsInPocket,
  parsePreflight,
  pocketAt,
  pocketBox,
  pocketCenter,
  preflightKey,
  rectRegion,
  regionUsable,
  ringArea,
  simplifyLasso,
  splitFindings,
  type Jig,
} from "./jigModel.ts";
import { getRemarkRegion, setRemarkRegion } from "./regionStore.ts";
import { EN } from "../workspace/locales/en.ts";

const jig: Jig = {
  id: "jig-3",
  name: "Coins",
  datum: [10, 0],
  rotation_deg: 90,
  pockets: [
    { id: "p1", number: 1, center: [5, 0], shape: { kind: "circle", diameter_mm: 4 } },
    { id: "p2", number: 2, center: [15, 0], shape: { kind: "rect", width_mm: 4, height_mm: 2 }, enabled: false },
  ],
  keep_out: [{ id: "k1", name: "Clamp", path: "M0,0L1,0L1,1L0,1Z" }],
};

function doc(extra: Partial<GDocument> = {}): GDocument {
  return { revision: 1, name: "d", stock: null, layers: [], objects: [], next_id: 1, ...extra } as GDocument;
}

test("pockets follow the jig's datum and rotation", () => {
  const c = pocketCenter(jig, jig.pockets[0]);
  assert.ok(Math.abs(c[0] - 10) < 1e-9 && Math.abs(c[1] - 5) < 1e-9, String(c));
  assert.equal(pocketAt([jig], [10, 5])?.pocket.id, "p1");
  // Pocket 2 is turned with the jig: 2 wide, 4 tall around (10, 15).
  assert.equal(pocketAt([jig], [10.9, 16.9])?.pocket.id, "p2");
  assert.equal(pocketAt([jig], [12, 15]), null);
  const rings = keepOutRings(doc({ jigs: [jig] }), [{ name: "Fixture", points: [[0, 0], [1, 0], [0, 1]] }, { name: "Off", points: [[0, 0], [1, 0], [0, 1]], enabled: false }]);
  assert.deepEqual(rings.map((r) => [r.name, r.device]), [["Coins / Clamp", false], ["Fixture", true]]);
  // The clamp at (0..1, 0..1) in the jig frame sits at x 9..10, y 0..1 after the turn.
  const xs = rings[0].ring.map((p) => p[0]);
  assert.ok(Math.min(...xs) > 8.99 && Math.max(...xs) < 10.01, String(xs));
  // The pocket's bounds (where the canvas puts its number, at the top edge): turned with the jig too.
  const b = pocketBox(jig, jig.pockets[1]);
  const near = (a: number, e: number) => Math.abs(a - e) < 1e-6;
  assert.ok(near(b.x0, 9) && near(b.x1, 11) && near(b.y0, 13) && near(b.y1, 17), JSON.stringify(b));
  const cb = pocketBox(jig, jig.pockets[0]);
  assert.ok(Math.abs(cb.y1 - 7) < 0.05 && Math.abs(cb.x0 - 8) < 0.05, JSON.stringify(cb));
});

test("objects in a pocket: the originals in the source pocket, tagged copies elsewhere", () => {
  const d = doc({
    jigs: [jig],
    virtual_arrays: [{ id: "array-9", sources: ["obj-1"], clones: ["obj-2"], settings: { jig: "jig-3", source_pocket: "p1", auto_increment: 1 } }],
    objects: [
      { id: "obj-1", name: "a", layer: "l", geometry: { kind: "rect", width_mm: 1, height_mm: 1, corner_radius_mm: 0 }, transform: [1, 0, 0, 1, 0, 0] },
      { id: "obj-2", name: "a [P2]", layer: "l", geometry: { kind: "rect", width_mm: 1, height_mm: 1, corner_radius_mm: 0 }, transform: [1, 0, 0, 1, 0, 0], extras: { "openetch.jig_pocket": "jig-3/p2" } },
    ],
  });
  assert.deepEqual(objectsInPocket(d, "jig-3", "p1"), ["obj-1"]);
  assert.deepEqual(objectsInPocket(d, "jig-3", "p2"), ["obj-2"]);
  assert.ok(isPlacement(d.virtual_arrays![0]));
  assert.ok(!isPlacement({ settings: { rows: 2 } }));
});

test("invocations are the commands agents call", () => {
  assert.deepEqual(J.fitToStock(["obj-1"], 1, "best"), { id: "object.fit_to_stock", params: { ids: ["obj-1"], margin_mm: 1, rotate: "best" } });
  assert.deepEqual(J.setPockets("jig-1", "all", true), { id: "jig.set_pockets", params: { jig: "jig-1", all: true, enabled: true } });
  assert.deepEqual(J.setPockets("jig-1", [2, 3], false).params, { jig: "jig-1", pockets: [2, 3], enabled: false });
  assert.deepEqual(J.preflight([{ name: "a", points: [[0, 0], [1, 0], [0, 1]], enabled: false }, { name: "b", points: [[0, 0], [1, 0], [0, 1]] }]).params, {
    keep_out: [{ name: "b", points: [[0, 0], [1, 0], [0, 1]] }],
  });
  assert.deepEqual(J.remarkCheck([[0, 0], [1, 0], [0, 1]], []).params, { region: [[0, 0], [1, 0], [0, 1]] });
  assert.deepEqual(J.create({ rows: 1, cols: 2, pitch_x_mm: 30, pitch_y_mm: 0, pocket: { kind: "circle", diameter_mm: 25 } }).params, {
    rows: 1,
    cols: 2,
    pitch_x_mm: 30,
    pitch_y_mm: 0,
    pocket: { kind: "circle", diameter_mm: 25 },
  });
});

test("grid jig checks and preview centres", () => {
  assert.equal(gridProblem(2, 3, 30, 30, [25, 25]), null);
  assert.equal(gridProblem(2, 3, 20, 30, [25, 25]), "jig.errPitch");
  assert.equal(gridProblem(0, 3, 30, 30, [25, 25]), "jig.errCount");
  assert.equal(gridProblem(1, 1, 0, 0, [0, 25]), "jig.errSize");
  // Row 0 (pocket 1) is the far row, +Y.
  assert.deepEqual(gridCentres(2, 2, 10, 10, [0, 0])[0], [-5, 5]);
});

test("regions: rectangles, lassos and usability", () => {
  const r = rectRegion([3, 4], [1, 1]);
  assert.deepEqual(r, [[1, 1], [3, 1], [3, 4], [1, 4]]);
  assert.equal(ringArea(r), 6);
  assert.ok(regionUsable(r));
  assert.ok(!regionUsable([[0, 0], [1, 1], [2, 2]]));
  assert.ok(!regionUsable(null));
  const dense = Array.from({ length: 2000 }, (_, i) => [Math.cos(i / 100) * 10, Math.sin(i / 100) * 10] as [number, number]);
  const s = simplifyLasso(dense, 0.5, 300);
  assert.ok(s.length <= 300 && s.length > 50, String(s.length));
});

test("a re-mark region scopes every Start", () => {
  setRemarkRegion([[0, 0], [1, 0], [1, 1]]);
  const ctx = startCtxOf({ ...DEFAULT_FACTS, machine: { connected: true } as never, link: "open" }, "dock");
  assert.deepEqual(startScope(ctx), { kind: "region" });
  assert.match(startLabel(ctx), /region/);
  // Start Here never carries the region (the engine refuses both together).
  const here = startCtxOf({ ...DEFAULT_FACTS, link: "open" }, "startHere", { fromSegment: 4 });
  assert.equal(here.region, null);
  setRemarkRegion([[0, 0], [1, 1]]);
  assert.equal(getRemarkRegion(), null, "fewer than 3 points clear it");
});

test("pre-flight findings have texts in every case", () => {
  const r = parsePreflight({
    warnings: [
      { check: "keep_out", blocking: true, text: "x", params: { name: "A", layer: "L", zone: "Z" }, layers: [], objects: [] },
      { check: "empty_layer", blocking: false, text: "y", params: { layer: "L" }, layers: [], objects: [] },
      { check: "unfilled_variable", blocking: false, text: "z", params: { kind: "serial_wrap", names: "T", end: 9, start: 0 }, layers: [], objects: [] },
    ],
    disabled: ["duplicate_shapes"],
    checks: ["keep_out"],
  });
  const { refusals, warnings } = splitFindings(r);
  assert.equal(refusals.length, 1);
  assert.equal(warnings.length, 2);
  for (const w of r.warnings) {
    const k = preflightKey(w);
    assert.ok(k.key in EN, k.key);
  }
  for (const c of ["cut_before_fill", "empty_layer", "unfilled_variable", "duplicate_shapes", "keep_out"]) assert.ok(`pf.check.${c}` in EN, c);
  assert.deepEqual(parsePreflight(null), { warnings: [], disabled: [], checks: [] });
});
