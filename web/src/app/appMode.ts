// "Use as an app" (docs/ux/ui-plan-2026-11.md 1.3, P4): which platform this is and the steps that give OpenEtch its
// own window there. Pure, so it is unit-tested without a DOM (appMode.test.ts); the dialog is AppModeDialog.tsx.
//
// What works over plain http (the Pi's http://<pi-ip>:7878): Safari's File > Add to Dock (macOS Sonoma+), Add
// to Home Screen on iPad and iPhone, and Chrome/Edge's Create shortcut > Open as window. A real install in
// Chrome/Edge/Android, and the title-bar layout (window-controls-overlay), need a secure origin (docs/https-on-the-pi.md).

import type { MsgKey } from "../workspace/locales/en.ts";
import { isAppWindow, type DisplayMode } from "../workspace/fullscreen.ts";

export type AppPlatform = "mac-safari" | "chromium" | "firefox" | "ipad" | "iphone" | "android" | "other";

/**
 * The platform from the user agent. iPadOS Safari reports a Mac ("Macintosh") by default; a Mac has no touch points,
 * an iPad has several (`navigator.maxTouchPoints`).
 */
export function detectPlatform(ua: string, touchPoints = 0): AppPlatform {
  if (/iPhone|iPod/.test(ua)) return "iphone";
  if (/iPad/.test(ua) || (/Macintosh/.test(ua) && touchPoints > 1)) return "ipad";
  if (/Android/.test(ua)) return "android";
  if (/Firefox\/|FxiOS/.test(ua)) return "firefox";
  // Edge and Opera say Chrome too; Chrome on a Mac also says Safari.
  if (/Chrome\/|Chromium\/|Edg\//.test(ua)) return "chromium";
  if (/Macintosh/.test(ua) && /Safari\//.test(ua) && /Version\//.test(ua)) return "mac-safari";
  return "other";
}

export interface InstallGuide {
  platform: AppPlatform;
  /** Already an app window here: the "Use as an app" item and dialog are not offered. */
  hidden: boolean;
  /** The steps, in order. */
  steps: MsgKey[];
  /** The browser can offer its own install prompt (Chromium on a secure origin): show an Install button when it does. */
  installPrompt: boolean;
  /** The page is plain http where a real install needs HTTPS: show the HTTPS note. */
  httpNote: boolean;
  /** Element full screen is a fallback worth mentioning (desktops). */
  fullscreenHint: boolean;
}

/** The steps for `ua` in `displayMode`, on a secure origin or not (`touchPoints`: navigator.maxTouchPoints). */
export function installGuide(ua: string, displayMode: DisplayMode, secure: boolean, touchPoints = 0): InstallGuide {
  const platform = detectPlatform(ua, touchPoints);
  const hidden = isAppWindow(displayMode);
  const base = { platform, hidden, installPrompt: false, httpNote: false, fullscreenHint: false };
  switch (platform) {
    case "mac-safari":
      return { ...base, fullscreenHint: true, steps: ["app.mode.step.macSafari1", "app.mode.step.macSafari2", "app.mode.step.macSafari3"] };
    case "chromium":
      return secure
        ? { ...base, installPrompt: true, fullscreenHint: true, steps: ["app.mode.step.chromiumSecure1", "app.mode.step.chromiumSecure2"] }
        : { ...base, httpNote: true, fullscreenHint: true, steps: ["app.mode.step.chromiumHttp1", "app.mode.step.chromiumHttp2"] };
    case "firefox":
      return { ...base, fullscreenHint: true, steps: ["app.mode.step.firefox1", "app.mode.step.firefox2"] };
    case "ipad":
    case "iphone":
      return { ...base, steps: ["app.mode.step.ios1", "app.mode.step.ios2", "app.mode.step.ios3", "app.mode.step.ios4"] };
    case "android":
      return secure
        ? { ...base, installPrompt: true, steps: ["app.mode.step.androidSecure1"] }
        : { ...base, httpNote: true, steps: ["app.mode.step.androidHttp1"] };
    default:
      return { ...base, fullscreenHint: true, steps: ["app.mode.step.other1"] };
  }
}

/** localStorage key: "Use as an app" was offered on this device (once, on the first touch visit). */
export const APP_MODE_OFFERED_KEY = "openetch.appModeOffered";

/** Offer the dialog by itself only on a touch device, in a browser tab, and only once. */
export function shouldOfferAppMode(coarse: boolean, displayMode: DisplayMode, alreadyOffered: boolean): boolean {
  return coarse && displayMode === "browser" && !alreadyOffered;
}

/**
 * Give the browser's and the app window's chrome (theme-color) the top bar's colour, which follows the theme. Call it
 * after the theme changes (Preferences > Theme); index.html has the light and dark defaults.
 */
export function syncThemeColor(): void {
  if (typeof document === "undefined") return;
  const bar = document.querySelector(".topbar") ?? document.documentElement;
  const bg = getComputedStyle(bar).backgroundColor;
  const color = bg && bg !== "rgba(0, 0, 0, 0)" && bg !== "transparent" ? bg : getComputedStyle(document.documentElement).getPropertyValue("--topbar-bg").trim();
  if (!color) return;
  for (const meta of document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')) meta.content = color;
}
