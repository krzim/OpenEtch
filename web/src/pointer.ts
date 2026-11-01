// Pointer capture that never throws. `setPointerCapture` throws "NotFoundError: No active pointer" when the pointer is
// already gone (a touch that ended, a synthetic event, a pen lifted between events); a drag must still start then, it
// just is not captured. No React, so `npm test` covers it.

/** Something that can capture a pointer (an Element). */
export interface PointerCapturer {
  setPointerCapture?: (pointerId: number) => void;
}

/** Capture `pointerId` on `el` if possible. Returns whether the capture took; never throws. */
export function capturePointer(el: PointerCapturer | null | undefined, pointerId: number): boolean {
  if (!el || typeof el.setPointerCapture !== "function") return false;
  try {
    el.setPointerCapture(pointerId);
    return true;
  } catch {
    // No active pointer: the drag still works while the pointer stays over the element.
    return false;
  }
}
