// The contact sheet's model (contactSheet.ts): the engine's answer checked, pages appended, a card's kinds and title.

import assert from "node:assert/strict";
import { test } from "node:test";
import { appendPage, parseSheet, rowKinds, rowTitle, sheetRequest } from "./contactSheet.ts";

const ANSWER = {
  from: 0,
  next: 2,
  total: 4,
  step: 1,
  start: 0,
  end: 3,
  targets: [{ id: "t", name: "Name", barcode: false, mode: "csv", container: { kind: "object", id: "r", name: "Tag" } }],
  rows: [
    { value: 0, problems: 0, texts: [{ id: "t", shown: "Ann", flags: [], d: "M0 0L1 0" }] },
    {
      value: 1,
      problems: 2,
      texts: [
        {
          id: "t",
          shown: "Maximilian\nx",
          flags: [
            { kind: "shrunk", message: "small" },
            { kind: "overflow", message: "runs outside Tag" },
          ],
        },
      ],
    },
  ],
  summary: { rows: 2, problems: { overflow: 1, shrunk: 1 } },
  base: [{ layer: "l", color: "#f00", fill: true, d: "M0 0Z" }, { d: 5 }],
  stock: "M1 1Z",
  view_box: [-10, -5, 20, 10],
};

test("the answer is checked into the model the grid draws", () => {
  const s = parseSheet(ANSWER);
  assert.equal(s.rows.length, 2);
  assert.equal(s.targets[0].container, "Tag");
  assert.deepEqual(s.base, [{ color: "#f00", fill: true, tool: false, d: "M0 0Z" }]);
  assert.deepEqual(s.viewBox, [-10, -5, 20, 10]);
  assert.deepEqual(s.problems, { overflow: 1, shrunk: 1 });
  assert.equal(s.next, 2);
  assert.deepEqual(rowKinds(s.rows[1]), ["overflow", "shrunk"], "most serious first");
  assert.equal(rowTitle(s.rows[1]), "Maximilian");
  assert.deepEqual(parseSheet(null).rows, []);
});

test("pages add up and the request asks for outlines", () => {
  const a = parseSheet(ANSWER);
  const b = parseSheet({
    ...ANSWER,
    next: null,
    rows: [{ value: 2, problems: 1, texts: [{ id: "t", shown: "", flags: [{ kind: "empty", message: "x" }] }] }],
    summary: { problems: { empty: 1 } },
    view_box: [-12, -5, 20, 12],
  });
  const all = appendPage(a, b);
  assert.deepEqual(
    all.rows.map((r) => r.value),
    [0, 1, 2],
  );
  assert.equal(all.next, null);
  assert.deepEqual(all.problems, { overflow: 1, shrunk: 1, empty: 1 });
  assert.deepEqual(all.viewBox, [-12, -5, 22, 12]);
  assert.deepEqual(sheetRequest(null, true), { id: "var.contact_sheet", params: { count: 48, outlines: true, problems_only: true } });
  assert.deepEqual(sheetRequest(7, false, 10).params, { count: 10, outlines: true, problems_only: false, from: 7 });
});
