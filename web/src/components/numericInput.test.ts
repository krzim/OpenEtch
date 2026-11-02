// Unit tests for number entry on touch keyboards (numericInput.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { HOLD_DELAY_MS, HOLD_REPEAT_MS, holdDelay, numericInputMode, numericInputProps, stepValue } from "./numericInput.ts";

test("signed fields use the text keyboard, unsigned ones the decimal pad", () => {
  assert.equal(numericInputMode(true), "text", "X/Y need '-' and '/' (iOS's decimal pad has neither)");
  assert.equal(numericInputMode(false), "decimal");
  assert.deepEqual(numericInputProps(true), { inputMode: "text", enterKeyHint: "done", autoComplete: "off", spellCheck: false });
  assert.equal(numericInputProps(false).inputMode, "decimal");
});

test("steppers move by the step without float noise and stay in range", () => {
  assert.equal(stepValue(0.2, 0.1, 1), 0.3);
  assert.equal(stepValue(1, 0.1, -1), 0.9);
  assert.equal(stepValue(10, 5, 1, 0, 12), 12);
  assert.equal(stepValue(0, 1, -1, 0), 0);
  assert.equal(stepValue(null, 1, 1, 5), 6);
  assert.equal(stepValue(null, 1, -1), -1);
  assert.equal(stepValue(2.25, 0.5, 1), 2.75, "keeps the value's own decimals");
  assert.equal(stepValue(3, 0, 1), 4, "a bad step counts as 1");
});

test("press-and-hold waits, then repeats, then speeds up", () => {
  assert.equal(holdDelay(0), HOLD_DELAY_MS);
  assert.equal(holdDelay(1), HOLD_REPEAT_MS);
  assert.ok(holdDelay(20) < HOLD_REPEAT_MS);
});
