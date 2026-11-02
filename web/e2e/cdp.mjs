// A small, dependency-free Chrome DevTools Protocol client for the device-emulation harness (web/e2e/README.md).
// Node 22: global WebSocket and fetch, no npm packages. It launches Microsoft Edge (or Chrome) headless, opens one
// isolated browser context per page (fresh localStorage and cookies), and drives it with emulated devices, touch,
// mouse, wheel, keys and screenshots.

import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const DEFAULT_BROWSERS = [
  process.env.E2E_BROWSER,
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/microsoft-edge",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
].filter(Boolean);

/** The first browser binary that exists (E2E_BROWSER overrides). */
export function findBrowser() {
  const b = DEFAULT_BROWSERS.find((p) => existsSync(p));
  if (!b) throw new Error("no Edge or Chrome found: set E2E_BROWSER to the browser's executable");
  return b;
}

/** A CDP connection over one WebSocket, with flat sessions (`sessionId` on each message). */
export class Connection {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.listeners = new Set();
    ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(typeof ev.data === "string" ? ev.data : Buffer.from(ev.data).toString());
      if (msg.id !== undefined) {
        const p = this.pending.get(msg.id);
        if (!p) return;
        this.pending.delete(msg.id);
        if (msg.error) p.reject(new Error(`${p.method}: ${msg.error.message}${msg.error.data ? ` (${msg.error.data})` : ""}`));
        else p.resolve(msg.result);
      } else {
        for (const l of this.listeners) l(msg);
      }
    });
  }

  static async open(url) {
    const ws = new WebSocket(url);
    await new Promise((res, rej) => {
      ws.addEventListener("open", res, { once: true });
      ws.addEventListener("error", () => rej(new Error(`cannot connect to ${url}`)), { once: true });
    });
    return new Connection(ws);
  }

  send(method, params = {}, sessionId) {
    const id = ++this.id;
    const msg = { id, method, params };
    if (sessionId) msg.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject, method });
      this.ws.send(JSON.stringify(msg));
    });
  }

  on(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  close() {
    try {
      this.ws.close();
    } catch {
      /* already closed */
    }
  }
}

/** Launch the browser headless with remote debugging and connect to it. */
export async function launch({ port = 9333, headless = true, userDataDir, browser = findBrowser() } = {}) {
  const dir = userDataDir ?? mkdtempSync(join(tmpdir(), "oe-e2e-"));
  const args = [
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${dir}`,
    "--hide-scrollbars",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-features=Translate,msEdgeSidebarV2,msUndersideButton",
    "about:blank",
  ];
  if (headless) args.unshift("--headless=new");
  const proc = spawn(browser, args, { stdio: "ignore" });
  let ver;
  for (let i = 0; i < 100; i++) {
    try {
      ver = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
      break;
    } catch {
      await sleep(100);
    }
  }
  if (!ver) {
    proc.kill();
    throw new Error(`the browser did not open its debugging port ${port}`);
  }
  const conn = await Connection.open(ver.webSocketDebuggerUrl);
  return {
    conn,
    proc,
    /** A new page in its own browser context (fresh storage). */
    newPage: () => Page.create(conn),
    async close() {
      try {
        await conn.send("Browser.close");
      } catch {
        /* gone */
      }
      conn.close();
      await sleep(300);
      try {
        proc.kill();
      } catch {
        /* gone */
      }
      if (!userDataDir) {
        try {
          rmSync(dir, { recursive: true, force: true });
        } catch {
          /* still locked: the OS temp cleaner gets it */
        }
      }
    },
  };
}

/** One tab in its own browser context. */
export class Page {
  static async create(conn) {
    const { browserContextId } = await conn.send("Target.createBrowserContext", { disposeOnDetach: true });
    const { targetId } = await conn.send("Target.createTarget", { url: "about:blank", browserContextId });
    const { sessionId } = await conn.send("Target.attachToTarget", { targetId, flatten: true });
    const p = new Page(conn, sessionId, targetId, browserContextId);
    await p.send("Page.enable");
    await p.send("Runtime.enable");
    p.conn.on((m) => {
      if (m.sessionId !== sessionId) return;
      if (m.method === "Runtime.exceptionThrown") p.errors.push(m.params.exceptionDetails?.exception?.description ?? m.params.exceptionDetails?.text);
      if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error")
        p.errors.push(m.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
      if (m.method === "Page.javascriptDialogOpening") p.send("Page.handleJavaScriptDialog", { accept: false }).catch(() => {});
    });
    return p;
  }

  constructor(conn, sessionId, targetId, browserContextId) {
    this.conn = conn;
    this.sessionId = sessionId;
    this.targetId = targetId;
    this.browserContextId = browserContextId;
    this.device = null;
    this.errors = [];
  }

  send(method, params = {}) {
    return this.conn.send(method, params, this.sessionId);
  }

  async close() {
    try {
      await this.conn.send("Target.closeTarget", { targetId: this.targetId });
      await this.conn.send("Target.disposeBrowserContext", { browserContextId: this.browserContextId });
    } catch {
      /* gone */
    }
  }

  /** Emulate a device from devices.mjs (size, DPR, touch, UA, colour scheme). Call before `goto`. */
  async setDevice(d, { theme = "dark" } = {}) {
    this.device = d;
    await this.send("Emulation.setDeviceMetricsOverride", {
      width: d.width,
      height: d.height,
      deviceScaleFactor: d.dpr,
      mobile: !!d.mobile,
      screenWidth: d.screen?.[0] ?? d.width,
      screenHeight: d.screen?.[1] ?? d.height,
      screenOrientation: d.width > d.height ? { type: "landscapePrimary", angle: 90 } : { type: "portraitPrimary", angle: 0 },
    });
    await this.send("Emulation.setTouchEmulationEnabled", d.touch ? { enabled: true, maxTouchPoints: 5 } : { enabled: false });
    if (d.ua) await this.send("Emulation.setUserAgentOverride", { userAgent: d.ua, platform: d.platform ?? "" });
    await this.send("Emulation.setEmulatedMedia", {
      features: [
        { name: "prefers-color-scheme", value: theme },
        { name: "prefers-reduced-motion", value: "reduce" },
      ],
    });
  }

  /** Change only the viewport height (the on-screen keyboard case), keeping the rest of the device. */
  async setViewportHeight(h) {
    const d = this.device;
    await this.send("Emulation.setDeviceMetricsOverride", {
      width: d.width,
      height: h,
      deviceScaleFactor: d.dpr,
      mobile: !!d.mobile,
      screenWidth: d.screen?.[0] ?? d.width,
      screenHeight: d.screen?.[1] ?? d.height,
    });
  }

  async goto(url, { waitFor, timeout = 20000 } = {}) {
    const loaded = new Promise((res) => {
      const off = this.conn.on((m) => {
        if (m.sessionId === this.sessionId && m.method === "Page.loadEventFired") {
          off();
          res();
        }
      });
    });
    await this.send("Page.navigate", { url });
    await Promise.race([loaded, sleep(timeout)]);
    if (waitFor) await this.waitFor(waitFor, { timeout });
  }

  /** Evaluate an expression (or a function and its JSON arguments) in the page; returns the value. */
  async evaluate(fnOrExpr, ...args) {
    const expression = typeof fnOrExpr === "function" ? `(${fnOrExpr})(...${JSON.stringify(args)})` : fnOrExpr;
    const r = await this.send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true, userGesture: true });
    if (r.exceptionDetails) throw new Error(`evaluate: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value;
  }

  /** Wait until a CSS selector matches a visible element (or a JS expression is truthy). */
  async waitFor(selectorOrExpr, { timeout = 10000, expr = false } = {}) {
    const t0 = Date.now();
    const test = expr
      ? selectorOrExpr
      : `(() => { const e = document.querySelector(${JSON.stringify(selectorOrExpr)}); if (!e) return false; const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; })()`;
    while (Date.now() - t0 < timeout) {
      if (await this.evaluate(test).catch(() => false)) return true;
      await sleep(100);
    }
    return false;
  }

  /** The centre of the first visible element matching `selector` (and optionally containing `text`), or null. */
  async center(selector, text) {
    return this.evaluate(
      (sel, txt) => {
        const els = [...document.querySelectorAll(sel)].filter((e) => {
          const r = e.getBoundingClientRect();
          if (r.width <= 0 || r.height <= 0) return false;
          if (txt == null) return true;
          const s = `${e.textContent ?? ""} ${e.getAttribute("aria-label") ?? ""} ${e.getAttribute("title") ?? ""}`.toLowerCase();
          return s.includes(txt.toLowerCase());
        });
        if (!els.length) return null;
        const r = els[0].getBoundingClientRect();
        return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height };
      },
      selector,
      text ?? null,
    );
  }

  // --- input ---------------------------------------------------------------------------------------------------

  async mouse(type, x, y, extra = {}) {
    await this.send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1, ...extra });
  }

  async click(x, y, { button = "left", modifiers = 0, clickCount = 1 } = {}) {
    await this.mouse("mouseMoved", x, y, { button: "none", modifiers });
    await this.mouse("mousePressed", x, y, { button, modifiers, clickCount, buttons: button === "right" ? 2 : 1 });
    await this.mouse("mouseReleased", x, y, { button, modifiers, clickCount, buttons: 0 });
  }

  async touch(type, points) {
    await this.send("Input.dispatchTouchEvent", {
      type,
      touchPoints: points.map((p, i) => ({ x: p.x, y: p.y, id: p.id ?? i, radiusX: 4, radiusY: 4, force: 1 })),
    });
  }

  async tap(x, y) {
    await this.touch("touchStart", [{ x, y }]);
    await sleep(40);
    await this.touch("touchEnd", []);
  }

  /** Tap on touch devices, click otherwise. */
  async press(x, y) {
    if (this.device?.touch) await this.tap(x, y);
    else await this.click(x, y);
  }

  /** Press the first visible element matching selector (and text); returns false when there is none. */
  async pressSel(selector, text) {
    const c = await this.center(selector, text);
    if (!c) return false;
    await this.press(c.x, c.y);
    return true;
  }

  async longPress(x, y, ms = 700) {
    await this.touch("touchStart", [{ x, y }]);
    await sleep(ms);
    await this.touch("touchEnd", []);
  }

  /** Drag with the mouse (or one finger on touch devices) in `steps` moves. */
  async drag(x1, y1, x2, y2, { steps = 12, touch = this.device?.touch, modifiers = 0, hold = 0 } = {}) {
    if (touch) {
      await this.touch("touchStart", [{ x: x1, y: y1 }]);
      if (hold) await sleep(hold);
      for (let i = 1; i <= steps; i++) {
        await this.touch("touchMove", [{ x: x1 + ((x2 - x1) * i) / steps, y: y1 + ((y2 - y1) * i) / steps }]);
        await sleep(16);
      }
      await this.touch("touchEnd", []);
    } else {
      await this.mouse("mouseMoved", x1, y1, { button: "none", modifiers });
      await this.mouse("mousePressed", x1, y1, { modifiers, buttons: 1 });
      if (hold) await sleep(hold);
      for (let i = 1; i <= steps; i++) {
        await this.mouse("mouseMoved", x1 + ((x2 - x1) * i) / steps, y1 + ((y2 - y1) * i) / steps, { modifiers, buttons: 1 });
        await sleep(16);
      }
      await this.mouse("mouseReleased", x2, y2, { modifiers, buttons: 0 });
    }
  }

  /** Two-finger pinch around (cx, cy): the fingers go from `from` px apart to `to` px apart (horizontal). */
  async pinch(cx, cy, from, to, { steps = 10 } = {}) {
    const pts = (d) => [
      { x: cx - d / 2, y: cy, id: 0 },
      { x: cx + d / 2, y: cy, id: 1 },
    ];
    await this.touch("touchStart", [pts(from)[0]]);
    await this.touch("touchStart", pts(from));
    for (let i = 1; i <= steps; i++) {
      await this.touch("touchMove", pts(from + ((to - from) * i) / steps));
      await sleep(16);
    }
    await this.touch("touchEnd", [pts(to)[1]]);
    await this.touch("touchEnd", []);
  }

  /** Mouse wheel at (x, y); negative deltaY zooms in on most canvases. `ctrl` sends a trackpad-pinch style wheel. */
  async wheel(x, y, deltaY, { ctrl = false, deltaX = 0 } = {}) {
    await this.mouse("mouseMoved", x, y, { button: "none" });
    await this.send("Input.dispatchMouseEvent", { type: "mouseWheel", x, y, deltaX, deltaY, modifiers: ctrl ? 2 : 0 });
  }

  /** A key press: `key` as in KeyboardEvent.key ("Escape", "Delete", "r", "Enter"); modifiers bit mask
   *  (1 Alt, 2 Ctrl, 4 Meta, 8 Shift). */
  async key(key, { modifiers = 0, code } = {}) {
    const named = {
      Escape: [27, "Escape"],
      Enter: [13, "Enter"],
      Delete: [46, "Delete"],
      Backspace: [8, "Backspace"],
      Tab: [9, "Tab"],
      ArrowLeft: [37, "ArrowLeft"],
      ArrowUp: [38, "ArrowUp"],
      ArrowRight: [39, "ArrowRight"],
      ArrowDown: [40, "ArrowDown"],
      Shift: [16, "ShiftLeft"],
      Control: [17, "ControlLeft"],
    };
    const n = named[key];
    const single = !n && key.length === 1;
    const vk = n ? n[0] : single ? key.toUpperCase().charCodeAt(0) : 0;
    const c = code ?? (n ? n[1] : single && /[a-z]/i.test(key) ? `Key${key.toUpperCase()}` : single && /\d/.test(key) ? `Digit${key}` : key);
    const text = single && !(modifiers & 6) ? key : undefined;
    await this.send("Input.dispatchKeyEvent", { type: text ? "keyDown" : "rawKeyDown", key, code: c, windowsVirtualKeyCode: vk, modifiers, text });
    await this.send("Input.dispatchKeyEvent", { type: "keyUp", key, code: c, windowsVirtualKeyCode: vk, modifiers });
  }

  async type(text) {
    await this.send("Input.insertText", { text });
  }

  /** Save a PNG of the viewport. */
  async screenshot(path) {
    const { data } = await this.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, Buffer.from(data, "base64"));
    return path;
  }

  /** Wait for two animation frames (layout and paint settle). */
  async settle(ms = 120) {
    await this.evaluate("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))").catch(() => {});
    if (ms) await sleep(ms);
  }
}
