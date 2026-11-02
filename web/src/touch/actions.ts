// Touch actions without a keyboard: the selection action bar, two- and three-finger taps and the nudge pad run the
// same actions as their hotkeys. They are sent as the action's current key chord (Preferences can rebind it) to the
// canvas, so App.tsx's one implementation of each action does the work and nothing is duplicated. The "menu" action
// opens the canvas context menu (a contextmenu event at the selection). DOM only at call time; the mapping is pure.

import { ACTION_BY_ID, chordsFor, eventInitOf } from "../workspace/hotkeys.ts";
import { getPrefs } from "../workspace/prefs.ts";
import { getHud } from "./hudState.ts";

/** TouchHud action ids (the Stage A contract) -> workspace/hotkeys.ts action ids, or a fixed key. */
export const TOUCH_ACTIONS: Readonly<Record<string, { action?: string; key?: string }>> = {
  copy: { action: "edit.copy" },
  cut: { action: "edit.cut" },
  paste: { action: "edit.paste" },
  pasteInPlace: { action: "edit.pasteInPlace" },
  duplicate: { action: "edit.duplicate" },
  delete: { action: "edit.delete" },
  selectAll: { action: "edit.selectAll" },
  group: { action: "arrange.group" },
  ungroup: { action: "arrange.ungroup" },
  flipH: { action: "arrange.flipH" },
  flipV: { action: "arrange.flipV" },
  undo: { action: "edit.undo" },
  redo: { action: "edit.redo" },
  // LightBurn's "." rotates the selection 90° clockwise (editing.ts rotateKeyDeg; a fixed key).
  rotate90: { key: "." },
  rotate90ccw: { key: "," },
  // The nudge pad: arrow keys (editing.ts nudgeDelta; Shift large, Ctrl small).
  nudgeLeft: { key: "ArrowLeft" },
  nudgeRight: { key: "ArrowRight" },
  nudgeUp: { key: "ArrowUp" },
  nudgeDown: { key: "ArrowDown" },
};

/** The chord an action runs on now: its first binding, else its native key, else null (unbound). */
export function chordForAction(actionId: string, overrides: Readonly<Record<string, string[]>>): string | null {
  const a = ACTION_BY_ID.get(actionId);
  if (!a) return null;
  return chordsFor(a, overrides)[0] ?? a.native?.[0] ?? null;
}

/** The key event init a touch action is sent as, or null for "menu" and unknown ids. */
export function keyInitFor(
  id: string,
  overrides: Readonly<Record<string, string[]>>,
  extra: { shift?: boolean; ctrl?: boolean } = {},
): { key: string; code: string; ctrlKey: boolean; altKey: boolean; shiftKey: boolean } | null {
  const a = TOUCH_ACTIONS[id];
  if (!a) return null;
  if (a.key) {
    const code = a.key === "." ? "Period" : a.key === "," ? "Comma" : a.key;
    return { key: a.key, code, ctrlKey: !!extra.ctrl, altKey: false, shiftKey: !!extra.shift };
  }
  const chord = a.action ? chordForAction(a.action, overrides) : null;
  return chord ? eventInitOf(chord) : null;
}

function canvasEl(): HTMLElement | null {
  return typeof document === "undefined" ? null : document.querySelector<HTMLElement>(".viewport");
}

/** Send a key event to the canvas as if the user pressed it there. */
function sendKey(init: { key: string; code: string; ctrlKey: boolean; altKey: boolean; shiftKey: boolean }): boolean {
  const el = canvasEl();
  if (!el) return false;
  el.focus({ preventScroll: true });
  el.dispatchEvent(new KeyboardEvent("keydown", { ...init, bubbles: true, cancelable: true }));
  el.dispatchEvent(new KeyboardEvent("keyup", { ...init, bubbles: true, cancelable: true }));
  return true;
}

/** Open the canvas context menu at the selection (or the canvas centre). */
export function openCanvasMenu(): boolean {
  const el = canvasEl();
  if (!el) return false;
  const h = getHud();
  const r = h.selRect ?? h.canvasRect ?? el.getBoundingClientRect();
  const x = (r.left + r.right) / 2;
  const y = Math.max(r.top, 0) + Math.min(24, (r.bottom - r.top) / 2);
  el.dispatchEvent(new MouseEvent("contextmenu", { clientX: x, clientY: y, bubbles: true, cancelable: true, button: 2 }));
  return true;
}

/**
 * Run a touch action by id ("copy", "paste", "duplicate", "delete", "group", "ungroup", "rotate90", "flipH", "flipV",
 * "undo", "redo", "menu", "nudgeLeft", ...). P1 passes this as `<TouchHud onAction={runTouchAction}/>`.
 */
export function runTouchAction(id: string, extra: { shift?: boolean; ctrl?: boolean } = {}): boolean {
  if (id === "menu") return openCanvasMenu();
  const init = keyInitFor(id, getPrefs().hotkeys, extra);
  return init ? sendKey(init) : false;
}
