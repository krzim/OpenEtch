// Unit tests for pointer.ts: pointer capture never throws (verify finding 4: the timeline marker's unguarded
// setPointerCapture threw "No active pointer" and the drag never started).

import assert from "node:assert/strict";
import { test } from "node:test";
import { capturePointer } from "./pointer.ts";

test("capture that throws is swallowed and reported as not taken", () => {
  const el = {
    setPointerCapture: (_id: number) => {
      throw new Error("NotFoundError: No active pointer with the given id is found.");
    },
  };
  assert.doesNotThrow(() => capturePointer(el, 7));
  assert.equal(capturePointer(el, 7), false);
});

test("capture works when it can, and without an element or the method", () => {
  const got: number[] = [];
  assert.equal(capturePointer({ setPointerCapture: (id: number) => void got.push(id) }, 3), true);
  assert.deepEqual(got, [3]);
  assert.equal(capturePointer({}, 1), false);
  assert.equal(capturePointer(null, 1), false);
  assert.equal(capturePointer(undefined, 1), false);
});
