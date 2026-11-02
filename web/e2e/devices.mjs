// The device matrix for the emulation harness (web/e2e/README.md). Sizes are the CSS viewport the page actually gets
// (browser bars already subtracted), `dpr` the device pixel ratio, `touch` a coarse primary pointer with touch events,
// `safe` the env(safe-area-inset-*) values the device reports to a page with viewport-fit=cover (the harness sets them
// through the --safe-* tokens, see scenarios.mjs `applySafeArea`; headless Chromium reports 0).
//
// Safe areas: in Safari with its bars showing, the bars cover the notch and the home indicator, so the page gets 0.
// With the bars collapsed the page reaches the home-indicator band (bottom 34 in portrait). As a Home Screen app
// (standalone, black-translucent status bar) the page is the whole screen: top 59 (Dynamic Island) and bottom 34.

const IOS_UA =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
// iPadOS Safari asks for desktop sites: it reports a Mac, and only the touch points give it away.
const IPADOS_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15";
const ANDROID_UA =
  "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36";
const MAC_SAFARI_UA = IPADOS_UA;
const MAC_CHROME_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

const NO_SAFE = { top: 0, right: 0, bottom: 0, left: 0 };

/** @typedef {{ id: string, name: string, width: number, height: number, dpr: number, touch: boolean, mobile: boolean,
 *   ua?: string, platform?: string, safe: {top:number,right:number,bottom:number,left:number}, screen?: [number, number],
 *   kind: "phone" | "tablet" | "desktop", themes: ("dark"|"light")[], keyboard?: number }} Device */

/** @type {Device[]} */
export const DEVICES = [
  {
    id: "iphone15-portrait",
    name: "iPhone 15 Safari portrait (bars shown)",
    width: 393,
    height: 659,
    dpr: 3,
    touch: true,
    mobile: true,
    ua: IOS_UA,
    platform: "iPhone",
    safe: NO_SAFE,
    screen: [393, 852],
    kind: "phone",
    themes: ["dark", "light"],
    keyboard: 336,
  },
  {
    id: "iphone15-collapsed",
    name: "iPhone 15 Safari portrait (bars collapsed)",
    width: 393,
    height: 759,
    dpr: 3,
    touch: true,
    mobile: true,
    ua: IOS_UA,
    platform: "iPhone",
    safe: { top: 0, right: 0, bottom: 34, left: 0 },
    screen: [393, 852],
    kind: "phone",
    themes: ["dark"],
    keyboard: 336,
  },
  {
    id: "iphone15-standalone",
    name: "iPhone 15 Home Screen app (safe areas 59/34)",
    width: 393,
    height: 852,
    dpr: 3,
    touch: true,
    mobile: true,
    ua: IOS_UA,
    platform: "iPhone",
    safe: { top: 59, right: 0, bottom: 34, left: 0 },
    screen: [393, 852],
    kind: "phone",
    themes: ["dark"],
    keyboard: 336,
  },
  {
    id: "iphone15-landscape",
    name: "iPhone 15 Safari landscape",
    width: 852,
    height: 330,
    dpr: 3,
    touch: true,
    mobile: true,
    ua: IOS_UA,
    platform: "iPhone",
    // Safari landscape with viewport-fit=cover: the Dynamic Island side and the home indicator.
    safe: { top: 0, right: 59, bottom: 21, left: 59 },
    screen: [852, 393],
    kind: "phone",
    themes: ["dark"],
    keyboard: 200,
  },
  {
    id: "iphone-se",
    name: "iPhone SE Safari portrait",
    width: 375,
    height: 553,
    dpr: 2,
    touch: true,
    mobile: true,
    ua: IOS_UA,
    platform: "iPhone",
    safe: NO_SAFE,
    screen: [375, 667],
    kind: "phone",
    themes: ["dark"],
    keyboard: 260,
  },
  {
    id: "pixel8",
    name: "Pixel 8 Chrome portrait",
    width: 412,
    height: 839,
    dpr: 2.625,
    touch: true,
    mobile: true,
    ua: ANDROID_UA,
    platform: "Linux armv8l",
    safe: NO_SAFE,
    screen: [412, 915],
    kind: "phone",
    themes: ["dark"],
    keyboard: 320,
  },
  {
    id: "ipad-air-portrait",
    name: "iPad Air Safari portrait",
    width: 820,
    height: 1106,
    dpr: 2,
    touch: true,
    mobile: true,
    ua: IPADOS_UA,
    platform: "MacIntel",
    safe: { top: 0, right: 0, bottom: 20, left: 0 },
    screen: [820, 1180],
    kind: "tablet",
    themes: ["dark", "light"],
    keyboard: 400,
  },
  {
    id: "ipad-air-landscape",
    name: "iPad Air Safari landscape",
    width: 1180,
    height: 746,
    dpr: 2,
    touch: true,
    mobile: true,
    ua: IPADOS_UA,
    platform: "MacIntel",
    safe: { top: 0, right: 0, bottom: 20, left: 0 },
    screen: [1180, 820],
    kind: "tablet",
    themes: ["dark"],
    keyboard: 400,
  },
  {
    id: "macbook-air-1470",
    name: "MacBook Air 13 Safari 1470x832",
    width: 1470,
    height: 832,
    dpr: 2,
    touch: false,
    mobile: false,
    ua: MAC_SAFARI_UA,
    platform: "MacIntel",
    safe: NO_SAFE,
    kind: "desktop",
    themes: ["dark", "light"],
  },
  {
    id: "macbook-air-1280",
    name: "MacBook Air 13 Chrome 1280x720",
    width: 1280,
    height: 720,
    dpr: 2,
    touch: false,
    mobile: false,
    ua: MAC_CHROME_UA,
    platform: "MacIntel",
    safe: NO_SAFE,
    kind: "desktop",
    themes: ["dark"],
  },
  {
    id: "win-1920",
    name: "Windows Edge 1920x969 @1.25",
    width: 1920,
    height: 969,
    dpr: 1.25,
    touch: false,
    mobile: false,
    safe: NO_SAFE,
    kind: "desktop",
    themes: ["dark"],
  },
  {
    id: "win-2560",
    name: "Windows Edge 2560x1307 @1.5",
    width: 2560,
    height: 1307,
    dpr: 1.5,
    touch: false,
    mobile: false,
    safe: NO_SAFE,
    kind: "desktop",
    themes: ["dark"],
  },
];

/** Devices whose id matches any of the comma-separated filters (substring), or all. */
export function pickDevices(filter) {
  if (!filter) return DEVICES;
  const parts = filter.split(",").map((s) => s.trim()).filter(Boolean);
  return DEVICES.filter((d) => parts.some((p) => d.id.includes(p) || d.kind === p));
}
