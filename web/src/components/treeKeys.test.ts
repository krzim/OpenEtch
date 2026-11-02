import assert from "node:assert/strict";
import { test } from "node:test";
import { treeKeyAction } from "./treeKeys.ts";

test("tree keys: arrows, Home/End move focus within the visible rows", () => {
  assert.deepEqual(treeKeyAction("ArrowDown", 0, 3, null), { focus: 1 });
  assert.deepEqual(treeKeyAction("ArrowDown", 2, 3, null), { focus: 2 });
  assert.deepEqual(treeKeyAction("ArrowUp", 0, 3, null), { focus: 0 });
  assert.deepEqual(treeKeyAction("ArrowDown", -1, 3, null), { focus: 0 });
  assert.deepEqual(treeKeyAction("Home", 2, 3, null), { focus: 0 });
  assert.deepEqual(treeKeyAction("End", 0, 3, null), { focus: 2 });
  assert.equal(treeKeyAction("ArrowDown", 0, 0, null), null);
});

test("tree keys: Enter/Space click a row; Right opens and Left closes a header only", () => {
  assert.deepEqual(treeKeyAction("Enter", 1, 3, null), { activate: true });
  assert.deepEqual(treeKeyAction(" ", 1, 3, null), { activate: true });
  assert.equal(treeKeyAction("Enter", -1, 3, null), null);
  assert.deepEqual(treeKeyAction("ArrowRight", 1, 3, "false"), { activate: true });
  assert.equal(treeKeyAction("ArrowRight", 1, 3, "true"), null);
  assert.deepEqual(treeKeyAction("ArrowLeft", 1, 3, "true"), { activate: true });
  assert.equal(treeKeyAction("ArrowLeft", 1, 3, null), null);
  assert.equal(treeKeyAction("Delete", 1, 3, null), null);
});
