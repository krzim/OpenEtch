// The one dialog primitive (docs/ux/ui-plan-2026-11.md P2 "Dialog"); ui.tsx `Modal` is an alias of it.
// - One stack with the menus (dialogStack.ts): Esc and Enter reach only the top dialog.
// - Focus: kept inside while it is open (Tab cycles), put on the first field (or an autoFocus element) when it opens,
//   given back to what had it when it closes. Named by its title (aria-labelledby); the Close button is translated.
// - The backdrop closes it on a click that started and ended on the backdrop, and only for dialogs without a form
//   (`dismissable`); with unsaved edits (`dirty`) closing asks "Discard changes?" first.
// - Sizes sm 360 / md 480 / lg 720 / xl 960 / full. At 600 px wide and below (and in the phone layout) it is a
//   full-screen sheet under the top bar, footer pinned, lifted above the on-screen keyboard (visualViewport).
// - It sits at --z-modal, below --z-stop: STOP in the top bar stays visible and clickable above every dialog.

import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useT } from "../workspace/usePrefs.ts";
import { installEscape, layers } from "./dialogStack.ts";
import { Icon } from "./ui.tsx";
import "./dialog.css";

/** Today's ui.tsx Modal props. */
export interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
  onSubmit?: () => void;
}

/** sm 360 / md 480 / lg 720 / xl 960 px wide, or full screen. */
export type DialogSize = "sm" | "md" | "lg" | "xl" | "full";

export interface DialogProps extends ModalProps {
  size?: DialogSize;
  /** A press on the backdrop closes it (default: true for dialogs without a form, false for form dialogs). */
  dismissable?: boolean;
  /** Unsaved edits: closing asks "Discard changes?" first. */
  dirty?: boolean;
}

const FOCUSABLE = 'a[href],button:not([disabled]),input:not([disabled]):not([type="hidden"]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"]),[contenteditable="true"]';
const FIELD = 'input:not([disabled]):not([type="hidden"]):not([type="checkbox"]):not([type="radio"]):not([type="button"]),select:not([disabled]),textarea:not([disabled])';

function visible(el: HTMLElement): boolean {
  return !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length);
}

/** Whether the dialog holds a form (backdrop presses then never close it). */
function hasForm(root: HTMLElement | null): boolean {
  return !!root?.querySelector(FIELD);
}

export function Dialog(p: DialogProps) {
  const t = useT();
  const size: DialogSize = p.size ?? (p.wide ? "xl" : "md");
  const titleId = useId();
  const backdrop = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const [confirm, setConfirm] = useState(false);
  const P = useRef(p);
  P.current = p;
  const confirmRef = useRef(confirm);
  confirmRef.current = confirm;
  // Enter first blurs the field so a commit-on-blur field (NumberField, TextField) sets its value, then submits on the
  // next tick with the re-rendered dialog's `onSubmit`, which sees that value (calling the old one sent stale state).
  const alive = useRef(true);
  const opener = useRef<Element | null>(typeof document === "undefined" ? null : document.activeElement);
  const downOnBackdrop = useRef(false);

  const requestClose = () => {
    if (P.current.dirty && !confirmRef.current) setConfirm(true);
    else P.current.onClose();
  };

  // On the Esc stack while mounted; Esc first dismisses the "Discard changes?" question.
  useEffect(() => {
    alive.current = true;
    installEscape();
    const id = layers.push("dialog", () => {
      if (confirmRef.current) setConfirm(false);
      else requestClose();
    });
    const prev = opener.current;
    return () => {
      alive.current = false;
      layers.remove(id);
      // Give the focus back, unless something else (another dialog, a field elsewhere) has taken it.
      const a = document.activeElement;
      if (prev instanceof HTMLElement && prev.isConnected && (!a || a === document.body || !a.isConnected || box.current?.contains(a))) {
        prev.focus({ preventScroll: true });
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Initial focus: an autoFocus element keeps it; else the first field, else the footer's primary button.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el || el.contains(document.activeElement)) return;
    const body = el.querySelector<HTMLElement>(".modal-body");
    const field = [...(body?.querySelectorAll<HTMLElement>(FIELD) ?? [])].find(visible);
    const primary = el.querySelector<HTMLElement>(".modal-foot .btn-primary:not([disabled]), .modal-foot .btn-warn:not([disabled])");
    (field ?? primary ?? el).focus({ preventScroll: true });
  }, []);

  // The on-screen keyboard: keep the sheet's footer above it.
  useEffect(() => {
    const vv = window.visualViewport;
    const el = backdrop.current;
    if (!vv || !el) return;
    const update = () => el.style.setProperty("--vv-inset", `${Math.max(0, Math.round(window.innerHeight - vv.height - vv.offsetTop))}px`);
    update();
    vv.addEventListener("resize", update);
    vv.addEventListener("scroll", update);
    return () => {
      vv.removeEventListener("resize", update);
      vv.removeEventListener("scroll", update);
    };
  }, []);

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const el = box.current;
    if (!el) return;
    if (e.key === "Tab") {
      // Focus trap: Tab and Shift+Tab cycle inside the dialog.
      const list = [...el.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(visible);
      if (!list.length) {
        e.preventDefault();
        return;
      }
      const first = list[0];
      const last = list[list.length - 1];
      const a = document.activeElement;
      if (e.shiftKey && (a === first || !el.contains(a))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (a === last || !el.contains(a))) {
        e.preventDefault();
        first.focus();
      }
      return;
    }
    const target = e.target as HTMLElement;
    if (e.key === "Enter" && P.current.onSubmit && target.tagName === "INPUT" && (target as HTMLInputElement).type !== "checkbox" && !confirmRef.current) {
      e.preventDefault();
      target.blur();
      window.setTimeout(() => {
        if (alive.current) P.current.onSubmit?.();
      }, 0);
    }
  };

  const dismissable = () => P.current.dismissable ?? !(P.current.onSubmit || hasForm(box.current));

  return (
    <div
      ref={backdrop}
      className={`modal-backdrop oe-dialog-backdrop oe-dialog-backdrop-${size}`}
      onPointerDown={(e) => {
        downOnBackdrop.current = e.target === e.currentTarget;
      }}
      onClick={(e) => {
        // Only a click that started and ended on the backdrop (not a text selection dragged out of a field).
        const both = downOnBackdrop.current && e.target === e.currentTarget;
        downOnBackdrop.current = false;
        if (both && dismissable()) requestClose();
      }}
    >
      <div
        ref={box}
        className={`modal oe-dialog oe-dialog-${size}${p.wide ? " modal-wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <div className="modal-head">
          <span id={titleId} className="oe-dialog-title">
            {p.title}
          </span>
          <button type="button" className="icon-btn" onClick={requestClose} aria-label={t("common.close")} title={t("common.close")}>
            <Icon name="close" />
          </button>
        </div>
        <div className="modal-body">{p.children}</div>
        {confirm && (
          <div className="oe-dialog-discard" role="alert">
            <span>{t("dlg.discardQ")}</span>
            <span className="oe-dialog-discard-btns">
              <button type="button" className="btn" autoFocus onClick={() => setConfirm(false)}>
                {t("dlg.keepEditing")}
              </button>
              <button type="button" className="btn btn-warn" onClick={() => P.current.onClose()}>
                {t("dlg.discard")}
              </button>
            </span>
          </div>
        )}
        {p.footer && <div className="modal-foot">{p.footer}</div>}
      </div>
    </div>
  );
}
