// Unit tests for the Esc stack shared by dialogs and menus (dialogStack.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { LayerStack } from "./dialogStack.ts";

test("Esc closes only the top dialog of two", () => {
  const s = new LayerStack();
  const closed: string[] = [];
  const a = s.push("dialog", () => (closed.push("a"), s.remove(a)));
  const b = s.push("dialog", () => (closed.push("b"), s.remove(b)));
  assert.ok(s.isTop(b));
  assert.equal(s.escape(), true);
  assert.deepEqual(closed, ["b"]);
  assert.ok(s.isTop(a));
  assert.equal(s.escape(), true);
  assert.deepEqual(closed, ["b", "a"]);
  assert.equal(s.escape(), false, "nothing left: Esc goes on to the app");
});

test("a menu over a dialog takes Esc first; a submenu before its menu", () => {
  const s = new LayerStack();
  const log: string[] = [];
  const d = s.push("dialog", () => log.push("dialog"));
  const m = s.push("menu", () => (log.push("menu"), s.remove(m)));
  const sub = s.push("menu", () => (log.push("sub"), s.remove(sub)));
  s.escape();
  s.escape();
  s.escape();
  assert.deepEqual(log, ["sub", "menu", "dialog"]);
  assert.equal(s.topDialog()?.id, d);
});

test("layers removed out of order leave the right top; topDialog skips menus", () => {
  const s = new LayerStack();
  const a = s.push("dialog", () => {});
  const b = s.push("dialog", () => {});
  const m = s.push("menu", () => {});
  s.remove(b);
  assert.equal(s.top()?.id, m);
  assert.equal(s.topDialog()?.id, a);
  assert.equal(s.dialogCount(), 1);
  s.remove(999);
  assert.equal(s.size, 2);
});

test("update swaps the Esc handler; subscribers hear pushes and removals", () => {
  const s = new LayerStack();
  let n = 0;
  const off = s.subscribe(() => n++);
  const id = s.push("dialog", () => assert.fail("stale handler"));
  let fresh = false;
  s.update(id, () => (fresh = true));
  s.escape();
  assert.ok(fresh);
  s.remove(id);
  off();
  s.push("menu", () => {});
  assert.equal(n, 2);
});
