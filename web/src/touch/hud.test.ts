// Unit tests for the touch HUD's model (touch/hudModel.ts), the long-press timing (touch/useLongPress.ts) and the
// touch actions' key mapping (touch/actions.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import { barActions, modToggles, selectionCovered } from "./hudModel.ts";
import { LongPressTracker } from "./useLongPress.ts";
import { TOUCH_ACTIONS, chordForAction, keyInitFor } from "./actions.ts";

test("modifier toggles follow the tool and stand for LightBurn's keys", () => {
  assert.deepEqual(
    modToggles("select", false).map((g) => g.mod),
    ["shift", "ctrl", "alt"],
    "Add (and constrain), Toggle (and no snap), guides only",
  );
  assert.deepEqual(
    modToggles("select", true).map((g) => [g.mod, g.label]),
    [
      ["shift", "touch.mod15"],
      ["ctrl", "touch.mod5"],
    ],
    "rotate handles: 15° and fine 5° steps",
  );
  assert.deepEqual(
    modToggles("rect", false).map((g) => g.label),
    ["touch.modSquare", "touch.modFromCentre"],
  );
  assert.deepEqual(
    modToggles("line", false).map((g) => g.label),
    ["touch.mod15", "touch.modNoSnap"],
  );
  assert.deepEqual(
    modToggles("node", false).map((g) => g.mod),
    ["shift"],
  );
  assert.deepEqual(modToggles("trim", false), [], "Trim has no modifiers");
});

test("the action bar: Group with several loose objects, Ungroup with a group, always Copy/Paste and the menu", () => {
  assert.deepEqual(barActions(0, false), []);
  const one = barActions(1, false).map((a) => a.id);
  assert.deepEqual(one, ["copy", "paste", "duplicate", "delete", "rotate90", "flipH", "flipV", "menu"]);
  assert.ok(barActions(3, false).some((a) => a.id === "group"));
  assert.ok(barActions(3, true).some((a) => a.id === "ungroup"));
  assert.ok(!barActions(3, true).some((a) => a.id === "group"));
  for (const a of barActions(3, false)) assert.ok(a.id === "menu" || a.id in TOUCH_ACTIONS, a.id);
});

test("long-press fires at 500 ms within the slop, once", () => {
  const tr = new LongPressTracker();
  assert.equal(tr.down(1, 10, 10, 0), true);
  tr.move(1, 15, 14);
  assert.equal(tr.due(499), false);
  assert.equal(tr.due(500), true);
  assert.equal(tr.fired, true);
  assert.equal(tr.due(900), false, "once");
  assert.deepEqual(tr.at, [10, 10]);
});

test("moving past the slop, lifting early or a second finger cancels the long-press", () => {
  const tr = new LongPressTracker();
  tr.down(1, 0, 0, 0);
  tr.move(1, 0, 11);
  assert.equal(tr.due(600), false, "a scroll");
  tr.down(2, 0, 0, 1000);
  tr.up(2);
  assert.equal(tr.due(1600), false, "lifted");
  tr.down(3, 0, 0, 2000);
  assert.equal(tr.down(4, 50, 0, 2100), false, "a second finger");
  assert.equal(tr.due(2600), false);
  assert.equal(tr.fired, false);
});

test("touch actions are sent as the action's current chord (rebinding follows)", () => {
  assert.equal(chordForAction("edit.copy", {}), "Ctrl+C");
  assert.equal(chordForAction("edit.copy", { "edit.copy": ["Ctrl+Shift+K"] }), "Ctrl+Shift+K");
  assert.equal(chordForAction("nope", {}), null);
  assert.deepEqual(keyInitFor("copy", {}), { key: "c", code: "KeyC", ctrlKey: true, altKey: false, shiftKey: false });
  assert.deepEqual(keyInitFor("flipH", {}), { key: "H", code: "KeyH", ctrlKey: true, altKey: false, shiftKey: true });
  assert.deepEqual(keyInitFor("rotate90", {}), { key: ".", code: "Period", ctrlKey: false, altKey: false, shiftKey: false });
  assert.deepEqual(keyInitFor("nudgeLeft", {}, { shift: true }), { key: "ArrowLeft", code: "ArrowLeft", ctrlKey: false, altKey: false, shiftKey: true });
  assert.equal(keyInitFor("menu", {}), null, "the menu is a contextmenu, not a key");
  assert.equal(keyInitFor("unknown", {}), null);
  assert.equal(keyInitFor("undo", {})?.key, "z");
});

test("the action bar hides while a sheet covers the selection (the canvas HUD places it otherwise)", () => {
  const sel = { left: 100, top: 500, right: 200, bottom: 560 };
  assert.equal(selectionCovered(sel, null), false, "no sheet");
  // The Layers sheet at half height (top edge at y 400) covers the shape.
  assert.equal(selectionCovered(sel, 400), true);
  // Covered by more than half (only 20 of 60 px show): still hidden.
  assert.equal(selectionCovered(sel, 520), true);
  // Mostly above the sheet: shown.
  assert.equal(selectionCovered(sel, 545), false);
  // A sheet at full height covers everything.
  assert.equal(selectionCovered({ left: 100, top: 200, right: 200, bottom: 260 }, 50), true);
});
