// Long-press (500 ms within the touch slop, no second finger) as the touch right-click (docs/ux/ui-plan-2026-11.md
// 1.2, audit T2): spread the returned handlers on a row, chip or item and open the same context menu as right-click
// from `cb`. A mouse right-click (and a Mac Ctrl+click) calls `cb` too, so one spread serves both; the browser's own
// touch long-press menu is suppressed, and the click that follows a long-press is swallowed so the row does not also
// act on it. The timing logic is the pure `LongPressTracker` (useLongPress.test.ts).

import { useMemo, useRef } from "react";
import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react";
import { TOL } from "../pointer.ts";

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

/**
 * Run `fn` after the input events already queued: on the next animation frame (the browser dispatches pending pointer
 * moves before it), or at once where there are no frames. Returns a handle for `cancelAfterQueuedInput`.
 */
export function afterQueuedInput(fn: () => void): number {
  if (typeof requestAnimationFrame !== "function") {
    fn();
    return 0;
  }
  return requestAnimationFrame(() => fn());
}

export function cancelAfterQueuedInput(handle: number): void {
  if (handle && typeof cancelAnimationFrame === "function") cancelAnimationFrame(handle);
}

/** The long-press timing of one element: a press, moves, a lift, and when the long-press fires. Pure. */
export class LongPressTracker {
  private id: number | null = null;
  private start: [number, number] = [0, 0];
  private t0 = 0;
  /** A long-press fired for the current press (the following click must be swallowed). */
  fired = false;
  private readonly slop: number;
  readonly ms: number;
  constructor(slop = TOL.touch.slop, ms = TOL.touch.longPressMs) {
    this.slop = slop;
    this.ms = ms;
  }

  /** A touch or pen press. Returns false when a press is already under way (a second finger: both are dropped). */
  down(id: number, x: number, y: number, t: number): boolean {
    this.fired = false;
    if (this.id !== null) {
      this.id = null;
      return false;
    }
    this.id = id;
    this.start = [x, y];
    this.t0 = t;
    return true;
  }

  /** Movement past the slop cancels the long-press (it is a scroll or a drag). */
  move(id: number, x: number, y: number): void {
    if (this.id === id && Math.hypot(x - this.start[0], y - this.start[1]) > this.slop) this.id = null;
  }

  up(id: number): void {
    if (this.id === id) this.id = null;
  }

  /** The timer: whether the long-press fires now (once). */
  due(t: number): boolean {
    if (this.id === null || t - this.t0 < this.ms) return false;
    this.id = null;
    this.fired = true;
    return true;
  }

  get pending(): boolean {
    return this.id !== null;
  }

  get at(): [number, number] {
    return this.start;
  }
}

/** Long-press (touch, pen) and right-click (mouse) handlers that call `cb` with the press position. */
export function useLongPress(cb: LongPressCallback): LongPressHandlers {
  const cbRef = useRef(cb);
  cbRef.current = cb;
  return useMemo(() => {
    const tr = new LongPressTracker();
    let timer: ReturnType<typeof setTimeout> | null = null;
    let lastTouch = -Infinity;
    let pendingFrame: number | null = null;
    let detachUp: (() => void) | null = null;
    const clear = () => {
      if (timer) clearTimeout(timer);
      timer = null;
      if (pendingFrame !== null) cancelAfterQueuedInput(pendingFrame);
      pendingFrame = null;
      detachUp?.();
      detachUp = null;
    };
    /** The lift ends the press wherever it lands: a row re-rendered under the finger (a tap that selects it, the
     *  first tap of a double-tap) no longer gets its own pointerup, and the long-press then fired over the dialog
     *  the double-tap opened. */
    const watchLift = (id: number) => {
      if (typeof window === "undefined") return;
      const end = (ev: PointerEvent) => {
        if (ev.pointerId !== id) return;
        tr.up(id);
        clear();
      };
      window.addEventListener("pointerup", end, true);
      window.addEventListener("pointercancel", end, true);
      detachUp = () => {
        window.removeEventListener("pointerup", end, true);
        window.removeEventListener("pointercancel", end, true);
      };
    };
    /** Swallow the click that a lifted long-press produces (once, in the capture phase). */
    const swallowNextClick = () => {
      const stop = (ev: Event) => {
        ev.stopPropagation();
        ev.preventDefault();
      };
      window.addEventListener("click", stop, { capture: true, once: true });
      setTimeout(() => window.removeEventListener("click", stop, { capture: true }), 600);
    };
    const h: LongPressHandlers = {
      onPointerDown: (e) => {
        if (e.pointerType === "mouse") return;
        lastTouch = performance.now();
        if (!e.isPrimary) {
          tr.down(e.pointerId, e.clientX, e.clientY, e.timeStamp);
          clear();
          return;
        }
        if (!tr.down(e.pointerId, e.clientX, e.clientY, e.timeStamp)) {
          clear();
          return;
        }
        const target = e.currentTarget;
        clear();
        watchLift(e.pointerId);
        timer = setTimeout(() => {
          timer = null;
          // Decide on the next frame: pointer moves queued while the main thread was busy (a heavy document) are
          // handled before it, so a drag that started in time is not taken for a long-press.
          pendingFrame = afterQueuedInput(() => {
            pendingFrame = null;
            if (!tr.due(performance.now() + 1)) return;
            detachUp?.();
            detachUp = null;
            navigator.vibrate?.(10);
            swallowNextClick();
            const [x, y] = tr.at;
            cbRef.current(x, y, target);
          });
        }, tr.ms);
      },
      onPointerMove: (e) => {
        if (e.pointerType === "mouse") return;
        tr.move(e.pointerId, e.clientX, e.clientY);
        if (!tr.pending) clear();
      },
      onPointerUp: (e) => {
        tr.up(e.pointerId);
        clear();
      },
      onPointerCancel: (e) => {
        tr.up(e.pointerId);
        clear();
      },
      onContextMenu: (e) => {
        e.preventDefault();
        // The browser's own touch long-press menu: ours fires from the timer instead.
        const pt = (e.nativeEvent as MouseEvent & { pointerType?: string }).pointerType;
        if (e.nativeEvent.isTrusted && (pt === "touch" || pt === "pen" || performance.now() - lastTouch < 1000)) return;
        e.stopPropagation();
        cbRef.current(e.clientX, e.clientY, e.currentTarget);
      },
    };
    return h;
  }, []);
}
