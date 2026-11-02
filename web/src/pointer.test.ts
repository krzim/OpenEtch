// Unit tests for pointer.ts: pointer capture never throws (verify finding 4: the timeline marker's unguarded
// setPointerCapture threw "No active pointer" and the drag never started).

import assert from "node:assert/strict";
import { test } from "node:test";
import { TOL, capturePointer, handleFactor, handleHitPx, isApplePlatform, isMacContextClick, modifierReplay, modsOf, pointerClass, tolOf } from "./pointer.ts";

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

test("a modifier key re-evaluates the drag, else the hover, so the Shift guide follows the key without a pointer move", () => {
  assert.equal(modifierReplay("Shift", "move", false), "drag");
  assert.equal(modifierReplay("Control", "ext", true), "drag", "a drag wins over the hover");
  assert.equal(modifierReplay("Shift", "none", true), "hover", "line tool hovering: releasing Shift drops the guide");
  assert.equal(modifierReplay("Alt", "none", false), null, "pointer off the canvas");
  assert.equal(modifierReplay("Shift", "idle", true), null, "a cancelled drag waits for the button to come up");
  assert.equal(modifierReplay("a", "move", true), null, "other keys do nothing");
});

// ---- pointer classes, the tolerance table and the modifier mapping (docs/ux/ui-plan-2026-11.md 1.2) ----------------

test("pointerClass: touch, pen, and everything else is a mouse", () => {
  assert.equal(pointerClass({ pointerType: "touch" }), "touch");
  assert.equal(pointerClass({ pointerType: "pen" }), "pen");
  assert.equal(pointerClass({ pointerType: "mouse" }), "mouse");
  assert.equal(pointerClass({ pointerType: "" }), "mouse", "synthetic events");
  assert.equal(pointerClass({}), "mouse");
  assert.equal(pointerClass(null), "mouse");
});

test("TOL: the mouse row keeps the old constants; touch is larger; pen sits between", () => {
  assert.deepEqual(
    { click: TOL.mouse.click, slop: TOL.mouse.slop, node: TOL.mouse.node, snap: TOL.mouse.snap, handle: TOL.mouse.handle, dbl: TOL.mouse.dblMs, dd: TOL.mouse.dblDist },
    { click: 5, slop: 4, node: 7, snap: 8, handle: 16, dbl: 350, dd: 6 },
    "Viewport TAP_SLOP/CLICK_TOL, EditTools SLOP/NODE_TOL/DBL_MS, snapping SNAP_TOL_PX",
  );
  assert.deepEqual(
    [TOL.touch.click, TOL.touch.slop, TOL.touch.node, TOL.touch.snap, TOL.touch.handle, TOL.touch.dblDist, TOL.touch.dblMs, TOL.touch.longPressMs],
    [14, 10, 18, 14, 44, 24, 400, 500],
    "the plan's touch values",
  );
  assert.deepEqual([TOL.pen.click, TOL.pen.slop, TOL.pen.node, TOL.pen.snap, TOL.pen.handle], [8, 6, 10, 10, 24], "the plan's pen values");
  for (const k of ["click", "slop", "node", "snap", "handle", "dblDist"] as const) {
    assert.ok(TOL.mouse[k] <= TOL.pen[k] && TOL.pen[k] <= TOL.touch[k], k);
  }
  assert.equal(tolOf({ pointerType: "touch" }), TOL.touch);
  assert.equal(handleHitPx("touch"), 44, "touch handles hit at 44 px");
  assert.equal(handleFactor("mouse"), 1);
  assert.ok(handleFactor("touch") >= 2, "touch handles are drawn at about 2x");
});

test("isApplePlatform from platform, userAgentData or the user agent", () => {
  assert.equal(isApplePlatform({ platform: "MacIntel" }), true);
  assert.equal(isApplePlatform({ platform: "iPhone" }), true);
  assert.equal(isApplePlatform({ userAgentData: { platform: "macOS" } }), true);
  assert.equal(isApplePlatform({ platform: "", userAgent: "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X)" }), true);
  assert.equal(isApplePlatform({ platform: "Win32", userAgent: "Mozilla/5.0 (Windows NT 10.0)" }), false);
  assert.equal(isApplePlatform({ platform: "Linux x86_64" }), false);
  assert.equal(isApplePlatform(undefined), false);
});

test("modsOf: physical OR virtual; Cmd is Ctrl on a Mac, where Ctrl alone is the right-click", () => {
  const k = (o: Partial<{ shiftKey: boolean; ctrlKey: boolean; metaKey: boolean; altKey: boolean }> = {}) => ({ shiftKey: false, ctrlKey: false, metaKey: false, altKey: false, ...o });
  const none = { shift: false, ctrl: false, alt: false };
  assert.deepEqual(modsOf(k(), none, false), none);
  assert.deepEqual(modsOf(k({ ctrlKey: true }), none, false), { shift: false, ctrl: true, alt: false }, "Windows Ctrl");
  assert.deepEqual(modsOf(k({ metaKey: true }), none, false), { shift: false, ctrl: true, alt: false }, "Windows keeps Meta as before");
  assert.deepEqual(modsOf(k({ ctrlKey: true }), none, true), none, "Mac: Ctrl+click is the context menu, not a toggle");
  assert.deepEqual(modsOf(k({ metaKey: true }), none, true), { shift: false, ctrl: true, alt: false }, "Mac: Cmd");
  assert.deepEqual(modsOf(k({ shiftKey: true }), { shift: false, ctrl: true, alt: false }, false), { shift: true, ctrl: true, alt: false }, "OR");
  assert.deepEqual(modsOf(k(), { shift: true, ctrl: false, alt: true }, true), { shift: true, ctrl: false, alt: true }, "virtual only");
  const mac = { pointerType: "mouse", button: 0, ctrlKey: true, metaKey: false };
  assert.equal(isMacContextClick(mac, true), true);
  assert.equal(isMacContextClick(mac, false), false, "Windows Ctrl+click toggles");
  assert.equal(isMacContextClick({ ...mac, pointerType: "touch" }, true), false);
  assert.equal(isMacContextClick({ ...mac, metaKey: true }, true), false, "Cmd+Ctrl+click: Cmd wins");
});
