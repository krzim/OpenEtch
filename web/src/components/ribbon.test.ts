// Unit tests for the ribbon's overflow rule (ribbonModel.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { collapseOrder, fitGroups, sameSet, type GroupFit } from "./ribbonModel.ts";

/** The Design ribbon, roughly as measured in the compact layout. */
const DESIGN: GroupFit[] = [
  { id: "create", width: 330, collapsedWidth: 70, priority: 100, pinned: true },
  { id: "modify", width: 220, collapsedWidth: 70, priority: 60 },
  { id: "arrange", width: 200, collapsedWidth: 72, priority: 80 },
  { id: "insert", width: 150, collapsedWidth: 64, priority: 40 },
  { id: "inspect", width: 170, collapsedWidth: 70, priority: 30 },
  { id: "laserTools", width: 90, collapsedWidth: 60, priority: 20 },
];

test("everything inline when it fits", () => {
  assert.equal(fitGroups(DESIGN, 1400).size, 0);
  assert.equal(fitGroups(DESIGN, 1160).size, 0, "exactly the total width");
});

test("the lowest priority collapses first, pinned never", () => {
  assert.deepEqual(collapseOrder(DESIGN), ["laserTools", "inspect", "insert", "modify", "arrange"]);
  assert.deepEqual([...fitGroups(DESIGN, 1150)], ["laserTools"]);
  assert.deepEqual([...fitGroups(DESIGN, 1030)], ["laserTools", "inspect"]);
  assert.deepEqual([...fitGroups(DESIGN, 1000)], ["laserTools", "inspect", "insert"]);
  const all = fitGroups(DESIGN, 100);
  assert.ok(!all.has("create"), "the pinned CREATE group stays");
  assert.equal(all.size, 5);
});

test("equal priorities fold from the end", () => {
  const g: GroupFit[] = [
    { id: "a", width: 100, collapsedWidth: 50, priority: 1 },
    { id: "b", width: 100, collapsedWidth: 50, priority: 1 },
  ];
  assert.deepEqual([...fitGroups(g, 160)], ["b"]);
});

test("the collapsed set only grows as the width shrinks", () => {
  let prev = new Set<string>();
  for (let w = 1400; w >= 0; w -= 7) {
    const cur = fitGroups(DESIGN, w);
    for (const id of prev) assert.ok(cur.has(id), `${id} came back at ${w}px`);
    prev = cur;
  }
});

test("1440 keeps more inline than 1280 (CREATE and ARRANGE stay)", () => {
  // Available ribbon width after the end caps, roughly, at 1440 and 1280 px windows in the compact layout.
  const at1440 = fitGroups(DESIGN, 1440 - 330);
  const at1280 = fitGroups(DESIGN, 1280 - 330);
  assert.ok(at1440.size < at1280.size);
  assert.ok(!at1440.has("create") && !at1440.has("arrange"));
});

test("sameSet", () => {
  assert.ok(sameSet(new Set(["a", "b"]), new Set(["b", "a"])));
  assert.ok(!sameSet(new Set(["a"]), new Set(["a", "b"])));
  assert.ok(!sameSet(new Set(["a"]), new Set(["b"])));
});
