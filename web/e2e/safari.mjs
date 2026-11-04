#!/usr/bin/env node
// Real Safari (web/e2e/README.md "Real Safari"): Mobile Safari in the iOS simulators of a Mac, driven over W3C
// WebDriver (safaridriver), with the same in-page layout check (overlap.mjs) and the same step helpers as the
// device-emulation harness (scenarios.mjs). A screenshot after every step; report.json and report.md like
// scenarios.mjs.
//
//   node e2e/safari.mjs --port 7965 --mac <host> [--driver-port 4479] [--devices iphone,ipad]
//                       [--steps guide,cockpit] [--out dir] [--no-tunnel] [--input native|synthetic|auto]
//                       [--desktop] [--keep-booted]
//
//   node e2e/safari.mjs --design --port 7976 --mac <host> [--dir dist-design] [--base /OpenEtch/] [--devices ...]
//   node e2e/safari.mjs --url https://<owner>.github.io/OpenEtch/ --mac <host> [--devices ...]   (the published site)
//
// --design: OpenEtch Design, the published static site (web/dist-design, `npm run build:design`), served here under
// its base path (e2e/static.mjs) and reverse-forwarded the same way; no engine. The walk is the design one
// (examples, drawing, text in an uploaded font, a LightBurn import, saving in the browser, reload and recovery,
// Preview, the guide, no machine controls, nothing loaded from outside the site).
//
// The engine is a SIMULATOR engine on this computer at 127.0.0.1:<port> (the flows frame, start a run, press
// Continue and replace the document). The script opens one SSH connection to the Mac that runs `safaridriver -p
// <driver-port>` there, forwards that port here (-L) and forwards the engine's port to the Mac (-R), so Safari in the
// simulator opens http://127.0.0.1:<port>/. It ends the WebDriver session, stops its safaridriver and the forwards,
// and shuts down the simulators it booted. It never runs sudo or changes a setting on the Mac.

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { sleep } from "./cdp.mjs";
import { checkLayout } from "./overlap.mjs";
import { serveSite } from "./static.mjs";
import {
  api,
  canvasCenter,
  canvasPoint,
  escape,
  execute,
  find,
  menuPath,
  moreSheet,
  openSection,
  phoneTab,
  pressFind,
  selectSomething,
  selectionCount,
  sheetOpen,
  sheetTo,
} from "./scenarios.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));

/** Simulators by short name: [device name, iOS runtime prefix or null for the newest]. */
export const SIM_DEVICES = {
  iphone: ["iPhone 17e", "26"],
  "iphone-pro": ["iPhone 16 Pro", "18"],
  "iphone-16e": ["iPhone 16e", "18"],
  ipad: ["iPad Pro 11-inch (M4)", "18"],
  "ipad-13": ["iPad Pro 13-inch (M4)", "18"],
  "ipad-mini": ["iPad mini (A17 Pro)", "26"],
};

function args(argv) {
  const a = { mac: process.env.OE_SAFARI_MAC, driverPort: 4479, devices: "iphone,ipad", input: "auto" };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--port") a.port = Number(v());
    else if (k === "--mac") a.mac = v();
    else if (k === "--driver-port") a.driverPort = Number(v());
    else if (k === "--devices") a.devices = v();
    else if (k === "--steps") a.steps = v().split(",");
    else if (k === "--out") a.out = v();
    else if (k === "--no-tunnel") a.noTunnel = true;
    else if (k === "--input") a.input = v();
    else if (k === "--desktop") a.desktop = true;
    else if (k === "--keep-booted") a.keepBooted = true;
    else if (k === "--design") a.design = true;
    else if (k === "--dir") a.dir = v();
    else if (k === "--base") a.base = v();
    else if (k === "--url") {
      a.siteUrl = v().replace(/\/?$/, "/");
      a.design = true;
    }
    else if (k === "--help" || k === "-h") a.help = true;
  }
  // A published site needs no local port; the tunnel still forwards one (nothing listens on it).
  if (a.siteUrl) a.port ??= 7976;
  return a;
}

// --- the Mac ---------------------------------------------------------------------------------------------------

const SSH = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"];

function mac(host, cmd, { allowFail = false } = {}) {
  try {
    return execFileSync("ssh", [...SSH, host, cmd], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  } catch (e) {
    if (allowFail) return e.stdout ?? "";
    throw new Error(`ssh ${host} "${cmd}": ${e.stderr || e.message}`);
  }
}

/** All available simulators: { udid, name, runtime ("18.5"), state }. */
function simulators(host) {
  const j = JSON.parse(mac(host, "xcrun simctl list devices available -j"));
  const out = [];
  for (const [rt, list] of Object.entries(j.devices)) {
    const m = rt.match(/iOS-(\d+)-(\d+)/);
    if (!m) continue;
    for (const d of list) out.push({ udid: d.udid, name: d.name, runtime: `${m[1]}.${m[2]}`, state: d.state });
  }
  return out;
}

function pickSim(sims, key) {
  const [name, rt] = SIM_DEVICES[key] ?? [key.replace(/@.*/, ""), key.includes("@") ? key.split("@")[1] : null];
  const c = sims.filter((s) => s.name === name && (!rt || s.runtime.startsWith(rt)));
  if (!c.length) throw new Error(`no simulator "${name}"${rt ? ` with iOS ${rt}` : ""} on the Mac`);
  // The newest runtime of the matches.
  return c.sort((x, y) => y.runtime.localeCompare(x.runtime, undefined, { numeric: true }))[0];
}

/** One SSH connection: safaridriver on the Mac, its port forwarded here and the engine's port forwarded there.
 *  `caffeinate -i` keeps the Mac from idle sleep while safaridriver runs (an assertion that ends with it, no setting
 *  changes): a Mac set to sleep after a minute otherwise drops off the network mid-run. */
export async function openTunnel(a) {
  const running = mac(a.mac, "pgrep -fl safaridriver || true").trim();
  if (running) throw new Error(`safaridriver already runs on the Mac (one WebDriver session at a time):\n${running}`);
  const proc = spawn(
    "ssh",
    [...SSH, "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=15", "-L", `${a.driverPort}:127.0.0.1:${a.driverPort}`, "-R", `${a.port}:127.0.0.1:${a.port}`, a.mac, `exec caffeinate -i safaridriver -p ${a.driverPort}`],
    { stdio: ["ignore", "ignore", "pipe"] },
  );
  let err = "";
  proc.stderr.on("data", (d) => (err += d));
  for (let i = 0; i < 100; i++) {
    if (proc.exitCode !== null) throw new Error(`the SSH tunnel ended: ${err}`);
    try {
      const s = await (await fetch(`http://127.0.0.1:${a.driverPort}/status`)).json();
      if (s.value?.ready) return proc;
    } catch {
      /* not yet */
    }
    await sleep(200);
  }
  proc.kill();
  throw new Error(`safaridriver did not answer on ${a.driverPort}: ${err}`);
}

export async function closeTunnel(a, proc) {
  try {
    proc?.kill();
  } catch {
    /* gone */
  }
  await sleep(500);
  // Only this run's safaridriver (by its port), never another one.
  mac(a.mac, `for p in $(pgrep -x safaridriver); do ps -o args= -p $p | grep -qE -- "-p ${a.driverPort}( |$)" && kill $p; done; true`, { allowFail: true });
}

// --- WebDriver ---------------------------------------------------------------------------------------------------

export class WebDriver {
  constructor(base) {
    this.base = base;
    this.id = null;
  }

  async cmd(method, path, body) {
    const url = `${this.base}${this.id ? `/session/${this.id}` : ""}${path}`;
    // A command that never answers (a touch the simulator does not finish) becomes an error, not a hang.
    const r = await fetch(url, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(90000) });
    const j = await r.json().catch(() => ({}));
    if (j.value && j.value.error) throw new Error(`${method} ${path}: ${j.value.error}: ${j.value.message}`);
    return j.value;
  }

  async start(capabilities) {
    const r = await fetch(`${this.base}/session`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ capabilities: { alwaysMatch: capabilities } }) }).catch((e) => {
      // The tunnel or safaridriver went away: say why (fetch's own message is only "fetch failed").
      throw new Error(`new session: ${e.message}${e.cause ? ` (${e.cause.code ?? e.cause.message ?? e.cause})` : ""}`);
    });
    const j = await r.json();
    if (j.value?.error) throw new Error(`new session: ${j.value.error}: ${j.value.message}`);
    this.id = j.value.sessionId;
    this.caps = j.value.capabilities;
    await this.cmd("POST", "/timeouts", { script: 60000, pageLoad: 60000, implicit: 0 });
    return this.caps;
  }

  async end() {
    if (!this.id) return;
    try {
      await this.cmd("DELETE", "");
    } catch {
      /* gone */
    }
    this.id = null;
  }
}

// --- synthetic input (in the page) -------------------------------------------------------------------------------

/** Installed in the page when the simulator does not take WebDriver's touches (a locked Mac): pointer events of type
 *  "touch" dispatched at the hit-tested element, with pointer capture honoured and a click after a short tap, as
 *  Mobile Safari sends them. Untrusted events: no on-screen keyboard, no native scrolling or page zoom. */
function installSynth() {
  if (window.__oeSynth) return true;
  const captured = new Map();
  const ids = new Set();
  for (const m of ["setPointerCapture", "releasePointerCapture"]) {
    const orig = Element.prototype[m];
    Element.prototype[m] = function (id) {
      if (ids.has(id)) {
        if (m === "setPointerCapture") captured.set(id, this);
        else captured.delete(id);
        return;
      }
      return orig.call(this, id);
    };
  }
  const origHas = Element.prototype.hasPointerCapture;
  Element.prototype.hasPointerCapture = function (id) {
    return ids.has(id) ? captured.get(id) === this : origHas.call(this, id);
  };
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  const fire = (type, x, y, id, extra = {}) => {
    const target = captured.get(id) ?? document.elementFromPoint(x, y) ?? document.body;
    const ev = new PointerEvent(type, {
      bubbles: !/enter|leave/.test(type),
      cancelable: !/enter|leave|cancel/.test(type),
      composed: true,
      clientX: x,
      clientY: y,
      screenX: x,
      screenY: y,
      pointerId: id,
      pointerType: "touch",
      isPrimary: extra.isPrimary ?? true,
      button: type === "pointermove" ? -1 : 0,
      buttons: type === "pointerdown" || type === "pointermove" ? 1 : 0,
      width: 20,
      height: 20,
      pressure: type === "pointerup" ? 0 : 0.5,
    });
    const ok = target.dispatchEvent(ev);
    return { target, ok };
  };
  let next = 1000;
  const S = {
    /** Fingers: [[x, y], ...] per frame; a sequence of frames, `holdMs` before the first move. */
    async gesture(frames, { holdMs = 0, stepMs = 16 } = {}) {
      const n = frames[0].length;
      const fid = Array.from({ length: n }, () => next++);
      fid.forEach((id) => ids.add(id));
      const t0 = performance.now();
      const downs = [];
      frames[0].forEach(([x, y], i) => {
        const id = fid[i];
        fire("pointerover", x, y, id, { isPrimary: i === 0 });
        const d = fire("pointerdown", x, y, id, { isPrimary: i === 0 });
        downs.push(d);
        if (d.ok && i === 0) {
          const f = d.target.closest?.("input, textarea, select, [contenteditable=true], button, [tabindex]");
          if (f && f !== document.activeElement) f.focus?.({ preventScroll: true });
        }
      });
      if (holdMs) await wait(holdMs);
      for (let k = 1; k < frames.length; k++) {
        await wait(stepMs);
        frames[k].forEach(([x, y], i) => fire("pointermove", x, y, fid[i], { isPrimary: i === 0 }));
      }
      const last = frames[frames.length - 1];
      await wait(Math.max(0, 40 - (performance.now() - t0)));
      const ups = last.map(([x, y], i) => fire("pointerup", x, y, fid[i], { isPrimary: i === 0 }));
      last.forEach(([x, y], i) => {
        fire("pointerout", x, y, fid[i], { isPrimary: i === 0 });
        captured.delete(fid[i]);
      });
      // A one-finger tap that did not move: the compatibility mouse events and the click.
      const [x0, y0] = frames[0][0];
      const [x1, y1] = last[0];
      const moved = Math.hypot(x1 - x0, y1 - y0) > 10;
      const dt = performance.now() - t0;
      if (n === 1 && !moved && dt < 650 && downs[0].ok) {
        const t = document.elementFromPoint(x1, y1) ?? document.body;
        const opts = { bubbles: true, cancelable: true, composed: true, clientX: x1, clientY: y1, button: 0, detail: 1, view: window };
        t.dispatchEvent(new MouseEvent("mousedown", { ...opts, buttons: 1 }));
        t.dispatchEvent(new MouseEvent("mouseup", opts));
        t.dispatchEvent(new MouseEvent("click", opts));
      }
      return { down: downs[0].target?.tagName, up: ups[0].target?.tagName };
    },
    key(key, mods = {}) {
      const t = document.activeElement && document.activeElement !== document.body ? document.activeElement : document.querySelector(".viewport") ?? document.body;
      const o = { key, code: key.length === 1 ? `Key${key.toUpperCase()}` : key, bubbles: true, cancelable: true, composed: true, ...mods };
      const ok = t.dispatchEvent(new KeyboardEvent("keydown", o));
      t.dispatchEvent(new KeyboardEvent("keyup", o));
      return ok;
    },
  };
  window.__oeSynth = S;
  return true;
}

// Probe whether a WebDriver touch reaches the page.
// Probe whether a WebDriver touch reaches the page: a tap on a pad of its own over the page, which swallows the tap (the
// app under it gets nothing) and is removed afterwards.
function armProbe() {
  window.__oeProbe = [];
  const pad = document.createElement("div");
  pad.id = "oe-probe-pad";
  pad.style.cssText = "position:fixed;left:40%;top:45%;width:60px;height:60px;z-index:2147483647;background:transparent;touch-action:none";
  const f = (e) => {
    window.__oeProbe.push(`${e.type}:${e.pointerType ?? ""}`);
    e.stopPropagation();
  };
  for (const t of ["pointerdown", "pointerup", "click"]) pad.addEventListener(t, f);
  document.body.appendChild(pad);
  const r = pad.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
}

/** Errors and console errors, collected in the page (WebDriver has no console). */
function installErrorHook() {
  if (window.__oeErrors) return true;
  window.__oeErrors = [];
  addEventListener("error", (e) => window.__oeErrors.push(`error: ${e.message} ${e.filename ?? ""}:${e.lineno ?? ""}`));
  addEventListener("unhandledrejection", (e) => window.__oeErrors.push(`rejection: ${e.reason?.stack ?? e.reason}`));
  const ce = console.error.bind(console);
  console.error = (...a) => {
    window.__oeErrors.push(a.map((x) => (x instanceof Error ? x.stack ?? x.message : String(x))).join(" ").slice(0, 400));
    ce(...a);
  };
  return true;
}

/** Safari-specific checks of the current screen (run in the page). */
export function safariChecks() {
  const vv = window.visualViewport;
  const probe = document.createElement("div");
  probe.style.cssText =
    "position:fixed;top:0;left:0;width:0;height:0;visibility:hidden;padding:env(safe-area-inset-top) env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left)";
  document.body.appendChild(probe);
  const pc = getComputedStyle(probe);
  const env = { top: parseFloat(pc.paddingTop), right: parseFloat(pc.paddingRight), bottom: parseFloat(pc.paddingBottom), left: parseFloat(pc.paddingLeft) };
  probe.style.cssText = "position:fixed;top:0;left:0;width:0;height:100vh;visibility:hidden";
  const vh100 = probe.getBoundingClientRect().height;
  probe.style.height = "100dvh";
  const dvh100 = probe.getBoundingClientRect().height;
  probe.remove();
  const rs = getComputedStyle(document.documentElement);
  const tokens = { top: rs.getPropertyValue("--safe-top").trim(), bottom: rs.getPropertyValue("--safe-bottom").trim() };
  const app = document.querySelector(".app")?.getBoundingClientRect();
  const issues = [];
  // Fields under 16 px make iOS zoom the page in when they get focus.
  const small = [];
  for (const el of document.querySelectorAll("input:not([type=hidden]):not([type=checkbox]):not([type=radio]):not([type=range]):not([type=color]), select, textarea, [contenteditable=true]")) {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) continue;
    const fs = parseFloat(getComputedStyle(el).fontSize);
    if (fs < 15.5) small.push(`${el.tagName.toLowerCase()}${el.className ? "." + String(el.className).split(" ")[0] : ""} ${fs}px`);
  }
  if (small.length) issues.push({ type: "input-zoom", severity: "medium", detail: `${small.length} field(s) under 16 px (iOS zooms in on focus): ${small.slice(0, 6).join(", ")}` });
  if (vv && Math.abs(vv.scale - 1) > 0.01) issues.push({ type: "page-zoom", severity: "high", detail: `visual viewport scale ${vv.scale.toFixed(2)}` });
  if (scrollX || scrollY) issues.push({ type: "page-scrolled", severity: "medium", detail: `the page is scrolled to ${scrollX},${scrollY}` });
  const se = document.scrollingElement;
  if (se && se.scrollWidth > innerWidth + 1) issues.push({ type: "h-scroll", severity: "medium", detail: `the page is ${se.scrollWidth} px wide in a ${innerWidth} px window` });
  if (app && Math.abs(app.bottom - innerHeight) > 1.5) issues.push({ type: "app-height", severity: "medium", detail: `.app ends at ${Math.round(app.bottom)} in a ${innerHeight} px window (100vh / dvh)` });
  const nav = document.querySelector(".phone-tabs")?.getBoundingClientRect();
  return {
    inner: [innerWidth, innerHeight],
    visual: vv ? [Math.round(vv.width), Math.round(vv.height), Math.round(vv.offsetTop), +vv.scale.toFixed(2)] : null,
    screen: [screen.width, screen.height],
    vh100,
    dvh100,
    env,
    tokens,
    app: app ? [Math.round(app.top), Math.round(app.bottom)] : null,
    phoneTabs: nav ? [Math.round(nav.top), Math.round(nav.bottom)] : null,
    active: document.activeElement ? `${document.activeElement.tagName.toLowerCase()}.${String(document.activeElement.className).split(" ")[0]}` : null,
    standalone: matchMedia("(display-mode: standalone)").matches,
    issues,
  };
}

// --- the page --------------------------------------------------------------------------------------------------

const KEYS = { Escape: "", Enter: "", Delete: "", Backspace: "", Tab: "", ArrowLeft: "", ArrowUp: "", ArrowRight: "", ArrowDown: "", F1: "", Shift: "", Control: "" };

/** The cdp.mjs Page interface (what scenarios.mjs's helpers use) over WebDriver. */
export class SafariPage {
  constructor(wd, device) {
    this.wd = wd;
    this.device = device;
    this.input = "native";
    this.errors = [];
  }

  async evaluate(fnOrExpr, ...args) {
    const body =
      typeof fnOrExpr === "function"
        ? `(${fnOrExpr.toString()}).apply(null, args)`
        : `(${fnOrExpr})`;
    const script = `const done = arguments[arguments.length - 1]; const args = Array.prototype.slice.call(arguments, 0, -1);
      try { Promise.resolve(${body}).then((v) => done({ ok: v === undefined ? null : v }), (e) => done({ err: String((e && e.stack) || e) })); }
      catch (e) { done({ err: String((e && e.stack) || e) }); }`;
    const r = await this.wd.cmd("POST", "/execute/async", { script, args });
    if (r && r.err) throw new Error(`evaluate: ${r.err}`);
    return r ? r.ok : null;
  }

  async waitFor(selectorOrExpr, { timeout = 10000, expr = false } = {}) {
    const t0 = Date.now();
    const test = expr
      ? selectorOrExpr
      : `(() => { const e = document.querySelector(${JSON.stringify(selectorOrExpr)}); if (!e) return false; const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; })()`;
    while (Date.now() - t0 < timeout) {
      if (await this.evaluate(test).catch(() => false)) return true;
      await sleep(150);
    }
    return false;
  }

  async goto(url, { waitFor, timeout = 30000 } = {}) {
    await this.wd.cmd("POST", "/url", { url });
    if (waitFor) await this.waitFor(waitFor, { timeout });
    await this.afterLoad();
  }

  async afterLoad() {
    await this.evaluate(installErrorHook);
    if (this.input === "synthetic") await this.evaluate(installSynth);
  }

  async collectErrors() {
    const e = await this.evaluate(() => {
      const x = window.__oeErrors ?? [];
      window.__oeErrors = [];
      return x;
    }).catch(() => []);
    this.errors.push(...(e ?? []));
  }

  // W3C pointer actions with pointerType touch; `fingers` is a list of frame lists.
  async touchActions(frames, { holdMs = 0, stepMs = 16 } = {}) {
    const n = frames[0].length;
    const actions = [];
    for (let i = 0; i < n; i++) {
      const a = [{ type: "pointerMove", duration: 0, x: Math.round(frames[0][i][0]), y: Math.round(frames[0][i][1]) }, { type: "pointerDown", button: 0 }];
      if (holdMs) a.push({ type: "pause", duration: holdMs });
      for (let k = 1; k < frames.length; k++) a.push({ type: "pointerMove", duration: stepMs, x: Math.round(frames[k][i][0]), y: Math.round(frames[k][i][1]) });
      if (frames.length === 1 && !holdMs) a.push({ type: "pause", duration: 40 });
      a.push({ type: "pointerUp", button: 0 });
      actions.push({ type: "pointer", id: `finger${i}`, parameters: { pointerType: "touch" }, actions: a });
    }
    await this.wd.cmd("POST", "/actions", { actions });
    await this.wd.cmd("DELETE", "/actions").catch(() => undefined);
  }

  /** Taps and long presses are real simulator touches. A finger that moves is dispatched in the page: Mobile Safari's
   *  WebDriver reports the lift (pointerup / touchend) at 0,0 whatever the action says, so a drawn rectangle ran to the
   *  top-left corner and a flick ended under the status bar. `--input native` sends those as real touches anyway. */
  async gesture(frames, opts = {}) {
    if (!this.device.touch) return this.mouseGesture(frames, opts);
    const moves = frames.some((f) => f.some(([x, y], i) => x !== frames[0][i][0] || y !== frames[0][i][1]));
    if (this.input === "synthetic" || (moves && this.input !== "native-all")) {
      await this.evaluate(installSynth);
      return this.evaluate((f, o) => window.__oeSynth.gesture(f, o), frames, opts);
    }
    return this.touchActions(frames, opts);
  }

  async mouseGesture(frames, { holdMs = 0, stepMs = 16, button = 0, clickCount = 1 } = {}) {
    const a = [{ type: "pointerMove", duration: 0, x: Math.round(frames[0][0][0]), y: Math.round(frames[0][0][1]) }];
    for (let c = 0; c < clickCount; c++) {
      a.push({ type: "pointerDown", button });
      if (holdMs) a.push({ type: "pause", duration: holdMs });
      for (let k = 1; k < frames.length; k++) a.push({ type: "pointerMove", duration: stepMs, x: Math.round(frames[k][0][0]), y: Math.round(frames[k][0][1]) });
      a.push({ type: "pointerUp", button });
    }
    await this.wd.cmd("POST", "/actions", { actions: [{ type: "pointer", id: "mouse", parameters: { pointerType: "mouse" }, actions: a }] });
    await this.wd.cmd("DELETE", "/actions").catch(() => undefined);
  }

  async tap(x, y) {
    await this.gesture([[[x, y]]]);
  }

  async press(x, y) {
    await this.tap(x, y);
  }

  async click(x, y, { button = "left", clickCount = 1 } = {}) {
    if (this.device.touch) {
      if (button === "right") return this.longPress(x, y, 750);
      for (let i = 0; i < clickCount; i++) await this.tap(x, y);
      return;
    }
    await this.mouseGesture([[[x, y]]], { button: button === "right" ? 2 : 0, clickCount });
  }

  async longPress(x, y, ms = 700) {
    await this.gesture([[[x, y]], [[x, y]]], { holdMs: ms });
  }

  async drag(x1, y1, x2, y2, { steps = 12, hold = 0 } = {}) {
    const frames = [];
    for (let i = 0; i <= steps; i++) frames.push([[x1 + ((x2 - x1) * i) / steps, y1 + ((y2 - y1) * i) / steps]]);
    await this.gesture(frames, { holdMs: hold });
  }

  async pinch(cx, cy, from, to, { steps = 10 } = {}) {
    const frames = [];
    for (let i = 0; i <= steps; i++) {
      const d = from + ((to - from) * i) / steps;
      frames.push([
        [cx - d / 2, cy],
        [cx + d / 2, cy],
      ]);
    }
    await this.gesture(frames);
  }

  async wheel(x, y, deltaY) {
    if (this.device.touch) return;
    await this.wd.cmd("POST", "/actions", {
      actions: [{ type: "wheel", id: "wheel", actions: [{ type: "scroll", x: Math.round(x), y: Math.round(y), deltaX: 0, deltaY, duration: 0 }] }],
    });
  }

  async mouse(type, x, y) {
    if (this.device.touch || type !== "mouseMoved") return;
    await this.wd.cmd("POST", "/actions", { actions: [{ type: "pointer", id: "mouse", parameters: { pointerType: "mouse" }, actions: [{ type: "pointerMove", duration: 0, x: Math.round(x), y: Math.round(y) }] }] });
  }

  /** Mobile Safari's WebDriver turns named keys into other characters (Escape arrives as "?"): on iOS named keys are
   *  dispatched in the page (untrusted, which the app's handlers accept); desktop Safari gets real key actions. */
  async key(key, { modifiers = 0 } = {}) {
    if (this.device.touch) {
      await this.evaluate((k, m) => window.__oeSynth?.key(k, m) ?? (() => {
        const t = document.activeElement && document.activeElement !== document.body ? document.activeElement : document.body;
        const o = { key: k, bubbles: true, cancelable: true, composed: true, ...m };
        t.dispatchEvent(new KeyboardEvent("keydown", o));
        t.dispatchEvent(new KeyboardEvent("keyup", o));
      })(), key, { altKey: !!(modifiers & 1), ctrlKey: !!(modifiers & 2), metaKey: !!(modifiers & 4), shiftKey: !!(modifiers & 8) });
      return;
    }
    const v = KEYS[key] ?? key;
    const mods = [modifiers & 2 && "", modifiers & 1 && "", modifiers & 8 && "", modifiers & 4 && ""].filter(Boolean);
    const a = [...mods.map((m) => ({ type: "keyDown", value: m })), { type: "keyDown", value: v }, { type: "keyUp", value: v }, ...mods.reverse().map((m) => ({ type: "keyUp", value: m }))];
    await this.wd.cmd("POST", "/actions", { actions: [{ type: "key", id: "kb", actions: a }] });
  }

  /** Type into the focused field: WebDriver's element send keys (the simulator's keyboard path); when that does not
   *  arrive, the value is set the way React sees an edit. */
  async type(text) {
    let ok = false;
    try {
      const el = await this.wd.cmd("GET", "/element/active");
      const id = el && Object.values(el)[0];
      if (id) {
        const before = await this.evaluate(() => document.activeElement?.value ?? document.activeElement?.textContent ?? "");
        await this.wd.cmd("POST", `/element/${id}/value`, { text });
        const after = await this.evaluate(() => document.activeElement?.value ?? document.activeElement?.textContent ?? "");
        ok = after !== before;
      }
    } catch {
      ok = false;
    }
    if (ok) return "native";
    await this.evaluate((t) => {
      const el = document.activeElement;
      if (!el || !("value" in el)) return false;
      const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, "value").set.call(el, (el.value ?? "") + t);
      el.dispatchEvent(new InputEvent("input", { bubbles: true, data: t, inputType: "insertText" }));
      return true;
    }, text);
    return "synthetic";
  }

  async setViewportHeight() {
    /* the real keyboard */
  }

  async send() {
    /* no CDP */
  }

  async screenshot(path) {
    const data = await this.wd.cmd("GET", "/screenshot");
    writeFileSync(path, Buffer.from(data, "base64"));
    return path;
  }

  async settle(ms = 120) {
    await this.evaluate("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))").catch(() => {});
    if (ms) await sleep(ms);
  }
}

/** Does a WebDriver touch reach the page? (It does not while the Mac's screen is locked.) */
async function probeInput(page) {
  const p = await page.evaluate(armProbe);
  try {
    await page.touchActions([[[p.x, p.y]]]);
  } catch (e) {
    await page.evaluate(() => document.getElementById("oe-probe-pad")?.remove());
    return { ok: false, detail: String(e.message ?? e) };
  }
  await sleep(600);
  const got = await page.evaluate(() => {
    document.getElementById("oe-probe-pad")?.remove();
    return window.__oeProbe ?? [];
  });
  return { ok: got.includes("pointerdown:touch"), detail: got.join(", ") || "no events" };
}

// --- the steps ---------------------------------------------------------------------------------------------------

const json = (body) => ({ method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

async function canvasTab(page) {
  await phoneTab(page, "Canvas");
  if (!(await sheetOpen(page))) await phoneTab(page, "Canvas");
}

async function closeSheet(page) {
  if (await sheetOpen(page)) await pressFind(page, ".sheet-close");
  await sleep(300);
}

async function workspace(page, kind, name) {
  if (kind === "phone") {
    if (name === "Laser") {
      await phoneTab(page, "Job");
      if (!(await sheetOpen(page))) await phoneTab(page, "Job");
    } else {
      await canvasTab(page);
      await closeSheet(page);
    }
    return;
  }
  await pressFind(page, ".workspaces button, nav[role=tablist] button", name);
  await sleep(600);
}

async function openGuide(page, kind) {
  await escape(page);
  if (kind === "phone") {
    await moreSheet(page);
    await openSection(page, "Help");
    await pressFind(page, ".phone-more button, .phone-more [role=menuitem]", "User guide");
  } else {
    // Help > User guide: the tablet's More menu (the top bar's Help on a wide window).
    const help = await find(page, ".topbar button, .ribbon button", "Help");
    if (help?.hit) await page.press(help.x, help.y);
    else await pressFind(page, "button", "More");
    await sleep(400);
    await pressFind(page, "[role=menuitem], .menu-item", "User guide");
  }
  if (!(await page.waitFor(".guide-md h1", { timeout: 8000 }))) throw new Error("the guide did not open");
}

// --- OpenEtch Design (--design): the static site, its in-page backend (window.__oeDesign, designmode/backend.ts) ---

const repoDir = resolve(here, "..", "..");
const b64File = (...p) => readFileSync(join(repoDir, ...p)).toString("base64");

async function dreq(page, method, path, body) {
  return page.evaluate((m, p, b) => window.__oeDesign.request(m, p, b === null ? undefined : b), method, path, body ?? null);
}

async function dbytes(page, method, path, base64) {
  return page.evaluate((m, p, b) => window.__oeDesign.bytes(m, p, b === null ? undefined : b), method, path, base64 ?? null);
}

async function dexec(page, id, params = {}) {
  const r = await dreq(page, "POST", "/api/execute", { id, params, origin: "ui" });
  if (r.status !== 200) throw new Error(`${id}: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
  return r.body;
}

async function crashes(page) {
  const c = await page.evaluate(() => globalThis.__oeDesignCrashes ?? []).catch(() => []);
  // Where the page is, and whether its IndexedDB opens at all (a blocked or hung open stalls the editor's start).
  const where = await page
    .evaluate(
      () =>
        new Promise((res) => {
          const info = { href: location.href, boot: globalThis.__oeDesignBoot ?? null };
          const r = indexedDB.open("openetch-design");
          r.onsuccess = () => (r.result.close(), res({ ...info, idb: "ok" }));
          r.onerror = () => res({ ...info, idb: `error ${r.error}` });
          r.onblocked = () => res({ ...info, idb: "blocked" });
          setTimeout(() => res({ ...info, idb: "no answer in 5 s" }), 5000);
        }),
    )
    .catch((e) => ({ evalError: String(e) }));
  return `${c?.length ? `(worker: ${c.join(" | ")}) ` : ""}${JSON.stringify(where)}`;
}

async function designReady(page) {
  if (!(await page.waitFor("!!window.__oeDesign", { expr: true, timeout: 40000 }))) throw new Error("no design backend");
  for (let i = 0; i < 100; i++) {
    const r = await dreq(page, "GET", "/api/project").catch(() => null);
    if (r?.status === 200) return;
    await sleep(250);
  }
  throw new Error(`the design backend did not answer ${await crashes(page)}`);
}

const MACHINE_WORDS = "^(start|start here|frame|framing|stop|jog|home|connect|find my laser|camera|run|approve)$";

function designStepsFor(kind) {
  const S = [];
  const add = (name, fn, kinds = ["phone", "tablet", "desktop"]) => {
    if (kinds.includes(kind)) S.push({ name, fn });
  };
  add("examples", async ({ page }) => {
    const list = (await dexec(page, "example.list")).result;
    const counts = {};
    for (const ex of list) {
      await dexec(page, "doc.new", { example: ex.id });
      counts[ex.id] = (await dreq(page, "GET", "/api/document")).body.objects.length;
    }
    await sleep(1500);
    await escape(page);
    return counts;
  });
  add("draw-rectangle", async ({ page }) => {
    await dexec(page, "doc.new");
    await sleep(1200);
    const b = await find(page, "button", "Draw a rectangle");
    if (!b?.hit) throw new Error("no Draw a rectangle on the start card");
    await page.press(b.x, b.y);
    await sleep(600);
    const r = await page.evaluate(() => {
      const c = document.querySelector("canvas").getBoundingClientRect();
      return { x0: c.left + c.width * 0.4, y0: c.top + c.height * 0.55, x1: c.left + c.width * 0.6, y1: c.top + c.height * 0.7 };
    });
    await page.drag(r.x0, r.y0, r.x1, r.y1);
    await sleep(1000);
    const n = (await dreq(page, "GET", "/api/document")).body.objects.length;
    if (!n) throw new Error("drawing made no object");
    return { objects: n };
  });
  add("text-uploaded-font", async ({ page }) => {
    await escape(page);
    const up = await dbytes(page, "POST", "/api/fonts/user?name=openetch-box.ttf", b64File("crates", "text", "tests", "fixtures", "ttf", "openetch-box.ttf"));
    if (up.status !== 200) throw new Error(`font upload ${up.status}`);
    await dexec(page, "doc.new");
    await sleep(1000);
    await dexec(page, "shape.text", { text: "OE", font: { family: "OpenEtch Box" }, height_mm: 10, center: [0, 30] });
    await sleep(1000);
    return { font: "OpenEtch Box" };
  });
  add("font-picker", async ({ page }) => {
    await dexec(page, "doc.new");
    await sleep(1000);
    const b = await find(page, "button", "Add text");
    if (!b?.hit) throw new Error("no Add text on the start card");
    await page.press(b.x, b.y);
    await sleep(700);
    const f = await find(page, ".tx-font-btn");
    if (!f) throw new Error("no font picker");
    await page.press(f.x, f.y);
    await sleep(900);
    const ok = await page.evaluate(() => ({
      add: [...document.querySelectorAll(".tx-font-pop button")].some((e) => /add font/i.test(e.textContent)),
      listed: document.querySelector(".tx-font-list")?.textContent.includes("OpenEtch Box") ?? false,
    }));
    if (!ok.add || !ok.listed) throw new Error(`font picker: ${JSON.stringify(ok)}`);
    return ok;
  });
  add("lightburn-import", async ({ page }) => {
    await escape(page);
    await escape(page);
    const up = await dbytes(page, "POST", "/api/document/upload?format=lbrn2&mode=replace&name=shapes.lbrn2", b64File("crates", "lbrn", "tests", "fixtures", "shapes.lbrn2"));
    if (up.status !== 200) throw new Error(`lbrn2 import ${up.status}`);
    const dl = await dbytes(page, "GET", "/api/document/download?format=lbrn2");
    if (dl.status !== 200) throw new Error(`lbrn2 export ${dl.status}`);
    await sleep(1500);
    return { objects: (await dreq(page, "GET", "/api/document")).body.objects.length, exported: dl.base64.length };
  });
  add("save-in-browser", async ({ page }) => {
    const s = await dreq(page, "POST", "/api/projects/save", { name: "Safari harness", origin: "ui" });
    if (s.status !== 200) throw new Error(`save ${s.status}`);
    await sleep(800);
    return s.body;
  });
  add("reload-and-reopen", async ({ page, ctx }) => {
    await page.goto(ctx.url, { waitFor: ".topbar" });
    await designReady(page);
    const list = (await dreq(page, "GET", "/api/projects")).body;
    if (!list.some((p) => p.name === "Safari harness")) throw new Error(`after a reload: ${JSON.stringify(list)}`);
    const o = await dreq(page, "POST", "/api/projects/open", { name: "Safari harness", origin: "ui" });
    if (o.status !== 200) throw new Error(`open ${o.status}`);
    await sleep(1500);
    return (await dreq(page, "GET", "/api/project")).body;
  });
  add("recovery-after-reload", async ({ page, ctx }) => {
    await dexec(page, "shape.rect", { width_mm: 10, height_mm: 10, center: [30, 30], name: "Unsaved square" });
    await sleep(4500);
    await page.goto(ctx.url, { waitFor: ".topbar" });
    await designReady(page);
    await sleep(1200);
    const doc = (await dreq(page, "GET", "/api/document")).body;
    if (!doc.objects.some((o) => o.name === "Unsaved square")) throw new Error(`the unsaved edit did not come back ${await crashes(page)}`);
    return (await dreq(page, "GET", "/api/project")).body;
  });
  add("preview-estimate", async ({ page }) => {
    await escape(page);
    if (kind === "phone") await phoneTab(page, "Preview");
    else await pressFind(page, ".workspaces button, nav[role=tablist] button", "Preview");
    await sleep(3000);
    const m = /Estimate for [^\n]*/.exec(await page.evaluate(() => document.body.innerText));
    if (!m) throw new Error("no time estimate");
    return { estimate: m[0] };
  });
  add("back-to-design", async ({ page }) => {
    await escape(page);
    if (kind === "phone") await phoneTab(page, "Canvas");
    else await pressFind(page, ".workspaces button, nav[role=tablist] button", "Design");
    await sleep(800);
  });
  add("no-machine-ui", async ({ page }) => {
    const hits = await page.evaluate((src) => {
      const re = new RegExp(src, "i");
      return [...document.querySelectorAll("button, a, [role=button], [role=menuitem]")]
        .filter((e) => {
          const r = e.getBoundingClientRect();
          return r.width > 0 && r.height > 0 && re.test((e.textContent ?? "").trim());
        })
        .map((e) => e.textContent.trim());
    }, MACHINE_WORDS);
    if (hits.length) throw new Error(`machine controls: ${hits.join(", ")}`);
  });
  add("guide-open", async ({ page }) => openGuide(page, kind));
  add("guide-closed", async ({ page }) => {
    await escape(page);
    await sleep(500);
  });
  add("site-files-only", async ({ page, ctx }) => {
    const foreign = await page.evaluate(
      (prefix) => performance.getEntriesByType("resource").map((e) => e.name).filter((n) => !n.startsWith(prefix) && !n.startsWith("blob:") && !n.startsWith("data:")),
      ctx.url,
    );
    if (foreign.length) throw new Error(`loaded from outside the site: ${foreign.join(", ")}`);
    return { outside: 0 };
  });
  return S;
}

/** The steps for a layout ("phone" | "tablet" | "desktop"). Each returns optional notes. */
function stepsFor(kind) {
  const S = [];
  const add = (name, fn, kinds = ["phone", "tablet", "desktop"]) => {
    if (kinds.includes(kind)) S.push({ name, fn });
  };

  add("example-loaded", async ({ page, ctx }) => {
    await execute(ctx.url, "doc.new", { example: "two-tone-coin" });
    await sleep(2500);
    // The example's notes window.
    await escape(page);
    const f = await find(page, "button", "Fit field");
    if (f?.hit) await page.press(f.x, f.y);
    await sleep(600);
  });
  add("design-workspace", async ({ page }) => workspace(page, kind, "Design"), ["tablet", "desktop"]);
  add("select", async ({ page, ctx }) => {
    await escape(page);
    if (kind === "phone") await closeSheet(page);
    ctx.sel = await selectSomething(page);
    if (!ctx.sel) throw new Error("nothing got selected");
    return { selected: ctx.sel.at };
  });
  add("tool-rectangle", async ({ page }) => {
    await escape(page);
    if (kind === "phone") await canvasTab(page);
    await pressFind(page, "button", "Rectangle");
    await sleep(300);
    if (kind === "phone") await closeSheet(page);
    return { touchMods: await page.evaluate(() => !!document.querySelector(".touch-mods")) };
  });
  add("touch-modifier-bar", async ({ page }) => {
    // The touch modifier bar while drawing: Shift held from the bar, a rectangle dragged out.
    const m = await find(page, ".touch-mods button", "Shift");
    if (m?.hit) await page.press(m.x, m.y);
    await sleep(200);
    const a = await canvasPoint(page, 40, 50);
    const b = await canvasPoint(page, 60, 35);
    await page.drag(a.x, a.y, b.x, b.y, { steps: 8 });
    await sleep(500);
    return { mods: await page.evaluate(() => [...document.querySelectorAll(".touch-mods button")].map((b) => `${b.textContent.trim()}${b.getAttribute("aria-pressed") === "true" ? "*" : ""}`)) };
  });
  add("tool-line", async ({ page }) => {
    await escape(page);
    if (kind === "phone") await canvasTab(page);
    await pressFind(page, "button", "Line");
    await sleep(300);
    if (kind === "phone") await closeSheet(page);
    const a = await canvasPoint(page, -60, 60);
    const b = await canvasPoint(page, -20, 40);
    await page.press(a.x, a.y);
    await sleep(150);
    await page.press(b.x, b.y);
  });
  add("context-menu", async ({ page, ctx }) => {
    await escape(page);
    await page.key("v");
    await escape(page);
    const [x, y] = ctx.sel?.at ?? [0, 0];
    const p = await canvasPoint(page, x, y);
    await page.longPress(p.x, p.y, 750);
    await sleep(500);
    return { menu: await page.evaluate(() => !!document.querySelector(".ctx-menu, [role=menu]")) };
  });
  add("text-tool", async ({ page }) => {
    await escape(page);
    if (kind === "phone") await canvasTab(page);
    const ok = await pressFind(page, "button", "Text").catch(() => null);
    if (!ok) await pressFind(page, "button", "Add text");
    await sleep(300);
    if (kind === "phone") await closeSheet(page);
    const p = await canvasCenter(page);
    await page.press(p.x, p.y);
    await sleep(900);
    return { focused: await page.evaluate(() => `${document.activeElement?.tagName} ${document.activeElement?.className ?? ""}`) };
  });
  add("text-typed", async ({ page }) => {
    const how = await page.type("Happy Halloween");
    await sleep(600);
    const vv = await page.evaluate(() => {
      const a = document.activeElement?.getBoundingClientRect();
      const v = visualViewport;
      return { field: a ? [Math.round(a.top), Math.round(a.bottom)] : null, visual: [Math.round(v.offsetTop), Math.round(v.height)], inner: innerHeight };
    });
    return { typed: how, ...vv };
  });
  add("text-done", async ({ page }) => {
    await page.key("Enter");
    await sleep(300);
    await page.evaluate(() => document.activeElement?.blur?.());
    await escape(page);
    await sleep(400);
  });
  add("number-field-focus", async ({ page }) => {
    // A numeric field focused: iOS zooms the page in on a field under 16 px (scale reported by the check).
    await escape(page);
    if (kind === "phone") await closeSheet(page);
    const sel = await selectSomething(page);
    if (!sel) throw new Error("nothing to select");
    if (kind === "phone") {
      await canvasTab(page);
      await sheetTo(page, "half");
    } else if (kind === "tablet") {
      const pill = await find(page, ".properties-pill");
      if (pill?.hit) await page.press(pill.x, pill.y);
      await sleep(400);
    }
    const inp = await find(page, ".bottom-sheet input[inputmode], .dock-overlay input, .field-input input, input[inputmode=decimal], input[type=text]");
    if (!inp) throw new Error("no field");
    await page.press(inp.x, inp.y);
    await sleep(900);
    return { focused: await page.evaluate(() => `${document.activeElement?.tagName} ${getComputedStyle(document.activeElement).fontSize}`), scale: await page.evaluate(() => visualViewport.scale) };
  });
  add("number-field-blur", async ({ page }) => {
    await page.evaluate(() => document.activeElement?.blur?.());
    await escape(page);
    await sleep(400);
  });
  add("rail-more", async ({ page }) => {
    await escape(page);
    const more = await find(page, ".ribbon-more-btn");
    if (!more) return { more: "no More button (every group fits the rail)" };
    await pressFind(page, ".ribbon-more-btn");
    await sleep(400);
    return { groups: await page.evaluate(() => [...document.querySelectorAll(".ribbon-more-group")].map((g) => g.textContent.trim().slice(0, 30))) };
  }, ["tablet"]);
  add("rail-more-closed", async ({ page }) => escape(page), ["tablet"]);
  for (const tab of ["Layers", "More"])
    for (const snap of ["half", "full"])
      add(`sheet-${tab.toLowerCase()}-${snap}`, async ({ page }) => {
        await phoneTab(page, tab);
        if (!(await sheetOpen(page))) await phoneTab(page, tab);
        if (!(await sheetTo(page, snap))) throw new Error(`sheet did not reach ${snap}`);
      }, ["phone"]);
  add("sheet-scrolled", async ({ page }) => {
    // A flick in the full More sheet (momentum scrolling in its list).
    const s = await find(page, ".bottom-sheet");
    if (!s) throw new Error("no sheet");
    await page.drag(s.x, s.y + s.h * 0.3, s.x, s.y - s.h * 0.3, { steps: 6 });
    await sleep(800);
    return { scrollTop: await page.evaluate(() => Math.max(...[...document.querySelectorAll(".bottom-sheet *")].map((e) => e.scrollTop))) };
  }, ["phone"]);
  add("dialog-preferences", async ({ page }) => {
    await escape(page);
    if (kind === "phone") {
      await moreSheet(page);
      await openSection(page, "Settings");
      await pressFind(page, ".phone-more button, .phone-more [role=button], .phone-more a", "Preferences");
    } else {
      const gear = await find(page, ".topbar button", "Preferences");
      if (gear?.hit) await page.press(gear.x, gear.y);
      else {
        await pressFind(page, "button", "More");
        await sleep(350);
        await pressFind(page, "[role=menuitem], .menu-item", "Preferences");
      }
    }
    if (!(await page.waitFor(".oe-dialog, [role=dialog]", { timeout: 5000 }))) throw new Error("no Preferences dialog");
    await sleep(400);
  });
  add("dialog-help", async ({ page }) => {
    // The dialog's "?": the guide on top of the dialog.
    await pressFind(page, ".oe-dialog .help-link, [role=dialog] .help-link");
    if (!(await page.waitFor(".guide-md h1", { timeout: 8000 }))) throw new Error("the guide did not open from the dialog's ?");
    await sleep(600);
  });
  add("dialog-help-closed", async ({ page }) => {
    await pressFind(page, ".guide-bar button", "Close the guide");
    await sleep(400);
    const still = await page.evaluate(() => !!document.querySelector(".oe-dialog, [role=dialog][aria-modal=true]"));
    await escape(page);
    return { dialogStayed: still };
  });
  add("dialog-devices", async ({ page }) => {
    await escape(page);
    if (kind === "phone") {
      await moreSheet(page);
      await openSection(page, "Desktop features");
      await pressFind(page, ".phone-more button, .phone-more [role=button], .phone-more a", "Devices");
    } else await menuPath(page, ["File", "Laser", "Devices"]).catch(async () => {
      await escape(page);
      await pressFind(page, "button", "More");
      await sleep(350);
      await pressFind(page, "[role=menuitem], .menu-item", "Devices");
    });
    await sleep(600);
  });
  add("laser-preview", async ({ page }) => {
    await escape(page);
    await workspace(page, kind, "Laser");
    if (!(await page.waitFor(".pv-timeline", { timeout: 10000 }))) throw new Error("no job preview");
    await sleep(500);
  });
  add("framing-start", async ({ page }) => {
    await pressFind(page, "button", "Frame");
    await sleep(1500);
    return { framing: (await api(page.ctxUrl, "/api/machine"))?.framing ?? null };
  });
  add("framing-stop", async ({ page }) => {
    let b = await find(page, "button", "Stop frame");
    if ((!b || !b.hit) && kind === "phone") {
      await workspace(page, kind, "Laser");
      b = await find(page, "button", "Stop frame");
    }
    if (!b) throw new Error("no Stop frame button");
    if (!b.hit) throw new Error(`Stop frame is not hit-testable (under ${b.cover})`);
    await page.press(b.x, b.y);
    await sleep(900);
  });
  // Start with click approval, a layer that pauses, Continue: in the production cockpit.
  add("cockpit-idle", async ({ page, ctx }) => {
    await escape(page);
    const doc = await api(ctx.url, "/api/document");
    const pauseLayer = (await execute(ctx.url, "layer.add", { name: "Frost" })).result.id;
    await execute(ctx.url, "shape.rect", { width_mm: 30, height_mm: 4, center: [0, -45], layer: pauseLayer });
    await execute(ctx.url, "layer.update", { id: pauseLayer, settings: { options: { operator_pause: true } } });
    await api(ctx.url, "/api/job/count/reset", json({}));
    await page.evaluate(() => {
      location.hash = "#/cockpit";
    });
    if (!(await page.waitFor(".cockpit-bar", { timeout: 8000 }))) throw new Error("the cockpit did not open");
    await sleep(900);
    return { layers: doc?.layers?.length };
  });
  add("cockpit-started", async ({ page, ctx }) => {
    await pressFind(page, ".cockpit .btn-start");
    for (let i = 0; i < 150 && !(await page.evaluate(() => !!document.querySelector(".cp-continue"))); i++) await sleep(100);
    const m = await api(ctx.url, "/api/machine");
    if (!(await page.evaluate(() => !!document.querySelector(".cp-continue")))) throw new Error(`no Continue (machine: ${JSON.stringify(m?.job ?? m?.state ?? null).slice(0, 200)})`);
  });
  add("cockpit-continued", async ({ page, ctx }) => {
    await pressFind(page, ".cp-continue");
    for (let i = 0; i < 150; i++) {
      const p = await api(ctx.url, "/api/production");
      if (!p.job?.active) break;
      await sleep(100);
    }
    await sleep(900);
  });
  add("cockpit-help", async ({ page }) => {
    await pressFind(page, ".cockpit .help-link");
    if (!(await page.waitFor(".guide-md h1", { timeout: 8000 }))) throw new Error("the guide did not open from the cockpit's ?");
    await sleep(500);
  });
  add("cockpit-exit", async ({ page }) => {
    await pressFind(page, ".guide-bar button", "Close the guide");
    await sleep(400);
    await pressFind(page, ".cockpit-exit");
    await sleep(800);
  });
  add("guide-open", async ({ page }) => openGuide(page, kind));
  add("guide-images", async ({ page }) => {
    // A page with pictures: every picture in view loaded at its size.
    await page.evaluate(() => {
      location.hash = "#/guide/the-workspace";
    });
    await page.waitFor(".guide-md img", { timeout: 8000 });
    await page.evaluate(() => document.querySelector(".guide-md img")?.scrollIntoView({ block: "center" }));
    await sleep(1500);
    const imgs = await page.evaluate(() => [...document.querySelectorAll(".guide-md img")].map((i) => ({ src: i.getAttribute("src"), ok: i.complete && i.naturalWidth > 0, w: Math.round(i.getBoundingClientRect().width) })));
    const bad = imgs.filter((i) => !i.ok);
    if (!imgs.length) throw new Error("no pictures on the page");
    return { pictures: imgs.length, notLoaded: bad.map((b) => b.src) };
  });
  add("guide-link", async ({ page }) => {
    // A link to another page of the guide.
    const href = await page.evaluate(() => {
      const a = [...document.querySelectorAll(".guide-md a[href^='#/guide/']")].find((x) => !x.getAttribute("href").startsWith(location.hash.split("#").slice(0, 2).join("#")));
      if (!a) return null;
      a.scrollIntoView({ block: "center" });
      return a.getAttribute("href");
    });
    if (!href) throw new Error("no cross-page link");
    await sleep(500);
    const a = await find(page, `.guide-md a[href="${href}"]`);
    if (!a?.hit) throw new Error(`link ${href} not hit-testable (${a?.cover})`);
    await page.press(a.x, a.y);
    await sleep(1200);
    return { href, now: await page.evaluate(() => location.hash) };
  });
  add("guide-search", async ({ page }) => {
    const f = await find(page, ".guide-search input");
    if (!f) throw new Error("no guide search field");
    await page.press(f.x, f.y);
    await sleep(300);
    await page.type("energy density");
    await sleep(600);
    return { hits: await page.evaluate(() => document.querySelectorAll(".guide-hit").length), scale: await page.evaluate(() => visualViewport.scale) };
  });
  add("guide-result", async ({ page }) => {
    await page.evaluate(() => document.activeElement?.blur?.());
    await pressFind(page, ".guide-hit", "Energy density");
    await sleep(900);
  });
  add("guide-closed", async ({ page }) => {
    await pressFind(page, ".guide-bar button", "Close the guide");
    await sleep(500);
  });
  add("bottom-bar", async ({ page }) => {
    // Back on the canvas: the bottom rows (timeline / status, the phone's tabs) against the home indicator.
    await escape(page);
    if (kind === "phone") {
      await canvasTab(page);
      await closeSheet(page);
    } else await workspace(page, kind, "Design");
    return await page.evaluate(() => {
      const pick = (s) => {
        const e = document.querySelector(s);
        if (!e) return null;
        const r = e.getBoundingClientRect();
        return [Math.round(r.top), Math.round(r.bottom)];
      };
      return { phoneTabs: pick(".phone-tabs"), statusbar: pick(".statusbar"), timeline: pick(".timeline"), inner: innerHeight };
    });
  });
  return S;
}

// --- one device --------------------------------------------------------------------------------------------------

async function runDevice(a, key, dir) {
  const sims = simulators(a.mac);
  const desktop = key === "desktop";
  const sim = desktop ? null : pickSim(sims, key);
  const wd = new WebDriver(`http://127.0.0.1:${a.driverPort}`);
  const results = [];
  const notes = {};
  const caps = desktop
    ? { browserName: "safari" }
    : { browserName: "safari", platformName: "iOS", "safari:useSimulator": true, "safari:deviceUDID": sim.udid };
  const t0 = Date.now();
  // A cold simulator takes longer to boot than safaridriver waits for Safari: boot it first, and wait until it is up.
  if (sim && sim.state !== "Booted") {
    mac(a.mac, `xcrun simctl boot ${sim.udid}`, { allowFail: true });
    mac(a.mac, `xcrun simctl bootstatus ${sim.udid} -b`, { allowFail: true });
  }
  for (let attempt = 1; ; attempt++) {
    try {
      notes.caps = await wd.start(caps);
      break;
    } catch (e) {
      if (attempt < 3 && /timed out|paired/.test(String(e.message))) {
        // Safari in the simulator still starting, or holding a session that ended badly: start it afresh.
        if (sim) mac(a.mac, `xcrun simctl terminate ${sim.udid} com.apple.mobilesafari`, { allowFail: true });
        await sleep(3000);
        continue;
      }
      return { device: key, name: sim ? `${sim.name} iOS ${sim.runtime}` : "macOS Safari", udid: sim?.udid, error: String(e.message ?? e), results };
    }
  }
  notes.sessionSeconds = Math.round((Date.now() - t0) / 1000);
  const page = new SafariPage(wd, { touch: !desktop, kind: "auto" });
  const url = a.design ? a.siteUrl : `http://127.0.0.1:${a.port}`;
  page.ctxUrl = url;
  mkdirSync(dir, { recursive: true });
  try {
    if (a.design) {
      // A first visit: nothing stored (preferences, the tab's session, the browser's projects). Cleared from a plain
      // file of the same site, before the editor has loaded: deleting the database while the editor's worker holds it
      // open, then navigating, leaves IndexedDB hung for the whole site in iOS Safari, so the next start never ends.
      await page.goto(`${url}icon.svg`, { timeout: 60000 });
      const cleared = await page.evaluate(async () => {
        localStorage.clear();
        sessionStorage.clear();
        return await new Promise((r) => {
          const q = indexedDB.deleteDatabase("openetch-design");
          q.onsuccess = () => r("deleted");
          q.onerror = () => r("error");
          q.onblocked = () => r("blocked");
        });
      });
      if (cleared !== "deleted") throw new Error(`could not clear the site's storage: ${cleared}`);
      const t1 = Date.now();
      await page.goto(`${url}?first=${Date.now()}`, { waitFor: ".topbar", timeout: 60000 });
      await designReady(page);
      notes.firstLoadMs = Date.now() - t1;
    } else {
      await api(url, "/api/machine/frame/stop", json({ origin: "ui" }));
      await execute(url, "doc.new");
      await page.goto(`${url}/`, { waitFor: ".btn-stop" });
      // A first visit: no stored preferences, so the "Use as an app" offer shows.
      await page.evaluate(() => {
        localStorage.clear();
        sessionStorage.clear();
      });
      await page.goto(`${url}/?first=${Date.now()}`, { waitFor: ".btn-stop" });
    }
    // The "Use as an app" offer comes 1.5 s after a first touch visit (AppModeDialog.tsx).
    notes.appOffer = await page.waitFor(`[...document.querySelectorAll("[role=dialog]")].some((e) => /as an app/i.test(e.textContent))`, { expr: true, timeout: 6000 });
    const layout = await page.evaluate(() => document.documentElement.dataset.layout);
    page.device.kind = layout === "phone" ? "phone" : layout === "tablet" ? "tablet" : "desktop";
    notes.layout = layout;
    const check = async (name, extra = {}) => {
      const shot = join(dir, `${String(results.length).padStart(2, "0")}-${name}.png`);
      await page.settle(250);
      await page.screenshot(shot).catch((e) => (extra.error = `${extra.error ?? ""} screenshot: ${e.message}`));
      const sc = await page.evaluate(safariChecks).catch((e) => ({ error: String(e), issues: [] }));
      const c = await page.evaluate(checkLayout, { touch: page.device.touch, safe: sc.env ?? {} }).catch((e) => ({ error: String(e), problems: [] }));
      await page.collectErrors();
      results.push({ step: name, shot, ...extra, safari: { ...sc, issues: undefined }, counts: c.counts, problems: [...(c.problems ?? []), ...(sc.issues ?? [])], error: extra.error ?? c.error });
    };
    await check("first-visit", { offer: await page.evaluate(() => [...document.querySelectorAll("[role=dialog]")].map((d) => d.textContent.trim().slice(0, 160))) });
    const close = await find(page, ".modal-foot button, [role=dialog] button", "Close");
    if (close?.hit) {
      await page.press(close.x, close.y);
      await sleep(500);
    }
    // After the offer: the probe's tap would land on the offer's backdrop and close it before its screenshot.
    if (!desktop) {
      if (a.input === "auto") {
        notes.nativeTouch = await probeInput(page);
        page.input = notes.nativeTouch.ok ? "native" : "synthetic";
      } else page.input = a.input;
      if (page.input === "synthetic") await page.evaluate(installSynth);
    }
    notes.input = page.input;
    if (notes.nativeTouch && !notes.nativeTouch.ok && close?.hit && (await page.evaluate(() => !!document.querySelector("[role=dialog]")))) {
      // The Close tap was a native one that did not arrive: close the offer again.
      const c2 = await find(page, ".modal-foot button, [role=dialog] button", "Close");
      if (c2?.hit) await page.press(c2.x, c2.y);
      await sleep(500);
    }
    const ctx = { url };
    const steps = (a.design ? designStepsFor(page.device.kind) : stepsFor(page.device.kind)).filter(
      (s) => !a.steps || a.steps.some((f) => s.name.includes(f)) || s.name === "example-loaded",
    );
    for (const s of steps) {
      let error;
      let out;
      try {
        out = await s.fn({ page, ctx });
      } catch (e) {
        error = String(e.message ?? e);
      }
      // A page reload (a hash route) drops the in-page helpers.
      await page.afterLoad().catch(() => undefined);
      await sleep(250);
      await check(s.name, { error, notes: out ?? undefined });
      console.log(`  ${key} ${s.name}${error ? ` ERROR ${error}` : ""}`);
    }
  } finally {
    if (!a.design) await api(url, "/api/machine/stop", json({})).catch(() => undefined);
    await wd.end();
  }
  return { device: key, name: sim ? `${sim.name} iOS ${sim.runtime}` : "macOS Safari", udid: sim?.udid, notes, errors: page.errors.slice(0, 30), results };
}

function markdown(runs) {
  const L = ["# Real Safari report", ""];
  for (const r of runs) {
    L.push(`## ${r.name} (${r.device})`, "");
    if (r.error) {
      L.push(`Session failed: ${r.error}`, "");
      continue;
    }
    if (r.notes.firstLoadMs) L.push(`First visit ready in ${r.notes.firstLoadMs} ms (OpenEtch Design)`, "");
    L.push(`Layout ${r.notes.layout}, "Use as an app" offer ${r.notes.appOffer ? "shown" : "not shown"}, input ${r.notes.input}${r.notes.nativeTouch ? ` (WebDriver touch probe: ${r.notes.nativeTouch.detail})` : ""}`, "");
    for (const s of r.results) {
      const probs = (s.problems ?? []).filter((p) => p.type !== "small");
      const small = (s.problems ?? []).find((p) => p.type === "small");
      const sf = s.safari ?? {};
      L.push(
        `- **${s.step}** \`${s.shot}\`${s.error ? ` ERROR: ${s.error}` : ""}${small ? ` (${small.count} small targets)` : ""} - inner ${sf.inner?.join("x")}, visual ${sf.visual?.join("/")}, safe t/r/b/l ${sf.env ? [sf.env.top, sf.env.right, sf.env.bottom, sf.env.left].join("/") : "-"}`,
      );
      if (s.notes) L.push(`  - notes: ${JSON.stringify(s.notes).slice(0, 300)}`);
      for (const p of probs) L.push(`  - ${p.severity} ${p.type}: ${p.detail}`);
    }
    for (const e of r.errors ?? []) L.push(`- console: ${String(e).slice(0, 240)}`);
    L.push("");
  }
  return L.join("\n");
}

async function main() {
  const a = args(process.argv.slice(2));
  if (a.help || !a.port || !a.mac) {
    console.log("usage: node e2e/safari.mjs --port <engine port> --mac <ssh host> (or OE_SAFARI_MAC) [--driver-port 4479] [--devices iphone,ipad] [--steps ...] [--out dir] [--no-tunnel] [--input native|synthetic|auto] [--desktop] [--keep-booted]");
    console.log(`devices: ${Object.keys(SIM_DEVICES).join(", ")}, or "Simulator name@iOS version"`);
    process.exitCode = a.help ? 0 : 2;
    return;
  }
  let site = null;
  if (a.siteUrl) {
    // OpenEtch Design as published (--url): nothing to serve, no engine.
  } else if (a.design) {
    // OpenEtch Design: the static site, served here; no engine.
    site = await serveSite({ dir: a.dir ?? join(here, "..", "dist-design"), port: a.port, base: a.base ?? "/OpenEtch/" });
    a.siteUrl = site.url;
  } else {
    const m = await api(`http://127.0.0.1:${a.port}`, "/api/machine").catch(() => null);
    if (!m || typeof m !== "object") throw new Error(`no engine at http://127.0.0.1:${a.port}`);
    if (m.simulated !== true) throw new Error("the engine does not report a simulator (simulated: true): never point this at a real machine");
  }
  const run = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const out = resolve(a.out ?? join(here, "..", "..", "target", "ux-evidence", `safari-${run}`));
  mkdirSync(out, { recursive: true });
  const bootedBefore = new Set(simulators(a.mac).filter((s) => s.state === "Booted").map((s) => s.udid));
  const tunnel = a.noTunnel ? null : await openTunnel(a);
  const runs = [];
  try {
    const keys = a.devices.split(",").filter(Boolean);
    if (a.desktop) keys.push("desktop");
    for (const key of keys) {
      console.log(`${key}:`);
      const r = await runDevice(a, key, join(out, key.replace(/[^a-z0-9-]+/gi, "-")));
      runs.push(r);
      if (r.error) console.log(`  session failed: ${r.error}`);
      writeFileSync(join(out, "report.json"), JSON.stringify(runs, null, 1));
      // Shut down a simulator this run booted before the next one boots (one at a time keeps the Mac responsive).
      if (r.udid && !bootedBefore.has(r.udid) && !a.keepBooted) mac(a.mac, `xcrun simctl shutdown ${r.udid}`, { allowFail: true });
    }
  } finally {
    if (tunnel) await closeTunnel(a, tunnel);
    if (site) await site.close();
    writeFileSync(join(out, "report.md"), markdown(runs));
  }
  console.log(`report: ${join(out, "report.md")}`);
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url)))
  main().catch((e) => {
    console.error(e);
    process.exit(1);
  });
