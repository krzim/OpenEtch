// Unit tests for command search (commandSearch.ts): ranking, aliases, recents and de-duplication.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { ACTION_BY_ID, ACTIONS } from "../workspace/hotkeys.ts";
import { ARRANGE_ACTION_ID, type ArrangeAction } from "./arrangeActions.ts";
import { ALIASES, mergeCommands, normalize, pushRecent, scoreCommand, searchCommands, type Command } from "./commandSearch.ts";

const noop = () => {};
const cmd = (key: string, label: string, extra: Partial<Command> = {}): Command => ({ key, label, actionId: key.includes(".") ? key : undefined, aliases: ALIASES[key], run: noop, ...extra });

const CMDS: Command[] = [
  cmd("tool.rect", "Rectangle"),
  cmd("tool.ellipse", "Ellipse"),
  cmd("tool.text", "Text"),
  cmd("tool.trim", "Trim shapes"),
  cmd("arrange.alignLeft", "Align left"),
  cmd("arrange.alignRight", "Align right"),
  cmd("app.preferences", "Preferences"),
  cmd("help.hotkeys", "Keyboard shortcuts"),
  cmd("edit.convertToPath", "Convert to path"),
  cmd("edit.delete", "Delete"),
  cmd("laser.stop", "Stop"),
  cmd("menu:File/Export/SVG", "SVG", { path: "File › Export" }),
];
const labels = (cs: Command[]) => cs.map((c) => c.label);

test("prefix beats word start beats substring", () => {
  const r = labels(searchCommands(CMDS, "re"));
  assert.equal(r[0], "Rectangle", "label prefix first");
  assert.ok(r.includes("Preferences"), "substring still listed");
  assert.ok(r.indexOf("Rectangle") < r.indexOf("Preferences"));
  assert.deepEqual(labels(searchCommands(CMDS, "left")), ["Align left"], "word start");
});

test("several words must all match, in any order", () => {
  assert.deepEqual(labels(searchCommands(CMDS, "align ri")), ["Align right"]);
  assert.deepEqual(labels(searchCommands(CMDS, "right align")), ["Align right"]);
  assert.deepEqual(searchCommands(CMDS, "align zzz"), []);
});

test("aliases, initials, the action id and the menu path find commands", () => {
  assert.equal(searchCommands(CMDS, "settings")[0].label, "Preferences");
  assert.equal(searchCommands(CMDS, "shortcuts")[0].label, "Keyboard shortcuts");
  assert.equal(searchCommands(CMDS, "abort")[0].label, "Stop");
  assert.equal(searchCommands(CMDS, "ctp")[0].label, "Convert to path", "initials");
  assert.equal(searchCommands(CMDS, "convertToPath")[0].label, "Convert to path", "MCP / hotkey id");
  assert.ok(labels(searchCommands(CMDS, "export")).includes("SVG"), "menu path");
  assert.equal(searchCommands(CMDS, "rctngl")[0].label, "Rectangle", "fuzzy letters in order");
});

test("accents and case do not matter", () => {
  assert.equal(normalize("Élipse  Ñ"), "elipse n");
  const es = [cmd("tool.ellipse", "Elipse"), cmd("app.preferences", "Preferencias")];
  assert.equal(searchCommands(es, "ÉLI")[0].label, "Elipse");
});

test("recents come first with nothing typed and get a lift when typed", () => {
  const r = searchCommands(CMDS, "", ["tool.trim", "laser.stop"]);
  assert.deepEqual(labels(r).slice(0, 2), ["Trim shapes", "Stop"]);
  assert.equal(r.length, CMDS.length);
  // "align" matches both equally; the recent one wins.
  assert.equal(searchCommands(CMDS, "align", ["arrange.alignRight"])[0].label, "Align right");
  assert.equal(searchCommands(CMDS, "align")[0].label, "Align left");
});

test("disabled commands sink below enabled ones of the same score", () => {
  const cs = [cmd("a.one", "Align one", { disabled: true }), cmd("a.two", "Align two")];
  assert.deepEqual(labels(searchCommands(cs, "align")), ["Align two", "Align one"]);
  assert.equal(scoreCommand(cs[0], ""), 1);
});

test("pushRecent moves to the front and caps the list", () => {
  assert.deepEqual(pushRecent(["a", "b", "c"], "b"), ["b", "a", "c"]);
  assert.deepEqual(pushRecent(["a", "b", "c"], "d", 3), ["d", "a", "b"]);
});

test("a menu item that runs an action replaces its entry; nothing is listed twice", () => {
  let ran = "";
  const actions = [cmd("edit.delete", "Delete", { run: () => (ran = "key") }), cmd("tool.rect", "Rectangle")];
  const menu = [
    cmd("menu:Edit/Delete", "Delete", { actionId: "edit.delete", path: "Edit", run: () => (ran = "menu") }),
    cmd("menu:Ctx/Delete", "Delete", { actionId: "edit.delete", path: "Context" }),
    cmd("menu:File/New", "New"),
    cmd("menu:File/New", "New"),
  ];
  const out = mergeCommands(actions, menu);
  assert.deepEqual(out.map((c) => c.key), ["edit.delete", "tool.rect", "menu:File/New"]);
  out[0].run();
  assert.equal(ran, "menu");
  assert.equal(out[0].path, "Edit");
  const keys = out.map((c) => c.key);
  assert.equal(new Set(keys).size, keys.length);
});

test("every Arrange action but centre-on-stock is a hotkeys.ts action, each a different one", () => {
  const ids = Object.entries(ARRANGE_ACTION_ID).filter(([a]) => a !== "stock").map(([, id]) => id);
  for (const id of ids) assert.ok(id && ACTION_BY_ID.has(id), String(id));
  assert.equal(new Set(ids).size, ids.length);
  assert.equal(ARRANGE_ACTION_ID.stock, undefined);
});

test("the Align menu, the context menu's align items and the hotkey actions list each Align command once", () => {
  // The toolbar's Arrange > Align submenu and the edit hook's Arrange items, as menuCommands flattens them, with the
  // action ids they now carry; before, five of the six had none and read twice ("Align right" and "Align right").
  const aligns: ArrangeAction[] = ["left", "hcenter", "right", "top", "vcenter", "bottom", "centers", "distH", "distHc", "distV", "distVc"];
  const actions = ACTIONS.filter((a) => a.id.startsWith("arrange.") && !a.fixed).map((a) => cmd(a.id, a.label));
  const menu = [
    ...aligns.map((a) => cmd(`menu:Arrange › Align/${a}`, `Align ${a}`, { actionId: ARRANGE_ACTION_ID[a], path: "Arrange › Align" })),
    ...aligns.map((a) => cmd(`menu:Arrange/${a}`, `Align ${a}`, { actionId: ARRANGE_ACTION_ID[a], path: "Arrange" })),
  ];
  const out = mergeCommands(actions, menu);
  for (const a of aligns) assert.equal(out.filter((c) => c.actionId === ARRANGE_ACTION_ID[a]).length, 1, a);
  assert.equal(out.length, actions.length, "no menu entry beside its action");
});

test("every action id a menu item names in the source is a hotkeys.ts action", () => {
  const src = join(fileURLToPath(new URL(".", import.meta.url)), "..");
  const bad: string[] = [];
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (/\.tsx?$/.test(p) && !p.endsWith(".test.ts")) {
        for (const m of readFileSync(p, "utf8").matchAll(/(?:actionId: |data-action-id=)"([\w.]+)"/g)) if (!ACTION_BY_ID.has(m[1])) bad.push(`${name}: ${m[1]}`);
      }
    }
  };
  walk(src);
  assert.deepEqual(bad, []);
});

test("the Laser Tools items and the File menu's New tab row carry their action ids (one entry each)", () => {
  // Seen in the app: "Material Library" was listed twice (File › Laser and Laser Tools), and "New tab" beside
  // "New document tab".
  const src = join(fileURLToPath(new URL(".", import.meta.url)), "..");
  const libs = readFileSync(join(src, "libs", "useLibsUi.tsx"), "utf8");
  const laserTools = libs.slice(libs.indexOf("const laserTools: DropItem[]"), libs.indexOf("];", libs.indexOf("const laserTools: DropItem[]")));
  for (const id of ["laser.materialTest", "laser.intervalTest", "laser.focusTest", "tool.startPoint", "laser.printCut", "window.library"]) {
    assert.ok(laserTools.includes(`actionId: "${id}"`), id);
  }
  assert.ok(!/hint: hk\(/.test(laserTools), "no item names its chord without its action");
  assert.ok(readFileSync(join(src, "App.tsx"), "utf8").includes('data-action-id="window.newTab"'));
  const menu = mergeCommands([cmd("window.library", "Material Library")], [
    cmd("menu:File › Laser/Material Library", "Material Library", { actionId: "window.library" }),
    cmd("menu:Laser Tools/Material Library", "Material Library", { actionId: "window.library" }),
  ]);
  assert.equal(menu.length, 1);
});
