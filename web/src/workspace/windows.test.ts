// Unit tests for the window layout (dock, float, tab, hide, reset), the Docking tool's moves, and the parsers of the
// open-documents and unsupported-settings answers.

import { test } from "node:test";
import assert from "node:assert/strict";
import type { Vec2 } from "../api/types.ts";
import { contactDistance, dockMoves, polysBox, type DockShape, type Polyline } from "./docking.ts";
import {
  applyPreset,
  areaUnits,
  DEFAULT_LAYOUT,
  DEFAULT_SIZES,
  defaultAreaMode,
  dockPolicy,
  dockTo,
  moveInArea,
  placeOf,
  sanitizeLayout,
  setActiveTab,
  setAreaMode,
  setDockSize,
  setVisible,
  tabWith,
  untab,
} from "./layout.ts";
import { hasLightBurnExtras, neighbourAfterClose, parseDocuments, parseUnsupported, unsupportedText } from "./wsApi.ts";

const KNOWN = ["browser", "laser", "cuts", "library", "properties"];

test("layout: defaults, docking to another area, floating and hiding", () => {
  const l = DEFAULT_LAYOUT;
  assert.equal(placeOf(l, "browser").area, "left");
  assert.equal(placeOf(l, "cuts").area, "right");
  assert.equal(placeOf(l, "someone-elses").area, "right", "unknown windows start on the right");
  assert.deepEqual(
    areaUnits(l, "right", KNOWN).map((u) => u.key),
    ["laser", "cuts", "library", "properties"],
  );
  const b = dockTo(l, "cuts", "bottom", KNOWN);
  assert.equal(placeOf(b, "cuts").area, "bottom");
  assert.deepEqual(areaUnits(b, "bottom", KNOWN).map((u) => u.key), ["cuts"]);
  const f = dockTo(b, "library", "float", KNOWN);
  assert.equal(placeOf(f, "library").area, "float");
  assert.ok(placeOf(f, "library").float, "a floating window gets a position");
  const h = setVisible(f, "properties", false);
  assert.ok(!areaUnits(h, "right", KNOWN).some((u) => u.members.includes("properties")), "hidden windows have no frame");
  assert.equal(placeOf(setVisible(h, "properties", true), "properties").visible, true);
});

test("layout: tab groups share one frame and dissolve when one member is left", () => {
  const t = tabWith(DEFAULT_LAYOUT, "library", "properties");
  const units = areaUnits(t, "right", KNOWN);
  const g = units.find((u) => u.members.length > 1);
  assert.ok(g, "library and properties are one frame");
  assert.deepEqual([...g.members].sort(), ["library", "properties"]);
  assert.equal(g.active, "library", "the window just tabbed in is shown");
  const t2 = setActiveTab(t, placeOf(t, "properties").group!, "properties");
  assert.equal(areaUnits(t2, "right", KNOWN).find((u) => u.members.length > 1)?.active, "properties");
  // Tabbing into a window on the bottom moves the window there.
  const bottom = tabWith(dockTo(DEFAULT_LAYOUT, "cuts", "bottom", KNOWN), "laser", "cuts");
  assert.equal(placeOf(bottom, "laser").area, "bottom");
  // Separating one of two dissolves the group.
  const u = untab(t, "library");
  assert.equal(placeOf(u, "library").group, null);
  assert.equal(placeOf(u, "properties").group, null);
  // Docking a member elsewhere takes it out of the group too.
  const d = dockTo(t, "properties", "left", KNOWN);
  assert.equal(placeOf(d, "library").group, null);
});

test("layout: moving a frame up keeps the others in order; garbage is sanitised", () => {
  const m = moveInArea(DEFAULT_LAYOUT, "library", -1, KNOWN);
  assert.deepEqual(areaUnits(m, "right", KNOWN).map((u) => u.key), ["laser", "library", "cuts", "properties"]);
  assert.equal(moveInArea(DEFAULT_LAYOUT, "laser", -1, KNOWN), DEFAULT_LAYOUT, "the first cannot move up");
  const s = sanitizeLayout({
    windows: { cuts: { area: "moon", order: "x", visible: 3, float: { x: 1, y: 2, w: 5, h: 5 } }, "bad id!": { area: "left" }, lone: { area: "right", group: "g1" } },
    activeTab: { g1: "lone", junk: 5 },
  });
  assert.equal(placeOf(s, "cuts").area, "right");
  assert.equal(placeOf(s, "cuts").visible, true);
  assert.equal(placeOf(s, "cuts").float, null, "a frame smaller than the minimum is dropped");
  assert.ok(!("bad id!" in s.windows));
  assert.equal(placeOf(s, "lone").group, null, "a group of one is no group");
  assert.deepEqual(sanitizeLayout(null), DEFAULT_LAYOUT);
});

// --- sizes, rails, presets ------------------------------------------------------------------------------------------------

test("layout: sizes are clamped and kept, area modes validated, bad input gives the defaults", () => {
  const l = sanitizeLayout({ sizes: { left: 900, right: 100, bottom: "x" }, areaMode: { left: "rail", right: "sideways", bottom: "rail" } });
  assert.deepEqual(l.sizes, { left: 420, right: 260, bottom: DEFAULT_SIZES.bottom }, "clamped to 200-420 / 260-480 / 120-600");
  assert.deepEqual(l.areaMode, { left: "rail" }, "unknown modes and areas are dropped");
  assert.deepEqual(sanitizeLayout({ sizes: null, areaMode: 5 }).sizes, DEFAULT_SIZES);
  assert.deepEqual(sanitizeLayout({ sizes: { left: 333.4, right: Infinity } }).sizes, { left: 333, right: DEFAULT_SIZES.right, bottom: DEFAULT_SIZES.bottom });
  // Every change keeps them.
  const r = setDockSize(DEFAULT_LAYOUT, "right", 400);
  assert.equal(r.sizes.right, 400);
  assert.equal(setDockSize(r, "right", 10).sizes.right, 260);
  const t = tabWith(dockTo(setAreaMode(r, "left", "pinned"), "cuts", "bottom", KNOWN), "laser", "cuts");
  assert.equal(t.sizes.right, 400, "docking and tabbing keep the sizes");
  assert.equal(t.areaMode.left, "pinned", "... and the area modes");
  assert.deepEqual(setAreaMode(t, "left", null).areaMode, {}, "null lets the layout decide again");
  // A stored layout survives a round trip.
  assert.deepEqual(sanitizeLayout(JSON.parse(JSON.stringify(t))), t);
});

test("dockPolicy: defaults per layout mode", () => {
  assert.equal(defaultAreaMode("wide", "left"), "pinned");
  assert.equal(defaultAreaMode("compact", "left"), "rail", "compact rails the Browser (owner decision 9)");
  assert.equal(defaultAreaMode("compact", "right"), "pinned");
  assert.equal(defaultAreaMode("tablet", "right"), "rail");
  assert.deepEqual(dockPolicy(1920, DEFAULT_SIZES, { layout: "wide" }), { left: "pinned", right: "pinned" });
  assert.deepEqual(dockPolicy(1280, DEFAULT_SIZES, { layout: "compact" }), { left: "rail", right: "pinned" });
  assert.deepEqual(dockPolicy(1024, DEFAULT_SIZES, { layout: "tablet", reserved: 52 }), { left: "rail", right: "rail" });
  assert.deepEqual(dockPolicy(390, DEFAULT_SIZES, { layout: "phone" }), { left: "hidden", right: "hidden" });
});

test("dockPolicy: the canvas keeps 640 px; the left side rails first, then the right", () => {
  const sizes = { left: 260, right: 320, bottom: 240 };
  // 1280 wide with both pinned: 1280 - 260 - 320 = 700, fine.
  assert.deepEqual(dockPolicy(1280, sizes, { layout: "wide" }), { left: "pinned", right: "pinned" });
  // 1200: 620 < 640, so the left rails (1200 - 32 - 320 = 848).
  assert.deepEqual(dockPolicy(1200, sizes, { layout: "wide" }), { left: "rail", right: "pinned" });
  // 960: even with the left on a rail 960 - 32 - 320 = 608, so the right rails too.
  assert.deepEqual(dockPolicy(960, sizes, { layout: "wide" }), { left: "rail", right: "rail" });
  // Wider panels rail sooner.
  assert.deepEqual(dockPolicy(1440, { left: 420, right: 480, bottom: 240 }, { layout: "wide" }), { left: "rail", right: "pinned" });
  // A side the user pinned explicitly goes last: here the right rails instead of the left.
  assert.deepEqual(dockPolicy(1200, sizes, { layout: "wide", modes: { left: "pinned" } }), { left: "pinned", right: "rail" });
  // Hidden stays hidden and frees the room.
  assert.deepEqual(dockPolicy(1000, sizes, { layout: "wide", modes: { left: "hidden" } }), { left: "hidden", right: "pinned" });
  // The tablet's tool rail counts: 1100 - 52 - 260 - 40 = 748 with the left pinned by the user and the right a rail.
  assert.deepEqual(dockPolicy(1100, sizes, { layout: "tablet", reserved: 52, modes: { left: "pinned" } }), { left: "pinned", right: "rail" });
});

test("applyPreset: compact and touch make the plan's tab groups; wide is the default layout pinned", () => {
  const known = ["browser", "laser", "pages", "cuts", "library", "artLibrary", "varText", "docking", "properties", "machine"];
  const sized = setDockSize(dockTo(DEFAULT_LAYOUT, "cuts", "float", known), "right", 400);
  const c = applyPreset(sized, "compact");
  const g = (id: string) => placeOf(c, id).group;
  assert.ok(g("cuts") && g("cuts") === g("properties"), "Cuts/Layers and Properties share a frame");
  assert.ok(g("library") && [g("artLibrary"), g("varText"), g("docking")].every((x) => x === g("library")), "the libraries share a frame");
  assert.ok(g("laser") && g("laser") === g("machine"), "Laser and Machine share a frame");
  assert.equal(placeOf(c, "cuts").area, "right", "the preset starts from the default places");
  const units = areaUnits(c, "right", known);
  assert.deepEqual(
    units.map((u) => u.active),
    ["laser", "pages", "cuts", "library"],
    "four frames on the right, the first member of each group shown",
  );
  assert.deepEqual(c.areaMode, { left: "rail", right: "pinned" });
  assert.equal(c.sizes.right, 400, "sizes are kept");
  assert.deepEqual(applyPreset(c, "touch").areaMode, { left: "rail", right: "rail" });
  assert.equal(placeOf(applyPreset(c, "touch"), "properties").group, placeOf(applyPreset(c, "touch"), "cuts").group);
  const w = applyPreset(c, "wide");
  assert.equal(placeOf(w, "properties").group, null, "wide has no groups");
  assert.deepEqual(w.areaMode, { left: "pinned", right: "pinned" });
  assert.equal(w.sizes.right, 400);
});

// --- Docking ---------------------------------------------------------------------------------------------------------------

const rect = (id: string, x0: number, y0: number, x1: number, y1: number): DockShape => {
  const polys: Polyline[] = [
    [
      [x0, y0],
      [x1, y0],
      [x1, y1],
      [x0, y1],
      [x0, y0],
    ],
  ];
  return { id, polys, box: polysBox(polys)! };
};
const WS: Vec2 = [100, 100];
const OPT = { moveAsGroup: false, lockInner: true, padding: 0 };
const close = (a: Vec2, b: Vec2) => Math.abs(a[0] - b[0]) < 1e-9 && Math.abs(a[1] - b[1]) < 1e-9;

test("docking: slides to the next shape's outline or the workspace edge", () => {
  const a = rect("a", 0, 0, 10, 10);
  const wall = rect("w", 30, -20, 35, 20);
  assert.equal(contactDistance(a.polys, wall.polys, [1, 0]), 20);
  const mv = dockMoves([a], [wall], "right", WS, OPT);
  assert.ok(close(mv[0].d, [20, 0]));
  // Nothing in the way: the workspace edge (field centred on 0, 100 mm wide).
  assert.ok(close(dockMoves([a], [wall], "left", WS, OPT)[0].d, [-50, 0]));
  assert.ok(close(dockMoves([a], [], "up", WS, OPT)[0].d, [0, 40]));
  // Padding keeps a gap in the direction of travel; already closer than that: no move.
  assert.ok(close(dockMoves([a], [wall], "right", WS, { ...OPT, padding: 2 })[0].d, [18, 0]));
  assert.deepEqual(dockMoves([rect("b", 19, 0, 29, 10)], [wall], "right", WS, { ...OPT, padding: 2 }), []);
  // A shape out of the lane does not stop it.
  assert.ok(close(dockMoves([a], [rect("off", 30, 20, 40, 30)], "right", WS, OPT)[0].d, [40, 0]));
});

test("docking: outlines, not boxes; containers and overlaps are no obstacles", () => {
  // A triangle whose slanted side faces the moving square: contact where the outlines meet.
  const tri: DockShape = (() => {
    const polys: Polyline[] = [
      [
        [20, 0],
        [40, 0],
        [40, 20],
        [20, 0],
      ],
    ];
    return { id: "t", polys, box: polysBox(polys)! };
  })();
  const sq = rect("s", 0, 10, 10, 20);
  // The square's lower-right corner (10, 10) meets the slanted edge at x = 30.
  assert.ok(close(dockMoves([sq], [tri], "right", WS, OPT)[0].d, [20, 0]));
  // Inside a container: goes on to the workspace edge.
  const box = rect("box", -40, -40, 40, 40);
  assert.ok(close(dockMoves([rect("in", 0, 0, 10, 10)], [box], "right", WS, OPT)[0].d, [40, 0]));
  // Overlapping at the start: not an obstacle.
  assert.ok(close(dockMoves([rect("o", 0, 0, 10, 10)], [rect("x", 5, 5, 15, 15)], "right", WS, OPT)[0].d, [40, 0]));
});

test("docking: several shapes stack; move as group stops together; inner shapes ride along", () => {
  const a = rect("a", 0, 0, 10, 10);
  const b = rect("b", -30, 0, -20, 10);
  const wall = rect("w", 30, -20, 35, 20);
  const each = dockMoves([b, a], [wall], "right", WS, OPT);
  assert.ok(close(each.find((m) => m.id === "a")!.d, [20, 0]));
  assert.ok(close(each.find((m) => m.id === "b")!.d, [40, 0]), "b stops against a's new place");
  const group = dockMoves([b, a], [wall], "right", WS, { ...OPT, moveAsGroup: true });
  assert.ok(group.every((m) => close(m.d, [20, 0])));
  const outer = rect("outer", -10, -10, 10, 10);
  const inner = rect("inner", -2, -2, 2, 2);
  const locked = dockMoves([outer, inner], [], "left", WS, OPT);
  assert.ok(locked.every((m) => close(m.d, [-40, 0])), "the inner shape keeps its place in the container");
  const free = dockMoves([outer, inner], [], "left", WS, { ...OPT, lockInner: false });
  // Unlocked, it is not held by the container's inner edge: it goes on until it meets the container's outer edge
  // (the container has docked at the workspace edge first): -2 -> -30.
  assert.ok(close(free.find((m) => m.id === "inner")!.d, [-28, 0]));
});

// --- open documents and unsupported settings --------------------------------------------------------------------------------

test("documents list: tabs from GET /api/documents", () => {
  const l = parseDocuments({
    active: "doc-2",
    documents: [
      { id: "default", name: "Untitled", project: null, display_name: null, document_name: "Untitled", dirty: false, objects: 0, active: false },
      { id: "doc-2", name: "Jig", project: "Jig", display_name: "Jig", dirty: true, objects: 3, active: true },
      { name: "no id" },
    ],
  });
  assert.ok(l);
  assert.equal(l.active, "doc-2");
  assert.deepEqual(
    l.documents.map((d) => [d.id, d.title, d.dirty, d.active]),
    [
      ["default", "Untitled", false, false],
      ["doc-2", "Jig", true, true],
    ],
  );
  assert.equal(parseDocuments({ ok: true }), null, "an older engine answers something else");
  assert.equal(neighbourAfterClose(l, "doc-2"), "default");
});

test("unsupported LightBurn settings: per layer, with sub-layers named", () => {
  const m = parseUnsupported({
    layers: [
      {
        layer_id: "l1",
        name: "Outline",
        unsupported: [
          { tag: "wobbleEnable", setting: "Wobble", value: "1", active: true, note: "not executed" },
          { tag: "perforate", setting: "Perforation", value: "0", active: false, note: "" },
          { tag: "zz", setting: "zz", value: "3", sub_layer: 1, active: null, note: "unknown" },
        ],
      },
    ],
  });
  const list = m.get("l1")!;
  assert.equal(list.length, 3);
  const text = unsupportedText(list, (n) => `sub ${n}`);
  assert.match(text, /^Wobble = 1: not executed/);
  assert.match(text, /zz \(sub 2\) = 3: unknown/);
  assert.ok(!text.includes("Perforation"), "a setting that is off is not listed");
  assert.equal(parseUnsupported(null).size, 0);
  assert.equal(hasLightBurnExtras([{ extras: { "lbrn2.wobbleEnable": "1" } }]), true);
  assert.equal(hasLightBurnExtras([{}]), false);
});

// --- the second language ------------------------------------------------------------------------------------------------

test("i18n: the machine-translated Spanish skeleton is registered, keeps placeholders and falls back to English", async () => {
  const { languages, translate } = await import("./i18n.ts");
  const { ES } = await import("./locales/es.ts");
  const { EN } = await import("./locales/en.ts");
  assert.ok(languages().some((l) => l.code === "es" && /automática/.test(l.name)), "marked as machine translated");
  for (const [k, v] of Object.entries(ES)) {
    assert.ok(k in EN, `es key ${k} exists in English`);
    const want = [...(EN[k as keyof typeof EN] as string).matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
    const got = [...(v as string).matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
    assert.deepEqual(got, want, `placeholders of ${k}`);
  }
  assert.equal(translate("es", "fm.saveAs"), "Guardar como…");
  assert.equal(translate("es", "tabs.close", { name: "Jig" }), "Cerrar Jig");
  assert.equal(translate("es", "fm.uploadHint"), EN["fm.uploadHint"], "untranslated keys fall back to English");
});
