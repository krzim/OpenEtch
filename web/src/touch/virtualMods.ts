// STAGE A STUB (owner: P3). Virtual modifier keys from the touch modifier bar (docs/ux/ui-plan-2026-11.md 1.2): a tap
// latches a modifier for the next gesture, a double-tap locks it. pointer.ts `modsOf()` ORs `getVirtualMods()` with
// the physical keys. The signatures below are final; until P3 lands nothing is ever latched or locked.

import { useSyncExternalStore } from "react";

/** The physical modifier a toggle stands for (Add = shift, Toggle = ctrl, No snap = ctrl, ...; P3 maps them). */
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

const NONE: VirtualMods = Object.freeze({ shift: false, ctrl: false, alt: false, latched: [], locked: [] });

/** The current virtual modifiers, for code outside React (modsOf). */
export function getVirtualMods(): VirtualMods {
  return NONE;
}

function subscribe(_fn: () => void): () => void {
  return () => {};
}

/** The current virtual modifiers; re-renders when they change (the modifier bar's toggle states). */
export function useVirtualMods(): VirtualMods {
  return useSyncExternalStore(subscribe, getVirtualMods, getVirtualMods);
}

/** Latch `mod` for the next gesture (a tap on its toggle). */
export function latch(_mod: VirtualMod): void {}

/** Lock `mod` on, or release it when it is locked (a double-tap on its toggle). */
export function lock(_mod: VirtualMod): void {}

/** End of a gesture: drop the latched modifiers (locked ones stay). */
export function clearLatched(): void {}
