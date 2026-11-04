import assert from "node:assert/strict";
import test from "node:test";
import { ACTION_BY_ID, chordsFor } from "./hotkeys.ts";
import { chordFromQt, chordToQt, exportLightBurnHotkeys, importLightBurnHotkeys, LIGHTBURN_ACTIONS } from "./lightburnHotkeys.ts";

test("every LightBurn action maps to one real, rebindable OpenEtch action", () => {
  const ids = Object.values(LIGHTBURN_ACTIONS);
  assert.equal(new Set(ids).size, ids.length, "no two LightBurn actions share an OpenEtch action");
  for (const [lb, id] of Object.entries(LIGHTBURN_ACTIONS)) {
    assert.match(lb, /^action[A-Z]\w+$/);
    const a = ACTION_BY_ID.get(id);
    assert.ok(a && !a.fixed, `${lb} -> ${id}`);
  }
  assert.ok(!ids.includes("laser.stop"), "STOP is never mapped");
});

test("Qt key texts and OpenEtch chords convert both ways", () => {
  assert.equal(chordFromQt("Ctrl+Alt+Shift+D"), "Ctrl+Alt+Shift+D");
  assert.equal(chordFromQt("Shift+Ctrl+d"), "Ctrl+Shift+D");
  assert.equal(chordFromQt("Alt+PgDown"), "Alt+PgDn");
  assert.equal(chordFromQt("Alt+Return"), "Alt+Enter");
  assert.equal(chordFromQt("Ctrl++"), "Ctrl++");
  assert.equal(chordFromQt("Ctrl+K, Ctrl+C"), null, "a sequence of chords");
  assert.equal(chordFromQt("Meta+A"), null);
  assert.equal(chordFromQt(""), null);
  assert.equal(chordToQt("Alt+PgDn"), "Alt+PgDown");
  assert.equal(chordToQt("Alt+Enter"), "Alt+Return");
  assert.equal(chordToQt("Ctrl+Shift++"), "Ctrl+Shift++");
});

test("a LightBurn hotkeys.json sets each mapped action to exactly its key", () => {
  const raw = {
    actionDistribute_Horizontal_Centered: "Ctrl+Alt+Shift+D",
    actionGroup: "Ctrl+Shift+G",
    actionUngroup: "",
    actionStop_Laser: "F9",
    actionCamera_Chooser: "Ctrl+Alt+C",
    actionWeld_Shapes: "Ctrl+W",
    actionZoom_In: 3,
  };
  const r = importLightBurnHotkeys(raw, { "arrange.group": ["Ctrl+G", "Alt+G"] });
  assert.deepEqual(r.overrides["arrange.distHCentered"], ["Ctrl+Alt+Shift+D"]);
  assert.deepEqual(r.overrides["arrange.group"], ["Ctrl+Shift+G"], "LightBurn's key replaces the action's keys");
  assert.deepEqual(r.overrides["arrange.ungroup"], [], "an empty text leaves the action without a key");
  assert.deepEqual(r.applied.sort(), ["arrange.distHCentered", "arrange.group", "arrange.ungroup"]);
  assert.equal(r.skipped.length, 4);
  assert.match(r.skipped.join("\n"), /actionStop_Laser: STOP keeps its keys/);
  assert.match(r.skipped.join("\n"), /actionCamera_Chooser: no matching OpenEtch action/);
  assert.match(r.skipped.join("\n"), /actionWeld_Shapes: "Ctrl\+W" is kept by the browser/);
  assert.match(r.skipped.join("\n"), /actionZoom_In: not a key/);
  assert.deepEqual(importLightBurnHotkeys(null, {}).applied, []);
  assert.deepEqual(importLightBurnHotkeys(["x"], {}).applied, []);
});

test("an imported key is taken from the action that had it", () => {
  // Ctrl+D is Duplicate by default.
  const r = importLightBurnHotkeys({ actionDistribute_Vertical_Centered: "Ctrl+D" }, {});
  assert.deepEqual(r.overrides["arrange.distVCentered"], ["Ctrl+D"]);
  const dup = ACTION_BY_ID.get("edit.duplicate");
  assert.ok(dup && !chordsFor(dup, r.overrides).includes("Ctrl+D"));
});

test("OpenEtch's overrides export as LightBurn names, and import back the same", () => {
  const overrides = { "arrange.distHCentered": ["Ctrl+Alt+Shift+D"], "view.preview": ["Alt+Enter"], "arrange.ungroup": [], "help.guide": ["F2"] };
  const out = exportLightBurnHotkeys(overrides);
  assert.deepEqual(out, { actionDistribute_Horizontal_Centered: "Ctrl+Alt+Shift+D", actionPreview: "Alt+Return", actionUngroup: "" });
  const back = importLightBurnHotkeys(out, {});
  for (const id of ["arrange.distHCentered", "view.preview", "arrange.ungroup"]) assert.deepEqual(back.overrides[id], overrides[id as keyof typeof overrides]);
});
