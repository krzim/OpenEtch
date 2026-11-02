// Unit tests for where the canvas letter keys act (keyFocus.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { canvasKeysFor, type FocusLike } from "./keyFocus.ts";

const el = (tagName: string, inside: string[]): FocusLike => ({
  tagName,
  closest: (sel) => (sel.split(",").some((s) => inside.includes(s.trim())) ? {} : null),
});

test("canvasKeysFor: canvas, body and nothing take the letter keys", () => {
  const body = el("BODY", []);
  assert.equal(canvasKeysFor(null, body, null), true);
  assert.equal(canvasKeysFor(body, body, null), true);
  assert.equal(canvasKeysFor(el("svg", [".viewport"]), body, null), true);
});

test("canvasKeysFor: a chrome button pressed with the pointer hands the letters back to the canvas", () => {
  // Seen in the app: after clicking the Ellipse tool, S no longer opened command search.
  for (const row of [".toolbar", ".tool-rail", ".topbar", ".bottom-row", ".palette-bar"]) {
    const b = el("BUTTON", [row]);
    assert.equal(canvasKeysFor(b, null, b), true, row);
  }
});

test("canvasKeysFor: keyboard focus, fields and panel buttons keep their keys", () => {
  const b = el("BUTTON", [".toolbar"]);
  assert.equal(canvasKeysFor(b, null, null), false, "reached with Tab: no pointer press on it");
  assert.equal(canvasKeysFor(b, null, el("BUTTON", [".toolbar"])), false, "the press was on another button");
  const input = el("INPUT", [".toolbar"]);
  assert.equal(canvasKeysFor(input, null, input), false);
  const tree = el("BUTTON", [".panel"]);
  assert.equal(canvasKeysFor(tree, null, tree), false, "the Browser tree and the docks keep their keys");
});
