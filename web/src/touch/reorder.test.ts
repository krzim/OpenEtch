// Unit tests for touch/reorder.ts: the pointer-event row reorder (audit T13) finds the row under the pointer and the
// new index the same way the old HTML5 drop did (workspace/layersTable.ts dropIndex).

import assert from "node:assert/strict";
import { test } from "node:test";
import { dropIndex } from "../workspace/layersTable.ts";
import { reorderIndex, reorderTarget } from "./reorder.ts";

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
