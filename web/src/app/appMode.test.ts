// Unit tests for app/appMode.ts: the platform from the user agent and the "Use as an app" steps for Safari on a Mac,
// Chrome/Edge on Windows (http and https), iPad, iPhone, Android and Firefox; hidden in an installed app window.

import assert from "node:assert/strict";
import { test } from "node:test";
import { detectPlatform, installGuide, shouldOfferAppMode } from "./appMode.ts";

const UA = {
  macSafari: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/19.1 Safari/605.1.15",
  macChrome: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
  winChrome: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
  winEdge: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0",
  winFirefox: "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:144.0) Gecko/20100101 Firefox/144.0",
  ipad: "Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
  iphone: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
  iphoneChrome: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/141.0 Mobile/15E148 Safari/604.1",
  android: "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36",
};

test("detectPlatform", () => {
  assert.equal(detectPlatform(UA.macSafari), "mac-safari");
  // iPadOS Safari asks for the desktop site by default: a Mac user agent with touch points.
  assert.equal(detectPlatform(UA.macSafari, 5), "ipad");
  assert.equal(detectPlatform(UA.macChrome), "chromium");
  assert.equal(detectPlatform(UA.winChrome), "chromium");
  assert.equal(detectPlatform(UA.winEdge), "chromium");
  assert.equal(detectPlatform(UA.winFirefox), "firefox");
  assert.equal(detectPlatform(UA.ipad), "ipad");
  assert.equal(detectPlatform(UA.iphone), "iphone");
  // Every iPhone browser is Safari underneath: Add to Home Screen is the route.
  assert.equal(detectPlatform(UA.iphoneChrome), "iphone");
  assert.equal(detectPlatform(UA.android), "android");
  assert.equal(detectPlatform("curl/8.0"), "other");
});

test("Safari on a Mac: File > Add to Dock, also over http", () => {
  const g = installGuide(UA.macSafari, "browser", false);
  assert.equal(g.platform, "mac-safari");
  assert.equal(g.hidden, false);
  assert.deepEqual(g.steps, ["app.mode.step.macSafari1", "app.mode.step.macSafari2", "app.mode.step.macSafari3"]);
  assert.equal(g.httpNote, false);
  assert.equal(g.fullscreenHint, true);
});

test("Chrome/Edge on Windows: Open as window on http (with the HTTPS note), Install on https", () => {
  const http = installGuide(UA.winChrome, "browser", false);
  assert.deepEqual(http.steps, ["app.mode.step.chromiumHttp1", "app.mode.step.chromiumHttp2"]);
  assert.equal(http.httpNote, true);
  assert.equal(http.installPrompt, false);
  const https = installGuide(UA.winEdge, "browser", true);
  assert.deepEqual(https.steps, ["app.mode.step.chromiumSecure1", "app.mode.step.chromiumSecure2"]);
  assert.equal(https.httpNote, false);
  assert.equal(https.installPrompt, true);
});

test("iPad and iPhone: Add to Home Screen; no full-screen hint on the iPhone", () => {
  for (const [ua, touch, platform] of [
    [UA.ipad, 5, "ipad"],
    [UA.macSafari, 5, "ipad"],
    [UA.iphone, 5, "iphone"],
  ] as const) {
    const g = installGuide(ua, "browser", false, touch);
    assert.equal(g.platform, platform);
    assert.deepEqual(g.steps, ["app.mode.step.ios1", "app.mode.step.ios2", "app.mode.step.ios3", "app.mode.step.ios4"]);
    assert.equal(g.httpNote, false, "Add to Home Screen works over http");
    assert.equal(g.fullscreenHint, false);
  }
});

test("Android: a shortcut on http (with the HTTPS note), Install app on https", () => {
  const http = installGuide(UA.android, "browser", false);
  assert.deepEqual([http.steps, http.httpNote], [["app.mode.step.androidHttp1"], true]);
  const https = installGuide(UA.android, "browser", true);
  assert.deepEqual([https.steps, https.installPrompt], [["app.mode.step.androidSecure1"], true]);
});

test("Firefox: no app window; full screen offered instead", () => {
  const g = installGuide(UA.winFirefox, "browser", false);
  assert.deepEqual(g.steps, ["app.mode.step.firefox1", "app.mode.step.firefox2"]);
  assert.equal(g.fullscreenHint, true);
});

test("already an app window: hidden, whatever the platform", () => {
  for (const mode of ["standalone", "window-controls-overlay", "minimal-ui"] as const) {
    for (const ua of [UA.macSafari, UA.winChrome, UA.ipad, UA.iphone, UA.android]) {
      assert.equal(installGuide(ua, mode, true).hidden, true, `${mode} ${ua}`);
    }
  }
  // The browser's F11 full screen is still a browser tab.
  assert.equal(installGuide(UA.winChrome, "fullscreen", false).hidden, false);
  assert.equal(installGuide(UA.winChrome, "browser", false).hidden, false);
});

test("offered once, by itself, only on a touch device in a browser tab", () => {
  assert.equal(shouldOfferAppMode(true, "browser", false), true);
  assert.equal(shouldOfferAppMode(true, "browser", true), false);
  assert.equal(shouldOfferAppMode(false, "browser", false), false);
  assert.equal(shouldOfferAppMode(true, "standalone", false), false);
});
