import assert from "node:assert/strict";
import { test } from "node:test";
import { errorText, userMessage } from "./errorText.ts";

test("errors drop the HTTP prefix and the command ids", () => {
  assert.equal(userMessage("path.dogbone: the geometry engine failed (bad ring)"), "The geometry engine failed (bad ring)");
  assert.equal(userMessage("POST /api/commands: 422 invalid input for 'path.dogbone': no inside corner to relieve"), "No inside corner to relieve");
  assert.equal(userMessage("invalid input for 'path.dogbone': diameter_mm must be > 0 and at most 100 (got 0)"), "diameter_mm must be > 0 and at most 100 (got 0)");
  assert.equal(userMessage("obj-3 is not closed; booleans need closed shapes (try path.close or path.auto_join)"), "obj-3 is not closed; booleans need closed shapes");
  assert.equal(userMessage("image.trace_many: obj-9: nothing to trace with these settings (try a higher threshold)"), "obj-9: nothing to trace with these settings (try a higher threshold)");
  // A file name is not a command id.
  assert.equal(userMessage("logo2.svg: not an SVG"), "logo2.svg: not an SVG");
  assert.equal(userMessage("engine unreachable: Failed to fetch"), "Engine unreachable: Failed to fetch");
});

test("errorText prefers an ApiError's detail", () => {
  const e = Object.assign(new Error("POST /api/commands: 422 x"), { detail: "invalid input for 'path.dogbone': no inside corner" });
  assert.equal(errorText(e), "No inside corner");
  assert.equal(errorText(new Error("path.offset: distance_mm must be finite")), "distance_mm must be finite");
  assert.equal(errorText("plain"), "Plain");
});
