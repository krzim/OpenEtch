// Pointer capture that never throws. `setPointerCapture` throws "NotFoundError: No active pointer" when the pointer is
// already gone (a touch that ended, a synthetic event, a pen lifted between events); a drag must still start then, it
// just is not captured. Also which modifier key events re-evaluate the drag or hover (modifierReplay). No React, so
// `npm test` covers it.

/** Something that can capture a pointer (an Element). */
export interface PointerCapturer {
  setPointerCapture?: (pointerId: number) => void;
}

/** Capture `pointerId` on `el` if possible. Returns whether the capture took; never throws. */
export function capturePointer(el: PointerCapturer | null | undefined, pointerId: number): boolean {
  if (!el || typeof el.setPointerCapture !== "function") return false;
  try {
    el.setPointerCapture(pointerId);
    return true;
  } catch {
    // No active pointer: the drag still works while the pointer stays over the element.
    return false;
  }
}

/** Modifier keys whose press or release changes what the pointer does (Shift constrains, Ctrl and Alt snapping). */
const MOD_KEYS: readonly string[] = ["Shift", "Control", "Alt", "Meta"];

/**
 * What a press or release of `key` re-evaluates at the last pointer position: the drag in progress (`gesture` is the
 * canvas gesture's kind), the hover of a tool when no button is down (the line tool's Shift angle and its guide follow
 * Shift without a pointer move), or nothing.
 */
export function modifierReplay(key: string, gesture: string, hovering: boolean): "drag" | "hover" | null {
  if (!MOD_KEYS.includes(key)) return null;
  if (gesture !== "none" && gesture !== "idle") return "drag";
  return hovering && gesture === "none" ? "hover" : null;
}

// ---- pointer classes and tolerances (docs/ux/ui-plan-2026-11.md 1.2) ------------------------------------------------

/** What kind of pointer an event came from. Unknown types count as a mouse. */
export type PointerClass = "mouse" | "pen" | "touch";

/** Pointer class of an event (`e.pointerType`). */
export function pointerClass(e: { pointerType?: string } | null | undefined): PointerClass {
  const t = e?.pointerType;
  return t === "touch" ? "touch" : t === "pen" ? "pen" : "mouse";
}

/** Screen-px tolerances of one pointer class. */
export interface Tolerances {
  /** Click (select) tolerance around an outline (LightBurn "Click Selection Tolerance"). */
  click: number;
  /** How far a press may wander before it becomes a drag. */
  slop: number;
  /** Node and node-handle pick radius (Edit Nodes). */
  node: number;
  /** Snap distance (the larger of this and Preferences > Snap distance). */
  snap: number;
  /** Size of a selection handle's hit area. */
  handle: number;
  /** Two presses closer than this (px) ... */
  dblDist: number;
  /** ... and this (ms) are a double-click / double-tap. */
  dblMs: number;
  /** A press held this long (ms) within the slop is a long-press (the touch right-click). */
  longPressMs: number;
}

/**
 * The tolerance table. The mouse row is what the canvas always used (Viewport TAP_SLOP 4 / CLICK_TOL 5, EditTools
 * SLOP 4 / NODE_TOL 7 / DBL 350 ms within 6 px, snapping 8, 16 px handles), so mouse behaviour is unchanged.
 */
export const TOL: Readonly<Record<PointerClass, Tolerances>> = {
  mouse: { click: 5, slop: 4, node: 7, snap: 8, handle: 16, dblDist: 6, dblMs: 350, longPressMs: 500 },
  pen: { click: 8, slop: 6, node: 10, snap: 10, handle: 24, dblDist: 12, dblMs: 350, longPressMs: 500 },
  touch: { click: 14, slop: 10, node: 18, snap: 14, handle: 44, dblDist: 24, dblMs: 400, longPressMs: 500 },
};

/** Tolerances for an event's pointer. */
export function tolOf(e: { pointerType?: string } | null | undefined): Tolerances {
  return TOL[pointerClass(e)];
}

/** Hit size (px) of a selection handle for a pointer class (P2 can use it for the design UI's own handles). */
export function handleHitPx(cls: PointerClass): number {
  return TOL[cls].handle;
}

/** How much larger than the mouse's the selection handles are drawn and hit for a pointer class (touch: 2x+). */
export function handleFactor(cls: PointerClass): number {
  return TOL[cls].handle / TOL.mouse.handle;
}

// ---- modifiers: physical keys OR the touch modifier bar, with the Apple mapping -------------------------------------

/** Whether this is an Apple platform (Cmd stands for Ctrl; Ctrl+click is the right-click there). */
export function isApplePlatform(nav: { platform?: string; userAgent?: string; userAgentData?: { platform?: string } } | undefined = typeof navigator === "undefined" ? undefined : navigator): boolean {
  if (!nav) return false;
  const p = nav.userAgentData?.platform || nav.platform || "";
  if (/mac|iphone|ipad|ipod/i.test(p)) return true;
  return /Macintosh|iPhone|iPad|iPod/.test(nav.userAgent ?? "");
}

/** Keyboard modifier state of an event. */
export interface KeyMods {
  shiftKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
}

/** Virtual modifiers (touch/virtualMods.ts) as plain flags. */
export interface ModFlags {
  shift: boolean;
  ctrl: boolean;
  alt: boolean;
}

const NO_MODS: ModFlags = { shift: false, ctrl: false, alt: false };

/**
 * The LightBurn modifiers of an event: physical keys OR the virtual ones. LightBurn's Ctrl is Ctrl on Windows and
 * Linux and Cmd on Apple platforms, where Ctrl+click is the system right-click (so Ctrl alone does not count there).
 */
export function modsOf(e: KeyMods, virtual: ModFlags = NO_MODS, apple = isApplePlatform()): { shift: boolean; ctrl: boolean; alt: boolean } {
  const ctrl = apple ? e.metaKey : e.ctrlKey || e.metaKey;
  return { shift: e.shiftKey || virtual.shift, ctrl: ctrl || virtual.ctrl, alt: e.altKey || virtual.alt };
}

/** A mouse press that is the Mac right-click (Ctrl+click): the context menu follows, the press must not select. */
export function isMacContextClick(e: { pointerType?: string; button: number; ctrlKey: boolean; metaKey: boolean }, apple = isApplePlatform()): boolean {
  return apple && pointerClass(e) === "mouse" && e.button === 0 && e.ctrlKey && !e.metaKey;
}
