// What the canvas tells the touch HUD (touch/TouchHud.tsx): where the selection is on screen (the action bar sits
// above it), where a sheet covers the canvas (the bar hides while it covers the selection), whether it holds a group (Group or Ungroup), whether the touch handles are in rotate mode (the modifier
// bar then offers 15° / 5° steps), and whether a gesture is under way (the bars fade out of the way). A small external
// store, so pointer moves re-render only the HUD.

import { useSyncExternalStore } from "react";

export interface HudState {
  /** The selection's bounds in client px (viewport coordinates), or null with nothing selected or off screen. */
  selRect: { left: number; top: number; right: number; bottom: number } | null;
  /** The canvas's own client rect (where the action bar falls back to, and the menu's anchor). */
  canvasRect: { left: number; top: number; right: number; bottom: number } | null;
  /** The selection includes grouped objects. */
  grouped: boolean;
  /** Touch handles show the rotate handles (a tap on the selection toggles them). */
  rotateMode: boolean;
  /** A drag, pan or pinch is under way. */
  busy: boolean;
  /** The top edge (client px) of a sheet over the canvas's lower part (the phone's bottom sheet), or null. */
  coverTop: number | null;
}

const INITIAL: HudState = { selRect: null, canvasRect: null, grouped: false, rotateMode: false, busy: false, coverTop: null };
let state: HudState = INITIAL;
const listeners = new Set<() => void>();

type Rect = HudState["selRect"];
const sameRect = (a: Rect, b: Rect) =>
  a === b || (!!a && !!b && Math.abs(a.left - b.left) < 0.5 && Math.abs(a.top - b.top) < 0.5 && Math.abs(a.right - b.right) < 0.5 && Math.abs(a.bottom - b.bottom) < 0.5);

/** Update part of the HUD state (no-op updates are dropped). */
export function publishHud(patch: Partial<HudState>): void {
  const next = { ...state, ...patch };
  if (
    sameRect(next.selRect, state.selRect) &&
    sameRect(next.canvasRect, state.canvasRect) &&
    next.grouped === state.grouped &&
    next.rotateMode === state.rotateMode &&
    next.busy === state.busy &&
    (next.coverTop === state.coverTop || (next.coverTop !== null && state.coverTop !== null && Math.abs(next.coverTop - state.coverTop) < 0.5))
  )
    return;
  state = next;
  for (const l of listeners) l();
}

export function getHud(): HudState {
  return state;
}

export function subscribeHud(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function useHud(): HudState {
  return useSyncExternalStore(subscribeHud, getHud, getHud);
}
