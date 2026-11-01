// Pointer capture that never throws. `setPointerCapture` throws "NotFoundError: No active pointer" when the pointer is
// already gone (a touch that ended, a synthetic event, a pen lifted between events); a drag must still start then, it
// just is not captured. Also which modifier key events re-evaluate the drag or hover (modifierReplay). No React, so
// `npm test` covers it.

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

/** Modifier keys whose press or release changes what the pointer does (Shift constrains, Ctrl and Alt snapping). */
const MOD_KEYS: readonly string[] = ["Shift", "Control", "Alt", "Meta"];

/**
 * What a press or release of `key` re-evaluates at the last pointer position: the drag in progress (`gesture` is the
 * canvas gesture's kind), the hover of a tool when no button is down (the line tool's Shift angle and its guide follow
 * Shift without a pointer move), or nothing.
 */
export function modifierReplay(key: string, gesture: string, hovering: boolean): "drag" | "hover" | null {
  if (!MOD_KEYS.includes(key)) return null;
  if (gesture !== "none" && gesture !== "idle") return "drag";
  return hovering && gesture === "none" ? "hover" : null;
}
