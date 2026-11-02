// Wheel and trackpad intent (docs/ux/ui-plan-2026-11.md 1.2, audit T6): whether a wheel event pans or zooms the
// canvas. The decision is latched per wheel stream (a stream ends after 150 ms without events), so a two-finger swipe
// never flips between pan and zoom halfway. Pure, so `npm test` covers it (wheelIntent.test.ts).
//
// - Ctrl+wheel (a trackpad pinch in Chrome and Edge, or Ctrl held with a mouse wheel) always zooms.
// - Shift+wheel pans sideways.
// - With "Trackpad: scroll pans, pinch zooms" on (Preferences; Auto = on for Apple platforms and once a trackpad has
//   been seen), pixel-mode scrolling pans, except a classic mouse wheel, which keeps zooming (LightBurn).
// - Otherwise sideways scrolling pans (as before) and the wheel zooms.

export type WheelIntent = "pan" | "zoom";

export interface WheelLike {
  deltaX: number;
  deltaY: number;
  /** 0 pixels, 1 lines, 2 pages. */
  deltaMode: number;
  ctrlKey: boolean;
  shiftKey: boolean;
  /** Non-standard but in Chrome and Safari: ±120 steps for a mouse wheel, -3 x deltaY for a trackpad. */
  wheelDeltaY?: number;
  /** Event time, ms. */
  timeStamp: number;
}

export interface WheelLatch {
  intent: WheelIntent | null;
  /** Time of the stream's last event. */
  last: number;
  /** A trackpad has been seen (fractional or two-axis pixel deltas): Auto turns trackpad panning on. */
  trackpadSeen: boolean;
}

export const WHEEL_STREAM_IDLE_MS = 150;

export function newWheelLatch(): WheelLatch {
  return { intent: null, last: -Infinity, trackpadSeen: false };
}

/** Whether an event looks like a notched mouse wheel rather than a trackpad. */
export function looksLikeMouseWheel(e: WheelLike): boolean {
  if (e.deltaMode !== 0) return true;
  if (e.deltaX !== 0) return false;
  if (!Number.isInteger(e.deltaY)) return false;
  if (typeof e.wheelDeltaY === "number" && e.wheelDeltaY !== 0) return e.wheelDeltaY % 120 === 0;
  // Without wheelDelta (Firefox): Windows and Linux mouse wheels step by 100 / 120 / 53 or more px.
  return Math.abs(e.deltaY) >= 50;
}

/** Whether the pixel deltas show a trackpad (fractional, or both axes at once). */
export function looksLikeTrackpad(e: WheelLike): boolean {
  return e.deltaMode === 0 && (!Number.isInteger(e.deltaX) || !Number.isInteger(e.deltaY) || (e.deltaX !== 0 && e.deltaY !== 0));
}

/**
 * Decide what `e` does, and update `latch` (mutated) for the stream. `pref` is Preferences > Trackpad (auto/on/off);
 * `apple` the platform.
 */
export function wheelIntent(latch: WheelLatch, e: WheelLike, pref: "auto" | "on" | "off", apple: boolean): WheelIntent {
  if (looksLikeTrackpad(e)) latch.trackpadSeen = true;
  const fresh = !(e.timeStamp - latch.last < WHEEL_STREAM_IDLE_MS) || latch.intent === null;
  latch.last = e.timeStamp;
  let intent: WheelIntent;
  if (e.ctrlKey) intent = "zoom";
  else if (!fresh && latch.intent) intent = latch.intent;
  else if (e.shiftKey) intent = "pan";
  else {
    const trackpadPans = pref === "on" || (pref === "auto" && (apple || latch.trackpadSeen));
    if (trackpadPans && !looksLikeMouseWheel(e)) intent = "pan";
    else intent = e.deltaMode === 0 && e.deltaX !== 0 ? "pan" : "zoom";
  }
  // A pinch (Ctrl) inside a pan stream does not re-latch it: the swipe after the pinch keeps its own decision.
  if (!e.ctrlKey || fresh) latch.intent = intent;
  return intent;
}

/** Screen px a wheel event pans by (lines count 40 px; Shift turns a vertical wheel sideways). */
export function wheelPanDelta(e: WheelLike): [number, number] {
  const k = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 400 : 1;
  if (e.shiftKey && e.deltaX === 0) return [e.deltaY * k, 0];
  return [e.deltaX * k, e.deltaY * k];
}
