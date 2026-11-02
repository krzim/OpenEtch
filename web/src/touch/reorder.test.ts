// Unit tests for touch/reorder.ts: the pointer-event row reorder (audit T13) finds the row under the pointer and the
// new index the same way the old HTML5 drop did (workspace/layersTable.ts dropIndex).

import assert from "node:assert/strict";
import { test } from "node:test";
import { dropIndex } from "../workspace/layersTable.ts";
import { reorderIndex, reorderTarget, reorderTargetRow } from "./reorder.ts";

const rows = [
  { top: 0, bottom: 40 },
  { top: 40, bottom: 80 },
  { top: 80, bottom: 120 },
];

test("reorderTarget: the row under the pointer and its half; outside the list, the nearest row", () => {
  assert.deepEqual(reorderTarget(10, rows), { index: 0, after: false });
  assert.deepEqual(reorderTarget(70, rows), { index: 1, after: true });
  assert.deepEqual(reorderTarget(-50, rows), { index: 0, after: false }, "above the list");
  assert.deepEqual(reorderTarget(500, rows), { index: 2, after: true }, "below the list");
  assert.equal(reorderTarget(10, []), null);
});

test("reorderIndex matches the HTML5 drop's dropIndex", () => {
  for (let from = 0; from < 3; from++)
    for (let over = 0; over < 3; over++)
      for (const after of [false, true]) assert.equal(reorderIndex(from, { index: over, after }, 3), dropIndex(from, over, after, 3), `${from} ${over} ${after}`);
  assert.equal(reorderIndex(0, { index: 2, after: true }, 3), 2, "to the end");
  assert.equal(reorderIndex(2, { index: 0, after: false }, 3), 0, "to the top");
  assert.equal(reorderIndex(1, { index: 1, after: true }, 3), null, "onto itself");
  assert.equal(reorderIndex(5, { index: 0, after: false }, 3), null);
});

test("reorderTargetRow: the tab under the pointer along a row of tabs that wraps, and its half", () => {
  // Sub-layer tabs: two on the first line, one on the second.
  const tabs = [
    { left: 0, right: 60, top: 0, bottom: 30 },
    { left: 64, right: 130, top: 0, bottom: 30 },
    { left: 0, right: 70, top: 32, bottom: 62 },
  ];
  assert.deepEqual(reorderTargetRow(10, 15, tabs), { index: 0, after: false });
  assert.deepEqual(reorderTargetRow(120, 15, tabs), { index: 1, after: true });
  assert.deepEqual(reorderTargetRow(50, 45, tabs), { index: 2, after: true }, "second line");
  assert.deepEqual(reorderTargetRow(300, 10, tabs), { index: 1, after: true }, "right of the first line stays on it");
  assert.deepEqual(reorderTargetRow(-20, 50, tabs), { index: 2, after: false }, "left of the second line");
  assert.equal(reorderTargetRow(0, 0, []), null);
  // Dragging the first sub-layer after the third moves it to the end (the engine's layer.sub_move to-index).
  assert.equal(reorderIndex(0, reorderTargetRow(60, 45, tabs)!, 3), 2);
});
