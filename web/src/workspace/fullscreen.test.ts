// Unit tests for workspace/fullscreen.ts: what the Fullscreen API offers (standard, webkit-prefixed, iPhone), the
// display mode from media queries, and F11 (the browser's own full screen) told apart from the page's.

import assert from "node:assert/strict";
import { test } from "node:test";
import { displayModeOf, fullscreenCaps, fullscreenKey, fullscreenStateOf, isAppWindow, type DisplayMode } from "./fullscreen.ts";

const fn = () => {};

test("fullscreenCaps: standard, webkit-prefixed, disabled and missing APIs", () => {
  // Chrome, Edge, Firefox, Safari 16.4+.
  assert.deepEqual(fullscreenCaps({ fullscreenEnabled: true, fullscreenElement: null, documentElement: { requestFullscreen: fn } }), {
    supported: true,
    active: false,
    prefixed: false,
  });
  // Older Safari: only the webkit names.
  assert.deepEqual(
    fullscreenCaps({ webkitFullscreenEnabled: true, webkitFullscreenElement: {}, documentElement: { webkitRequestFullscreen: fn } }),
    { supported: true, active: true, prefixed: true },
  );
  // iPhone Safari and iOS Home Screen apps: the method may exist, but full screen is not enabled.
  assert.equal(fullscreenCaps({ webkitFullscreenEnabled: false, documentElement: { webkitRequestFullscreen: fn } }).supported, false);
  assert.equal(fullscreenCaps({ fullscreenEnabled: false, documentElement: { requestFullscreen: fn } }).supported, false);
  // Enabled but no method (a policy-blocked frame, or a fake): unsupported.
  assert.equal(fullscreenCaps({ fullscreenEnabled: true, documentElement: {} }).supported, false);
  // No document at all (node, SSR).
  assert.deepEqual(fullscreenCaps(undefined), { supported: false, active: false, prefixed: false });
  assert.equal(fullscreenCaps({ fullscreenEnabled: true, documentElement: null }).supported, false);
  // The standard name wins when both exist.
  const both = fullscreenCaps({ fullscreenEnabled: true, fullscreenElement: {}, documentElement: { requestFullscreen: fn, webkitRequestFullscreen: fn } });
  assert.deepEqual(both, { supported: true, active: true, prefixed: false });
});

test("displayModeOf: the first matching display mode, iOS standalone, else a browser tab", () => {
  const only = (mode: string) => (q: string) => q === `(display-mode: ${mode})`;
  assert.equal(displayModeOf(() => false), "browser");
  assert.equal(displayModeOf(only("standalone")), "standalone");
  assert.equal(displayModeOf(only("minimal-ui")), "minimal-ui");
  assert.equal(displayModeOf(only("fullscreen")), "fullscreen");
  assert.equal(displayModeOf(only("window-controls-overlay")), "window-controls-overlay");
  // An installed window with the title-bar overlay also matches standalone in some browsers: the overlay wins.
  assert.equal(displayModeOf((q) => q.includes("window-controls-overlay") || q.includes("standalone")), "window-controls-overlay");
  // Old iOS Home Screen apps answer only navigator.standalone.
  assert.equal(displayModeOf(() => false, true), "standalone");
  const queries: string[] = [];
  displayModeOf((q) => (queries.push(q), false));
  assert.deepEqual(queries, [
    "(display-mode: window-controls-overlay)",
    "(display-mode: fullscreen)",
    "(display-mode: standalone)",
    "(display-mode: minimal-ui)",
  ]);
});

test("fullscreenStateOf: F11 is the browser's (active, browser-owned); element full screen is the page's", () => {
  const caps = (active: boolean) => ({ supported: true, active, prefixed: false });
  assert.deepEqual(fullscreenStateOf(caps(false), "browser"), { supported: true, active: false, browserOwned: false });
  assert.deepEqual(fullscreenStateOf(caps(true), "fullscreen"), { supported: true, active: true, browserOwned: false });
  assert.deepEqual(fullscreenStateOf(caps(false), "fullscreen"), { supported: true, active: true, browserOwned: true });
  // An app window is not full screen.
  assert.deepEqual(fullscreenStateOf(caps(false), "standalone"), { supported: true, active: false, browserOwned: false });
  // iPhone: unsupported, and never browser-owned.
  const iphone = { supported: false, active: false, prefixed: false };
  assert.deepEqual(fullscreenStateOf(iphone, "standalone"), { supported: false, active: false, browserOwned: false });
});

test("isAppWindow: installed windows only", () => {
  const cases: [DisplayMode, boolean][] = [
    ["browser", false],
    ["fullscreen", false],
    ["standalone", true],
    ["minimal-ui", true],
    ["window-controls-overlay", true],
  ];
  for (const [m, want] of cases) assert.equal(isAppWindow(m), want, m);
});

test("the hotkey lets the key through where there is no full screen (no DOM)", () => {
  assert.equal(fullscreenKey(), false);
});
