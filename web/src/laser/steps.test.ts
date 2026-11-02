// Unit tests for laser/steps.ts: the Beginner Mode step bar highlights the next step.

import assert from "node:assert/strict";
import { test } from "node:test";
import { beginnerSteps } from "./steps.ts";

const base = { previewed: false, framing: false, framedRevision: null, revision: 3, running: false };

test("Preview, then Frame, then Start", () => {
  assert.equal(beginnerSteps(base).current, "preview");
  assert.equal(beginnerSteps({ ...base, previewed: true }).current, "frame");
  assert.equal(beginnerSteps({ ...base, previewed: true, framing: true }).current, "start");
  assert.equal(beginnerSteps({ ...base, previewed: true, framedRevision: 3 }).current, "start");
  assert.equal(beginnerSteps({ ...base, previewed: true, framedRevision: 3, running: true }).current, "burning");
});

test("an edit after framing asks for a new frame", () => {
  const s = beginnerSteps({ ...base, previewed: true, framedRevision: 2 });
  assert.equal(s.current, "frame");
  assert.equal(s.done.frame, false);
});

test("framing counts even before the preview, but the preview stays the next step", () => {
  const s = beginnerSteps({ ...base, framing: true });
  assert.equal(s.done.frame, true);
  assert.equal(s.current, "preview");
});
