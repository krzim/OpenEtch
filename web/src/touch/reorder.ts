// Pointer-event reordering of list rows (docs/ux/ui-plan-2026-11.md 1.2, audit T13): a drag grip on each row moves it
// up or down the list the same way with a mouse, a pen or a finger. HTML5 drag-and-drop does not work on many touch
// browsers and, on iPad, starts a system drag on long-press that fights the long-press menu. `reorderTarget` is pure
// (reorder.test.ts); `useReorder` wires it to a grip's pointer events and the rows' rects.

import { useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, PointerEvent as ReactPointerEvent } from "react";
import { capturePointer } from "../pointer.ts";

/** Where a dragged row would drop: before or after row `index`. */
export interface DropAt {
  index: number;
  after: boolean;
}

/** The row under client y (the nearest when y is above or below the list), and which half of it. */
export function reorderTarget(y: number, rows: readonly { top: number; bottom: number }[]): DropAt | null {
  if (!rows.length) return null;
  let best = 0;
  let bestD = Infinity;
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    const d = y < r.top ? r.top - y : y > r.bottom ? y - r.bottom : 0;
    if (d < bestD) {
      bestD = d;
      best = i;
    }
  }
  const r = rows[best];
  return { index: best, after: y > (r.top + r.bottom) / 2 };
}

/** The new index of row `from` dropped at `at` among `count` rows, or null when it stays where it is. */
export function reorderIndex(from: number, at: DropAt, count: number): number | null {
  if (!(from >= 0 && from < count && at.index >= 0 && at.index < count)) return null;
  const slot = at.index + (at.after ? 1 : 0);
  const to = slot > from ? slot - 1 : slot;
  return to === from ? null : to;
}

export interface ReorderGrip {
  onPointerDown: (e: ReactPointerEvent<HTMLElement>) => void;
  onPointerMove: (e: ReactPointerEvent<HTMLElement>) => void;
  onPointerUp: (e: ReactPointerEvent<HTMLElement>) => void;
  onPointerCancel: (e: ReactPointerEvent<HTMLElement>) => void;
  /** Arrow keys move the row one step (the grip is a button). */
  onKeyDown: (e: ReactKeyboardEvent<HTMLElement>) => void;
}

/**
 * Reordering for a list whose rows carry `data-reorder-row` (in list order) inside `container`. `move(from, to)`
 * commits a move. Returns the drop indicator to draw and the grip handlers for row `index`.
 */
export function useReorder(container: { current: HTMLElement | null }, count: number, move: (from: number, to: number) => void) {
  const [dragging, setDragging] = useState<number | null>(null);
  const [dropAt, setDropAt] = useState<DropAt | null>(null);
  const live = useRef<{ id: number; from: number; at: DropAt | null } | null>(null);
  const moveRef = useRef(move);
  moveRef.current = move;
  const rects = () => [...(container.current?.querySelectorAll<HTMLElement>("[data-reorder-row]") ?? [])].map((el) => el.getBoundingClientRect());
  const end = (commit: boolean) => {
    const l = live.current;
    live.current = null;
    setDragging(null);
    setDropAt(null);
    if (!commit || !l?.at) return;
    const to = reorderIndex(l.from, l.at, count);
    if (to !== null) moveRef.current(l.from, to);
  };
  const grip = (index: number): ReorderGrip => ({
    onPointerDown: (e) => {
      if (e.pointerType === "mouse" && e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();
      capturePointer(e.currentTarget, e.pointerId);
      live.current = { id: e.pointerId, from: index, at: null };
      setDragging(index);
    },
    onPointerMove: (e) => {
      const l = live.current;
      if (!l || l.id !== e.pointerId) return;
      e.preventDefault();
      const at = reorderTarget(e.clientY, rects());
      l.at = at;
      setDropAt((d) => (d && at && d.index === at.index && d.after === at.after ? d : at));
    },
    onPointerUp: (e) => {
      if (live.current?.id !== e.pointerId) return;
      e.stopPropagation();
      end(true);
    },
    onPointerCancel: (e) => {
      if (live.current?.id === e.pointerId) end(false);
    },
    onKeyDown: (e) => {
      const step = e.key === "ArrowUp" ? -1 : e.key === "ArrowDown" ? 1 : 0;
      if (!step) return;
      e.preventDefault();
      e.stopPropagation();
      const to = index + step;
      if (to >= 0 && to < count) moveRef.current(index, to);
    },
  });
  /** The row class for the drop indicator ("drop-before" / "drop-after"), or "". */
  const dropClass = (index: number) => (dropAt && dropAt.index === index && dragging !== null ? (dropAt.after ? " drop-after" : " drop-before") : "");
  return { dragging, dropAt, grip, dropClass };
}
