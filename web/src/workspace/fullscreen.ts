// STAGE A STUB (owner: P4). Full screen and the display mode (docs/ux/ui-plan-2026-11.md 1.3): `useFullscreen()` for
// the top-bar toggle, the Window menu and the `view.fullscreen` hotkey (Alt+Enter); `useDisplayMode()` tells a browser
// tab from an installed app window. The signatures below are final; until P4 lands full screen reports unsupported and
// the display mode is "browser".

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

const NOOP = (): void => {};
const UNSUPPORTED: FullscreenState = Object.freeze({ supported: false, active: false, browserOwned: false, toggle: NOOP });

function subscribe(_fn: () => void): () => void {
  return () => {};
}

const getFullscreen = (): FullscreenState => UNSUPPORTED;
const getDisplayMode = (): DisplayMode => "browser";

export function useFullscreen(): FullscreenState {
  return useSyncExternalStore(subscribe, getFullscreen, getFullscreen);
}

export function useDisplayMode(): DisplayMode {
  return useSyncExternalStore(subscribe, getDisplayMode, getDisplayMode);
}
