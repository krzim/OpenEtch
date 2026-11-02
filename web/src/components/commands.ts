// Running a hotkeys.ts action from a menu item, a ribbon button or command search, exactly as its key would: the
// action's current chord is sent as a key press, so the same handler runs (App.tsx's own keys, the workspace key map
// with its rebinding, EditTools) and nothing is implemented twice. Actions without a key can be run through a runner
// the workspace layer may register (`setActionRunner`). No React; DOM only when called.

import { ACTION_BY_ID, chordsFor, eventInitOf } from "../workspace/hotkeys.ts";
import { getPrefs } from "../workspace/prefs.ts";

type Runner = (id: string) => boolean;

let runner: Runner | null = null;

/**
 * Register a direct runner for workspace actions (`(id) => handlers[id]?.("") !== false`), used before the key
 * press route. Integration hook: useWorkspaceUi can call it with its key map's handlers.
 */
export function setActionRunner(fn: Runner | null): void {
  runner = fn;
}

/** The chord a press of action `id` is sent as now (with the user's rebinding), or null when it has none. */
export function chordToSend(id: string, overrides: Readonly<Record<string, string[]>> = getPrefs().hotkeys): string | null {
  const a = ACTION_BY_ID.get(id);
  if (!a || a.fixed) return null;
  return chordsFor(a, overrides)[0] ?? null;
}

/** Whether `id` can be run now (it has a key, or a runner is registered). */
export function canRunAction(id: string): boolean {
  return !!runner || chordToSend(id) !== null;
}

/**
 * Run hotkeys action `id`. Returns false when it cannot be run this way (no key bound and no runner). The focus
 * leaves any toolbar or menu button first, so keys that act only with the canvas (or nothing) focused work.
 */
export function runHotkeyAction(id: string): boolean {
  if (runner && runner(id)) return true;
  const chord = chordToSend(id);
  const init = chord ? eventInitOf(chord) : null;
  if (!init || typeof document === "undefined") return false;
  const a = document.activeElement;
  if (a instanceof HTMLElement && a !== document.body && !a.closest(".viewport")) a.blur();
  const target = document.activeElement instanceof HTMLElement && document.activeElement !== document.body ? document.activeElement : document.body;
  target.dispatchEvent(new KeyboardEvent("keydown", { ...init, bubbles: true, cancelable: true }));
  target.dispatchEvent(new KeyboardEvent("keyup", { ...init, bubbles: true, cancelable: true }));
  return true;
}
