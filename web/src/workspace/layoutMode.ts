// Layout modes (docs/ux/ui-plan-2026-11.md 1.1): which chrome the window gets, from its size, the primary pointer and
// the Preferences overrides. Replaces App.tsx's single NARROW_QUERY. CSS keys off three attributes on <html>, written
// here whenever the result changes:
// - data-layout:  wide | compact | tablet | phone
// - data-pointer: fine | coarse
// - data-density: comfortable | compact | touch
//
// `resolveLayout` is pure (tests in layoutMode.test.ts). The store is built like prefs.ts: `getLayoutMode()` for code
// outside React, `useLayoutMode()` to re-render when the mode (not merely the size) changes.

import { useSyncExternalStore } from "react";
import { getPrefs, subscribePrefs, type DensityPref, type LayoutOverride } from "./prefs.ts";

export type LayoutMode = Exclude<LayoutOverride, "auto">;
export type PointerKind = "fine" | "coarse";
export type Density = Exclude<DensityPref, "auto">;

export const LAYOUT_MODES: readonly LayoutMode[] = ["wide", "compact", "tablet", "phone"];

/** Breakpoints in CSS px (first match wins, in this order). */
export const BREAKPOINTS = {
  /** phone: narrower than this ... */
  phoneMaxW: 600,
  /** ... or a coarse pointer with the short side below this (a phone in landscape). */
  phoneCoarseMinSide: 500,
  /** tablet: narrower than this, or any coarse pointer. */
  tabletMaxW: 1100,
  /** compact: narrower than this ... */
  compactMaxW: 1600,
  /** ... or lower than this (a MacBook's Safari window). */
  compactMaxH: 900,
} as const;

export interface LayoutInput {
  /** Viewport width and height in CSS px. */
  w: number;
  h: number;
  /** The primary pointer is coarse (a finger). */
  coarse: boolean;
  /** Preferences > Appearance > Layout ("auto" or absent: from the size and pointer). */
  override?: LayoutOverride;
  /** Preferences > Appearance > Density ("auto" or absent: from the pointer and the layout). */
  density?: DensityPref;
}

export interface LayoutState {
  layout: LayoutMode;
  pointer: PointerKind;
  density: Density;
}

/** The layout for a viewport when nothing is overridden. */
export function autoLayout(w: number, h: number, coarse: boolean): LayoutMode {
  const b = BREAKPOINTS;
  if (w < b.phoneMaxW || (coarse && Math.min(w, h) < b.phoneCoarseMinSide)) return "phone";
  if (w < b.tabletMaxW || coarse) return "tablet";
  if (w < b.compactMaxW || h < b.compactMaxH) return "compact";
  return "wide";
}

/** The density when nothing is overridden: touch for a coarse pointer, else comfortable only in the wide layout. */
export function autoDensity(layout: LayoutMode, coarse: boolean): Density {
  if (coarse) return "touch";
  return layout === "wide" ? "comfortable" : "compact";
}

/** Layout, pointer and density for a viewport and the Preferences overrides. */
export function resolveLayout(i: LayoutInput): LayoutState {
  const w = Number.isFinite(i.w) && i.w > 0 ? i.w : 1920;
  const h = Number.isFinite(i.h) && i.h > 0 ? i.h : 1080;
  const layout = i.override && i.override !== "auto" ? i.override : autoLayout(w, h, i.coarse);
  const density = i.density && i.density !== "auto" ? i.density : autoDensity(layout, i.coarse);
  return { layout, pointer: i.coarse ? "coarse" : "fine", density };
}

export function sameLayout(a: LayoutState, b: LayoutState): boolean {
  return a.layout === b.layout && a.pointer === b.pointer && a.density === b.density;
}

/**
 * The on-screen keyboard: while a text field has focus, a viewport that only got shorter (same width) is the keyboard
 * opening, not a new device or window size, so the layout stays (the tablet layout used to flip to the phone shell
 * mid-typing, and the field lost its place).
 */
export function keyboardResize(prev: { w: number; h: number }, next: { w: number; h: number }, typing: boolean): boolean {
  return typing && Math.abs(next.w - prev.w) < 1 && next.h < prev.h;
}

/**
 * The size to choose a layout for while a text field has focus. A device turned with the on-screen keyboard up gets a
 * viewport as short as the keyboard leaves it (an iPad in landscape: 1180 x ~350), which alone would pick the phone
 * shell, drop the field and close the keyboard. The screen tells the real height for that orientation: when the
 * viewport is as wide as one of the screen's sides, the height is taken as the other side less the browser's bars
 * (`bars`), unless the viewport is taller. Browsers report `screen` in either orientation (iOS keeps it portrait),
 * hence both sides are tried. Not typing, or a window narrower than the screen (a desktop): the viewport as it is.
 */
export function typingLayoutSize(next: { w: number; h: number }, screen: { width: number; height: number } | null, typing: boolean, bars = 100): { w: number; h: number } {
  if (!typing || !screen || !(screen.width > 0 && screen.height > 0)) return next;
  const other = Math.abs(next.w - screen.width) < 2 ? screen.height : Math.abs(next.w - screen.height) < 2 ? screen.width : null;
  if (other === null) return next;
  return { w: next.w, h: Math.max(next.h, other - bars) };
}

/** A focused element that brings up the on-screen keyboard. */
export function isTextEntry(el: Element | null): boolean {
  if (!el) return false;
  const tag = el.tagName;
  if (tag === "TEXTAREA") return true;
  if ((el as HTMLElement).isContentEditable) return true;
  if (tag !== "INPUT") return false;
  const type = ((el as HTMLInputElement).type || "text").toLowerCase();
  return !["button", "checkbox", "radio", "range", "color", "file", "submit", "reset", "image", "hidden"].includes(type);
}

/** Write the three attributes on `root` (default: <html>). */
export function applyLayoutAttributes(s: LayoutState, root?: { dataset: DOMStringMap }): void {
  const el = root ?? (typeof document === "undefined" ? null : document.documentElement);
  if (!el) return;
  el.dataset.layout = s.layout;
  el.dataset.pointer = s.pointer;
  el.dataset.density = s.density;
}

// --- store -----------------------------------------------------------------------------------------------------------

const COARSE_QUERY = "(pointer: coarse)";

function coarseQuery(): MediaQueryList | null {
  try {
    return typeof window === "undefined" || !window.matchMedia ? null : window.matchMedia(COARSE_QUERY);
  } catch {
    return null;
  }
}

function compute(size?: { w: number; h: number }): LayoutState {
  const p = getPrefs();
  const hasWindow = typeof window !== "undefined";
  return resolveLayout({
    w: size?.w ?? (hasWindow ? window.innerWidth : 1920),
    h: size?.h ?? (hasWindow ? window.innerHeight : 1080),
    coarse: !!coarseQuery()?.matches,
    override: p.layoutOverride,
    density: p.density,
  });
}

let current: LayoutState = compute();
const listeners = new Set<() => void>();
let detach: (() => void) | null = null;
/** The viewport the current layout was chosen for (before an on-screen keyboard shrank it). */
let chosenFor = typeof window === "undefined" ? { w: 1920, h: 1080 } : { w: window.innerWidth, h: window.innerHeight };

/** Recompute; when the result changed, write the attributes and notify subscribers. */
function refresh(): void {
  if (typeof window !== "undefined") {
    const size = { w: window.innerWidth, h: window.innerHeight };
    const typing = typeof document !== "undefined" && isTextEntry(document.activeElement);
    const keyboard = keyboardResize(chosenFor, size, typing);
    // <html data-keyboard>: the on-screen keyboard is up (the phone shell hides its tab bar to leave the field room).
    if (typeof document !== "undefined") {
      if (keyboard) document.documentElement.dataset.keyboard = "";
      else delete document.documentElement.dataset.keyboard;
    }
    if (keyboard) return;
    // Turned (or resized) while typing: the layout for the screen's height, not the keyboard's leftover.
    // Touch screens only: a desktop window is not turned, and its keyboard takes no room.
    chosenFor = typingLayoutSize(size, typeof screen === "undefined" ? null : screen, typing && !!coarseQuery()?.matches);
  }
  const next = compute(typeof window === "undefined" ? undefined : chosenFor);
  if (sameLayout(next, current)) return;
  current = next;
  applyLayoutAttributes(current);
  for (const l of listeners) l();
}

function attach(): () => void {
  const mq = coarseQuery();
  const offPrefs = subscribePrefs(refresh);
  if (typeof window !== "undefined") {
    window.addEventListener("resize", refresh);
    window.addEventListener("orientationchange", refresh);
  }
  mq?.addEventListener?.("change", refresh);
  return () => {
    offPrefs();
    if (typeof window !== "undefined") {
      window.removeEventListener("resize", refresh);
      window.removeEventListener("orientationchange", refresh);
    }
    mq?.removeEventListener?.("change", refresh);
  };
}

/** The current layout state (same object until something changes, as useSyncExternalStore needs). */
export function getLayoutMode(): LayoutState {
  if (!detach) {
    const next = compute();
    if (!sameLayout(next, current)) current = next;
  }
  return current;
}

export function subscribeLayoutMode(fn: () => void): () => void {
  listeners.add(fn);
  if (!detach) {
    detach = attach();
    current = compute();
    applyLayoutAttributes(current);
  }
  return () => {
    listeners.delete(fn);
    if (listeners.size === 0 && detach) {
      detach();
      detach = null;
    }
  };
}

/** The layout state; re-renders when the layout, pointer or density changes. While mounted, <html> carries the
 * data-layout / data-pointer / data-density attributes. */
export function useLayoutMode(): LayoutState {
  return useSyncExternalStore(subscribeLayoutMode, getLayoutMode, getLayoutMode);
}
