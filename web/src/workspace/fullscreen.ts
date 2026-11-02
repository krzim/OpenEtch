// Full screen and the display mode (docs/ux/ui-plan-2026-11.md 1.3, P4). `useFullscreen()` drives the top-bar toggle
// (app/FullscreenButton.tsx), the Window menu item and the `view.fullscreen` hotkey (Alt+Enter, `fullscreenKey`);
// `useDisplayMode()` tells a browser tab from an installed app window.
//
// - Element full screen on <html> (`requestFullscreen({navigationUI: "hide"})`, webkit-prefixed on older Safari). It
//   needs a user gesture (the click or the keydown) but no HTTPS. Unsupported where `fullscreenEnabled` is false
//   (iPhone, iOS Home Screen apps): the toggle is hidden there and "Use as an app" is the route.
// - The state follows `fullscreenchange` and `(display-mode: fullscreen)`, which Chromium also matches in its own F11
//   mode: then `browserOwned` is true and only F11 can leave it.
// - On a secure origin Chromium keeps Esc for the app (`navigator.keyboard.lock(["Escape"])`; holding Esc leaves).
//   Elsewhere Esc leaves full screen, and the first time that happens a tip explains it (`useFullscreenTip`).
//
// The pure parts (`fullscreenCaps`, `displayModeOf`, `fullscreenStateOf`) are unit-tested without a DOM.

import { useSyncExternalStore } from "react";

export interface FullscreenState {
  /** The Fullscreen API is available (false on iPhone and in iOS Home Screen apps: hide the toggle). */
  supported: boolean;
  /** The app is full screen now (element full screen, or the browser's own F11). */
  active: boolean;
  /** Full screen belongs to the browser (F11): the page cannot leave it; tell the user to press F11. */
  browserOwned: boolean;
  /** Enter or leave element full screen. */
  toggle: () => void;
}

/** The CSS display mode the app runs in. */
export type DisplayMode = "browser" | "minimal-ui" | "standalone" | "fullscreen" | "window-controls-overlay";

/** The display modes in the order they are tested (the first match wins). */
export const DISPLAY_MODES: readonly Exclude<DisplayMode, "browser">[] = ["window-controls-overlay", "fullscreen", "standalone", "minimal-ui"];

/** What `fullscreenCaps` reads from `document` (prefixed names included). */
export interface FullscreenDocLike {
  fullscreenEnabled?: boolean;
  webkitFullscreenEnabled?: boolean;
  fullscreenElement?: unknown;
  webkitFullscreenElement?: unknown;
  documentElement?: { requestFullscreen?: unknown; webkitRequestFullscreen?: unknown } | null;
}

export interface FullscreenCaps {
  /** Element full screen can be requested. */
  supported: boolean;
  /** An element is full screen now. */
  active: boolean;
  /** Only the webkit-prefixed API exists (older Safari). */
  prefixed: boolean;
}

/** The Fullscreen API as `doc` offers it. */
export function fullscreenCaps(doc: FullscreenDocLike | null | undefined): FullscreenCaps {
  if (!doc) return { supported: false, active: false, prefixed: false };
  const el = doc.documentElement ?? null;
  const std = typeof el?.requestFullscreen === "function";
  const webkit = typeof el?.webkitRequestFullscreen === "function";
  const enabled = doc.fullscreenEnabled ?? doc.webkitFullscreenEnabled ?? false;
  const active = !!(doc.fullscreenElement ?? doc.webkitFullscreenElement);
  return { supported: enabled === true && (std || webkit), active, prefixed: !std && webkit };
}

/**
 * The display mode from `(display-mode: …)` media queries (`matches(query)`), with iOS's `navigator.standalone` for
 * Home Screen apps on older iOS.
 */
export function displayModeOf(matches: (query: string) => boolean, iosStandalone = false): DisplayMode {
  for (const m of DISPLAY_MODES) {
    if (matches(`(display-mode: ${m})`)) return m;
  }
  return iosStandalone ? "standalone" : "browser";
}

/** Whether the app runs as an installed app window (not a browser tab, not the browser's F11 full screen). */
export function isAppWindow(mode: DisplayMode): boolean {
  return mode === "standalone" || mode === "window-controls-overlay" || mode === "minimal-ui";
}

/** Combine the API state and the display mode: F11 shows as `display-mode: fullscreen` with no full-screen element. */
export function fullscreenStateOf(caps: FullscreenCaps, mode: DisplayMode): { supported: boolean; active: boolean; browserOwned: boolean } {
  const browserOwned = !caps.active && mode === "fullscreen";
  return { supported: caps.supported, active: caps.active || browserOwned, browserOwned };
}

// ---- the live store (browser only) ---------------------------------------------------------------------------------

/** A one-time tip next to the toggle: why Esc left full screen, or that F11 owns it. */
export type FullscreenTip = "escape" | "browser";

/** localStorage key: the Esc tip was shown once. */
export const ESC_TIP_KEY = "openetch.fullscreenEscTip";

interface KeyboardLock {
  lock?: (keys?: string[]) => Promise<void>;
  unlock?: () => void;
}

const listeners = new Set<() => void>();
let fsState: FullscreenState = Object.freeze({ supported: false, active: false, browserOwned: false, toggle: () => toggle() });
let mode: DisplayMode = "browser";
let tip: FullscreenTip | null = null;
let installed = false;
/** We asked to leave (so the change that follows is not an Esc). */
let leaving = false;
/** Element full screen that this page entered is on. */
let entered = false;

function hasDom(): boolean {
  return typeof window !== "undefined" && typeof document !== "undefined";
}

function readMode(): DisplayMode {
  if (!hasDom() || typeof window.matchMedia !== "function") return "browser";
  const nav = navigator as Navigator & { standalone?: boolean };
  return displayModeOf((q) => window.matchMedia(q).matches, nav.standalone === true);
}

function emit(): void {
  for (const fn of [...listeners]) fn();
}

/** Recompute from the document; the snapshot object changes only when a field does. */
function refresh(): void {
  if (!hasDom()) return;
  const caps = fullscreenCaps(document as unknown as FullscreenDocLike);
  const nextMode = readMode();
  const next = fullscreenStateOf(caps, nextMode);
  // Left element full screen without asking for it: Esc (or the browser's own exit). Explain once.
  if (entered && !caps.active) {
    entered = false;
    keyboard()?.unlock?.();
    if (!leaving && !tipShown()) showTip("escape");
    leaving = false;
  }
  let changed = nextMode !== mode;
  mode = nextMode;
  if (next.supported !== fsState.supported || next.active !== fsState.active || next.browserOwned !== fsState.browserOwned) {
    fsState = Object.freeze({ ...next, toggle: fsState.toggle });
    changed = true;
  }
  if (changed) emit();
}

function install(): void {
  if (installed || !hasDom()) return;
  installed = true;
  document.addEventListener("fullscreenchange", refresh);
  document.addEventListener("webkitfullscreenchange", refresh);
  if (typeof window.matchMedia === "function") {
    for (const m of DISPLAY_MODES) {
      const mq = window.matchMedia(`(display-mode: ${m})`);
      mq.addEventListener?.("change", refresh);
    }
  }
  // F11 does not always fire a media change event; the window size always changes.
  window.addEventListener("resize", refresh);
  refresh();
}

function subscribe(fn: () => void): () => void {
  install();
  listeners.add(fn);
  return () => void listeners.delete(fn);
}

function keyboard(): KeyboardLock | undefined {
  return hasDom() ? (navigator as Navigator & { keyboard?: KeyboardLock }).keyboard : undefined;
}

function tipShown(): boolean {
  try {
    return localStorage.getItem(ESC_TIP_KEY) === "1";
  } catch {
    return false;
  }
}

function showTip(kind: FullscreenTip): void {
  tip = kind;
  if (kind === "escape") {
    try {
      localStorage.setItem(ESC_TIP_KEY, "1");
    } catch {
      // Private mode: the tip may come again; harmless.
    }
  }
  emit();
}

/** Hide the tip. */
export function dismissFullscreenTip(): void {
  if (tip === null) return;
  tip = null;
  emit();
}

async function enter(): Promise<void> {
  const el = document.documentElement as HTMLElement & { webkitRequestFullscreen?: () => void };
  try {
    if (typeof el.requestFullscreen === "function") await el.requestFullscreen({ navigationUI: "hide" });
    else el.webkitRequestFullscreen?.();
  } catch {
    return; // Refused (no user gesture, or blocked by a permissions policy): nothing changes.
  }
  entered = true;
  leaving = false;
  dismissFullscreenTip();
  // Keep Esc for the app (cancel a tool, close a dialog) where the browser allows it: Chromium, secure origins.
  if (window.isSecureContext) {
    try {
      await keyboard()?.lock?.(["Escape"]);
    } catch {
      // Not allowed here: Esc leaves full screen, and the tip explains it.
    }
  }
  refresh();
}

function exit(): void {
  const d = document as Document & { webkitExitFullscreen?: () => void; webkitFullscreenElement?: Element | null };
  leaving = true;
  keyboard()?.unlock?.();
  if (d.fullscreenElement && typeof d.exitFullscreen === "function") void d.exitFullscreen().catch(() => (leaving = false));
  else if (d.webkitFullscreenElement) d.webkitExitFullscreen?.();
}

/** Enter or leave full screen; under the browser's F11 only a tip can help. Call it in a click or key handler. */
export function toggle(): void {
  if (!hasDom()) return;
  install();
  refresh();
  const s = fsState;
  if (!s.supported) return;
  if (s.browserOwned) showTip("browser");
  else if (s.active) exit();
  else void enter();
}

/**
 * The `view.fullscreen` hotkey handler (Alt+Enter, rebindable): toggles and takes the key, or returns false where
 * full screen is unsupported so the key goes on as before. EditTools ignores Enter with Alt held, so Alt+Enter never
 * also finishes a line or pen path.
 */
export function fullscreenKey(): boolean {
  if (!hasDom()) return false;
  install();
  refresh();
  if (!fsState.supported) return false;
  toggle();
  return true;
}

// The first read installs the listeners and reads the document (no subscriber exists yet then, so nothing is
// notified during a render), so the first render already shows the real state.
const getFullscreen = (): FullscreenState => (install(), fsState);
const getDisplayMode = (): DisplayMode => (install(), mode);
const getTip = (): FullscreenTip | null => tip;

export function useFullscreen(): FullscreenState {
  return useSyncExternalStore(subscribe, getFullscreen, getFullscreen);
}

export function useDisplayMode(): DisplayMode {
  return useSyncExternalStore(subscribe, getDisplayMode, getDisplayMode);
}

/** The tip to show next to the full-screen toggle, or null. */
export function useFullscreenTip(): FullscreenTip | null {
  return useSyncExternalStore(subscribe, getTip, getTip);
}

/** The display mode now (outside React). */
export function displayModeNow(): DisplayMode {
  install();
  refresh();
  return mode;
}
