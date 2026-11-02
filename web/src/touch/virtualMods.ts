// Virtual modifier keys from the touch modifier bar (docs/ux/ui-plan-2026-11.md 1.2): a tap latches a modifier for the
// next gesture, a double-tap locks it until it is tapped again. pointer.ts `modsOf()` ORs `getVirtualMods()` with the
// physical keys, and the Viewport replays a drag in progress when a toggle changes (the same path as a key press,
// pointer.ts `modifierReplay`). The Viewport calls `clearLatched()` when a gesture ends. A plain external store with
// no DOM, so `npm test` covers it.

import { useSyncExternalStore } from "react";

/** The physical modifier a toggle stands for (Add / Constrain / Square / 15° = shift, Toggle / No snap / From centre /
 * 5° = ctrl, Guides only = alt). */
export type VirtualMod = "shift" | "ctrl" | "alt";

export interface VirtualMods {
  /** Active now (latched or locked): what modsOf() ORs with the keyboard. */
  shift: boolean;
  ctrl: boolean;
  alt: boolean;
  /** Latched for the next gesture only (cleared by clearLatched() when the gesture ends). */
  latched: readonly VirtualMod[];
  /** Locked on until tapped again. */
  locked: readonly VirtualMod[];
}

const ORDER: readonly VirtualMod[] = ["shift", "ctrl", "alt"];

function build(latched: readonly VirtualMod[], locked: readonly VirtualMod[]): VirtualMods {
  const on = (m: VirtualMod) => latched.includes(m) || locked.includes(m);
  return Object.freeze({
    shift: on("shift"),
    ctrl: on("ctrl"),
    alt: on("alt"),
    latched: Object.freeze(ORDER.filter((m) => latched.includes(m))),
    locked: Object.freeze(ORDER.filter((m) => locked.includes(m))),
  });
}

const NONE: VirtualMods = build([], []);
let state: VirtualMods = NONE;
const listeners = new Set<() => void>();

function set(next: VirtualMods): void {
  if (next.latched.join() === state.latched.join() && next.locked.join() === state.locked.join()) return;
  state = next;
  for (const l of listeners) l();
}

/** The current virtual modifiers, for code outside React (modsOf). */
export function getVirtualMods(): VirtualMods {
  return state;
}

/** Called whenever the virtual modifiers change. */
export function subscribeVirtualMods(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** The current virtual modifiers; re-renders when they change (the modifier bar's toggle states). */
export function useVirtualMods(): VirtualMods {
  return useSyncExternalStore(subscribeVirtualMods, getVirtualMods, getVirtualMods);
}

/**
 * Latch `mod` for the next gesture (a tap on its toggle). A tap on a latched toggle releases it; a tap on a locked one
 * unlocks it (so one tap always turns a lit toggle off).
 */
export function latch(mod: VirtualMod): void {
  if (state.locked.includes(mod)) return set(build(state.latched, state.locked.filter((m) => m !== mod)));
  if (state.latched.includes(mod)) return set(build(state.latched.filter((m) => m !== mod), state.locked));
  set(build([...state.latched, mod], state.locked));
}

/** Lock `mod` on, or release it when it is locked (a double-tap on its toggle). */
export function lock(mod: VirtualMod): void {
  if (state.locked.includes(mod)) return set(build(state.latched.filter((m) => m !== mod), state.locked.filter((m) => m !== mod)));
  set(build(state.latched.filter((m) => m !== mod), [...state.locked, mod]));
}

/** End of a gesture: drop the latched modifiers (locked ones stay). */
export function clearLatched(): void {
  if (state.latched.length) set(build([], state.locked));
}

/** Drop every virtual modifier (tests; leaving the canvas). */
export function resetVirtualMods(): void {
  set(NONE);
}
