// The phone's bottom sheet (docs/ux/ui-plan-2026-11.md 1.1, P1): a panel that rises from the bottom tab bar to one of
// three heights (peek about 30 % of the space under the top bar, half, full), with a drag handle (touch-action none,
// so a drag on it never scrolls or zooms the page). The handle is a button: a tap or Enter cycles the heights, Arrow
// Up / Down change them, and Esc closes the sheet (unless a dialog is open over it). It never covers the top bar, so
// STOP stays in view and hit-testable.

import { useEffect, useRef, useState, type KeyboardEvent as RKeyboardEvent, type PointerEvent as RPointerEvent, type ReactNode } from "react";
import { Icon } from "../components/ui.tsx";
import { escapeTakenElsewhere } from "./DockHost.tsx";
import { SHEET_SNAPS, snapFor, type SheetSnap } from "./phone.ts";
import { useT } from "./usePrefs.ts";
import "./phone.css";

export type { SheetSnap };

export interface BottomSheetProps {
  open: boolean;
  snap: SheetSnap;
  onSnap: (s: SheetSnap) => void;
  title: string;
  children: ReactNode;
  /** Closing (Esc, the close button, dragging it down); without it the sheet cannot be closed by the user. */
  onClose?: () => void;
}

export function BottomSheet(p: BottomSheetProps) {
  const t = useT();
  const root = useRef<HTMLElement>(null);
  const drag = useRef<{ y: number; h: number; space: number } | null>(null);
  const [live, setLive] = useState<number | null>(null);
  /** Set when a drag ended: the click that follows must not also cycle the height. */
  const moved = useRef(false);
  const { onClose, open } = p;

  useEffect(() => {
    if (!open || !onClose) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || escapeTakenElsewhere(e)) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);

  if (!p.open) return null;

  const space = () => root.current?.parentElement?.getBoundingClientRect().height ?? window.innerHeight;
  const onDown = (e: RPointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0 || !root.current) return;
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // The pointer is already gone.
    }
    drag.current = { y: e.clientY, h: root.current.getBoundingClientRect().height, space: space() };
  };
  const onMove = (e: RPointerEvent<HTMLButtonElement>) => {
    const d = drag.current;
    if (!d) return;
    const h = Math.max(24, Math.min(d.space, d.h + (d.y - e.clientY)));
    if (Math.abs(e.clientY - d.y) > 4 || live !== null) setLive(h);
  };
  const onUp = () => {
    const d = drag.current;
    drag.current = null;
    if (!d || live === null) return;
    const next = snapFor(live / d.space);
    setLive(null);
    moved.current = true;
    if (next === null) onClose?.();
    else p.onSnap(next);
  };
  const cycle = () => {
    if (moved.current) {
      moved.current = false;
      return;
    }
    const i = SHEET_SNAPS.indexOf(p.snap);
    p.onSnap(SHEET_SNAPS[(i + 1) % SHEET_SNAPS.length]);
  };
  const onKey = (e: RKeyboardEvent<HTMLButtonElement>) => {
    const i = SHEET_SNAPS.indexOf(p.snap);
    if (e.key === "ArrowUp") {
      e.preventDefault();
      p.onSnap(SHEET_SNAPS[Math.min(SHEET_SNAPS.length - 1, i + 1)]);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      if (i === 0) onClose?.();
      else p.onSnap(SHEET_SNAPS[i - 1]);
    }
  };
  return (
    <section
      ref={root}
      className={`bottom-sheet snap-${p.snap}${live !== null ? " dragging" : ""}`}
      style={live !== null ? { height: live } : undefined}
      role="dialog"
      aria-modal="false"
      aria-label={p.title}
    >
      <div className="sheet-head">
        <button
          type="button"
          className="sheet-handle"
          aria-label={t("shell.sheetResize", { title: p.title })}
          title={t("shell.sheetResize", { title: p.title })}
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerCancel={onUp}
          onClick={cycle}
          onKeyDown={onKey}
        >
          <span className="sheet-grip" aria-hidden="true" />
        </button>
        <span className="sheet-title">{p.title}</span>
        {p.onClose && (
          <button type="button" className="icon-btn sheet-close" aria-label={t("common.close")} title={t("common.close")} onClick={p.onClose}>
            <Icon name="close" />
          </button>
        )}
      </div>
      <div className="sheet-body">{p.children}</div>
    </section>
  );
}
