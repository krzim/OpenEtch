// Unit tests for folding the canvas context menu into submenus (contextMenu.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { structureMenu } from "./contextMenu.ts";
import type { MenuItem } from "./Menu.tsx";

const a = () => {};
const it = (label: string, hint?: string, extra: Partial<Exclude<MenuItem, { sep: true }>> = {}): MenuItem => ({ label, hint, action: a, ...extra });
const SEP: MenuItem = { sep: true };

/** Roughly App.tsx's canvas menu with the edit, files, libs and docking entries. */
function canvasMenu(): MenuItem[] {
  return [
    it("Cut", "Ctrl+X"),
    it("Copy", "Ctrl+C"),
    it("Paste", "Ctrl+V"),
    it("Duplicate", "Ctrl+D"),
    it("Delete", "Del"),
    SEP,
    it("Group", "Ctrl+G"),
    it("Ungroup", "Ctrl+U"),
    SEP,
    it("Edit nodes", "Ctrl+`"),
    it("Weld", "Ctrl+W"),
    it("Break apart", "Alt+B"),
    it("Convert to path", "Ctrl+Shift+C"),
    it("Bring to front", "Ctrl+PgUp"),
    it("Send to back", "Ctrl+PgDn"),
    it("Lock"),
    SEP,
    it("Trace image", "Alt+T", { section: "modify" }),
    it("Set start point", undefined, { section: "laser" }),
    it("Clear start point", undefined, { section: "laser" }),
    SEP,
    it("Paste in place", "Alt+V", { section: "top" }),
    it("Warp", undefined, { section: "modify" }),
    SEP,
    it("Edit text", "Double-click"),
    SEP,
    it("Align left", "L"),
    it("Align centres", "C"),
    it("Align right", "R"),
    it("Centre on stock"),
    it("Centre on field", "P"),
    SEP,
    it("Dock left"),
    it("Dock right"),
    it("Dock up"),
    it("Dock down"),
    SEP,
    it("Flip horizontal", "Ctrl+Shift+H"),
    it("Flip vertical", "Ctrl+Shift+V"),
    SEP,
    it("Select all", "Ctrl+A"),
    it("Invert selection", "Ctrl+Shift+I"),
  ];
}

const labels = (items: MenuItem[]) => items.map((x) => (x.sep ? "—" : x.label));
const sub = (items: MenuItem[], label: string) => {
  const row = items.find((x) => !x.sep && x.label === label);
  return row && !row.sep ? (row.submenu ?? []) : [];
};

test("short menus are left alone", () => {
  const m = [it("Rename"), it("Delete", "Del"), SEP, it("Move up")];
  assert.equal(structureMenu(m), m);
});

test("the canvas menu folds into at most 14 rows with Modify, Arrange, Select and Laser submenus", () => {
  const out = structureMenu(canvasMenu());
  const rows = out.filter((x) => !x.sep);
  assert.ok(rows.length <= 14, `${rows.length} rows: ${labels(out).join(", ")}`);
  assert.deepEqual(labels(out).slice(0, 5), ["Cut", "Copy", "Paste", "Duplicate", "Delete"]);
  assert.deepEqual(
    rows.filter((x) => !x.sep && x.submenu).map((x) => !x.sep && x.label),
    ["Modify", "Arrange", "Select", "Laser"],
  );
  assert.deepEqual(labels(sub(out, "Modify")), ["Edit nodes", "Weld", "Break apart", "Convert to path", "Lock", "—", "Trace image", "—", "Warp"]);
  assert.deepEqual(labels(sub(out, "Arrange")), [
    "Bring to front",
    "Send to back",
    "—",
    "Align left",
    "Align centres",
    "Align right",
    "Centre on stock",
    "Centre on field",
    "—",
    "Dock left",
    "Dock right",
    "Dock up",
    "Dock down",
    "—",
    "Flip horizontal",
    "Flip vertical",
  ]);
  assert.deepEqual(labels(sub(out, "Select")), ["Select all", "Invert selection"]);
  assert.deepEqual(labels(sub(out, "Laser")), ["Set start point", "Clear start point"]);
  // Group, Paste in place and the text entry stay at the top.
  for (const l of ["Group", "Ungroup", "Paste in place", "Edit text"]) assert.ok(labels(out).includes(l), l);
});

test("every action survives the folding exactly once", () => {
  const flat = (items: MenuItem[]): string[] => items.flatMap((x) => (x.sep ? [] : x.submenu ? flat(x.submenu) : [x.label]));
  const before = flat(canvasMenu()).sort();
  const after = flat(structureMenu(canvasMenu())).sort();
  assert.deepEqual(after, before);
});

test("a submenu of only disabled items is disabled; labels are translated; More takes the overflow", () => {
  const m: MenuItem[] = [];
  for (let i = 0; i < 20; i++) m.push(it(`Item ${i}`), SEP);
  m.push(it("Select all", "Ctrl+A", { disabled: true }));
  const out = structureMenu(m, { select: "Seleccionar", more: "Más" });
  const sel = out.find((x) => !x.sep && x.label === "Seleccionar");
  assert.ok(sel && !sel.sep && sel.disabled);
  assert.ok(out.some((x) => !x.sep && x.label === "Más"));
  assert.ok(out.filter((x) => !x.sep).length <= 14);
});
