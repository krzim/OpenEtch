// Esc stops framing (owner decision 4, docs/ux/ui-plan-2026-11.md): while the red pointer frames, Esc ends the frame,
// unless something else took the key first (an open menu, dialog or popover, or a text field). Esc never stops a burn:
// STOP, Ctrl+. (Cmd+.) and the foot switch do. Several components may install it; each key press stops at most once.

import { useEffect } from "react";
import { jobApi } from "../api/jobApi.ts";

const handled = new WeakSet<Event>();

/** Whether this Escape press is free to stop framing (pure on the event and the document). */
export function escFreeForFraming(e: Pick<KeyboardEvent, "key" | "defaultPrevented" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "target">, doc: Pick<Document, "querySelector"> | null): boolean {
  if (e.key !== "Escape" || e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return false;
  const el = e.target as HTMLElement | null;
  if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable)) return false;
  if (doc && doc.querySelector(".modal-backdrop, [role=menu], .ctx-menu, .frame-pop")) return false;
  return true;
}

/** While `framing`, Esc stops the frame; `after` runs once the engine answered (refresh the status, toast). */
export function useEscStopsFraming(framing: boolean, after: (err: unknown | null) => void): void {
  useEffect(() => {
    if (!framing) return;
    const onKey = (e: KeyboardEvent) => {
      if (handled.has(e) || !escFreeForFraming(e, document)) return;
      handled.add(e);
      jobApi
        .frameStop()
        .then(() => after(null))
        .catch((err: unknown) => after(err));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [framing, after]);
}
