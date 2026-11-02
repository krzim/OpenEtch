// STAGE A STUB (owner: P3). Long-press (500 ms within the touch slop, no second finger) as the touch right-click
// (docs/ux/ui-plan-2026-11.md 1.2): spread the returned handlers on a row, chip or item and open the same context menu
// as right-click from `cb`. The signature below is final; until P3 lands it returns no handlers, so nothing changes.

import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react";

/** Called at the press position (client px) with the element the handlers are on. */
export type LongPressCallback = (x: number, y: number, target: Element) => void;

/** Pointer handlers to spread on the element: `<div {...longPress}>`. */
export interface LongPressHandlers {
  onPointerDown?: (e: ReactPointerEvent<Element>) => void;
  onPointerMove?: (e: ReactPointerEvent<Element>) => void;
  onPointerUp?: (e: ReactPointerEvent<Element>) => void;
  onPointerCancel?: (e: ReactPointerEvent<Element>) => void;
  onContextMenu?: (e: ReactMouseEvent<Element>) => void;
}

const NONE: LongPressHandlers = Object.freeze({});

export function useLongPress(_cb: LongPressCallback): LongPressHandlers {
  return NONE;
}
