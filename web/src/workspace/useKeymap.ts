// Applies the key map (hotkeys.ts) to the window. A capture-phase listener runs before App.tsx's own handler:
// workspace actions run here; a rebound app action is re-sent as its native chord so App.tsx's handler (the one
// implementation of those actions) does the work, unless App registered a handler for it (the drawing tools, whose
// native letters align a multi-selection); a native chord whose action moved elsewhere is swallowed.

import { useEffect, useRef } from "react";
import { chordOf, eventInitOf, isBareKey, resolveKey } from "./hotkeys.ts";
import { getPrefs } from "./prefs.ts";

/** A workspace action handler, given the chord that was pressed ("Ctrl+."); returning false lets the key through
 * (nothing to do in this state). */
export type KeyHandler = (chord: string) => boolean | void;

const synthetic = new WeakSet<Event>();

/** While set (the hotkey editor is recording), every key press goes to it and nowhere else. */
let capture: ((e: KeyboardEvent) => void) | null = null;

export function setKeyCapture(fn: ((e: KeyboardEvent) => void) | null): void {
  capture = fn;
}

function typingIn(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

function canvasOrBody(): boolean {
  const a = document.activeElement;
  return !a || a === document.body || !!a.closest?.(".viewport");
}

export interface KeymapConfig {
  /** Workspace action id -> what it does. */
  handlers: Record<string, KeyHandler | undefined>;
  /** Workspace keys do nothing now (an app dialog is open). */
  blocked: () => boolean;
  /** A workspace dialog is open: keys aimed outside it are stopped. */
  modalOpen: () => boolean;
}

/** A ref to fill with the current `KeymapConfig` on every render (see `useKeymap`). */
export function useKeymapConfig(): { current: KeymapConfig } {
  return useRef<KeymapConfig>({ handlers: {}, blocked: () => false, modalOpen: () => false });
}

/**
 * Install the key map; `config.current` is read on every key press, so it can be filled later in the render. Call it
 * before other hooks that install capture-phase key listeners (EditTools), so rebinding is applied ahead of them.
 */
export function useKeymap(config: { current: KeymapConfig }): void {
  const live = config;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (synthetic.has(e) || e.defaultPrevented) return;
      if (capture) {
        if (chordOf(e) === null) return;
        e.preventDefault();
        e.stopImmediatePropagation();
        capture(e);
        return;
      }
      // A workspace dialog is open: keys aimed elsewhere (the canvas, the page) must not edit the document behind it.
      // Escape still reaches the dialog's close handler.
      if (live.current.modalOpen() && e.key !== "Escape" && !(e.target instanceof Element && e.target.closest(".modal"))) {
        e.stopImmediatePropagation();
        return;
      }
      const chord = chordOf(e);
      if (!chord) return;
      // F1 works from anywhere; other keys leave text fields alone.
      if (typingIn(e.target) && chord !== "F1") return;
      const r = resolveKey(chord, getPrefs().hotkeys);
      if (r.kind === "pass") return;
      if (r.kind === "block") {
        e.stopImmediatePropagation();
        return;
      }
      if (r.kind === "ws") {
        const { handlers: h, blocked: b } = live.current;
        const fn = h[r.action];
        // STOP is never held back by an open dialog.
        if (!fn || (b() && r.action !== "laser.stop")) return;
        // Plain letter keys act only with the canvas (or nothing) focused, as App.tsx's canvas keys do.
        if (isBareKey(chord) && !canvasOrBody() && chord !== "F1" && chord !== "Shift+?") return;
        if (fn(chord) === false) return;
        e.preventDefault();
        e.stopImmediatePropagation();
        return;
      }
      // An app action with a handler of its own runs it instead of being re-sent as its native chord: the native key
      // can mean something else in some states (R/E/C align when two or more shapes are selected, as in LightBurn),
      // while Ctrl+R / Ctrl+E or a rebound key always mean the tool.
      const own = live.current.handlers[r.action];
      if (own && !live.current.blocked() && (!isBareKey(chord) || canvasOrBody()) && own(chord) !== false) {
        e.preventDefault();
        e.stopImmediatePropagation();
        return;
      }
      const init = eventInitOf(r.to);
      if (!init) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      const ev = new KeyboardEvent("keydown", { ...init, repeat: e.repeat, bubbles: true, cancelable: true });
      synthetic.add(ev);
      (e.target ?? window).dispatchEvent(ev);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
}
