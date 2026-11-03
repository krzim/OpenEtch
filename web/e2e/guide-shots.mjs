#!/usr/bin/env node
// The user guide's screenshots: reads docs/guide/shots.json, opens each shot's built-in example on its emulated device,
// runs its steps (the language in guideSteps.mjs, documented in web/e2e/README.md "Guide screenshots") and writes
// docs/guide/img/<name>.webp. Public-safe by construction: only the built-in examples (doc.new {example}), no --doc
// and no path outside the repository, the light theme unless a shot says otherwise, a fixed date and time in the
// browser, the engine's address and any path or LAN address on screen replaced before the capture, and a refusal when
// something private is still visible.
//
//   npm run guide:shots -- --url http://127.0.0.1:7971 [--only name,name] [--check] [--out ../target/guide-shots]
//   npm run guide:shots -- --engine ../target/debug/openetch [--port 7971] [--web-dir dist]
//
// The engine must be a SIMULATOR on this computer (loopback; the script asks it and refuses otherwise): shots replace
// its document, and its device must not come from this computer's LightBurn prefs. With --engine the script starts one
// itself, with its own temporary --config-dir and --projects-dir, no LightBurn prefs, --approval click, --no-token, on
// 127.0.0.1, and stops it at the end.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { hostname, tmpdir, userInfo } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, sleep } from "./cdp.mjs";
import { DEVICES } from "./devices.mjs";
import { parseKey, shotsProblems } from "./guideSteps.mjs";
import { api, applySafeArea, canvasPoint, escape, execute, find, menuPath, phoneTab, pressFind, selectionCount, sheetOpen, sheetTo } from "./scenarios.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, "..", "..");
const GUIDE = join(REPO, "docs", "guide");
const RESERVED_PORTS = new Set([7878, 7979]);
/** The date and time the browser reports during a shot (UTC). */
const FIXED_TIME = Date.UTC(2026, 0, 15, 10, 30, 0);
const QUALITY = 82;

function fail(msg) {
  console.error(`guide-shots: ${msg}`);
  process.exit(2);
}

function insideRepo(p) {
  const r = relative(REPO, resolve(p));
  return r === "" || (!r.startsWith("..") && !isAbsolute(r));
}

function args(argv) {
  const a = { port: 7971 };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => {
      const x = argv[++i];
      if (x === undefined) fail(`${k} needs a value`);
      return x;
    };
    if (k === "--url") a.url = v();
    else if (k === "--only") a.only = v().split(",").map((s) => s.trim()).filter(Boolean);
    else if (k === "--check") a.check = true;
    else if (k === "--out") a.out = v();
    else if (k === "--shots") a.shots = v();
    else if (k === "--engine") a.engine = v();
    else if (k === "--port") a.port = Number(v());
    else if (k === "--web-dir") a.webDir = v();
    else if (k === "--cdp-port") a.cdpPort = Number(v());
    else if (k === "--headful") a.headful = true;
    else if (k === "--help" || k === "-h") a.help = true;
    else if (k === "--doc" || k.startsWith("--doc=")) fail("--doc is refused: guide screenshots use only the built-in example projects (shots.json \"example\")");
    else fail(`unknown option ${k}`);
  }
  for (const [k, p] of [
    ["--out", a.out],
    ["--shots", a.shots],
    ["--engine", a.engine],
    ["--web-dir", a.webDir],
  ])
    if (p && !insideRepo(p)) fail(`${k} ${p} is outside the repository; guide screenshots read and write only inside it`);
  return a;
}

/** The built-in example ids (crates/commands/src/builtin/examples.rs `EXAMPLES`). */
export function builtinExamples() {
  const src = readFileSync(join(REPO, "crates", "commands", "src", "builtin", "examples.rs"), "utf8");
  const list = src.slice(src.indexOf("pub const EXAMPLES"));
  return [...list.matchAll(/^\s+id: "([a-z0-9-]+)",/gm)].map((m) => m[1]);
}

// --- engine ------------------------------------------------------------------------------------------------------

async function startEngine(a) {
  if (RESERVED_PORTS.has(a.port)) fail(`port ${a.port} is reserved for the real laser's engine; pick another (--port)`);
  const bin = resolve(a.engine);
  if (!existsSync(bin) && !existsSync(`${bin}.exe`)) fail(`no engine at ${a.engine}`);
  const webDir = resolve(a.webDir ?? join(REPO, "web", "dist"));
  if (!existsSync(join(webDir, "index.html"))) fail(`${webDir} has no index.html: run npm run build first, or pass --web-dir`);
  const tmp = mkdtempSync(join(tmpdir(), "oe-guide-shots-"));
  mkdirSync(join(tmp, "cfg"));
  mkdirSync(join(tmp, "projects"));
  const listen = `127.0.0.1:${a.port}`;
  // --lightburn-prefs at a file that does not exist: the engine imports no LightBurn devices from this computer.
  const proc = spawn(bin, ["--config-dir", join(tmp, "cfg"), "--projects-dir", join(tmp, "projects"), "--lightburn-prefs", join(tmp, "no-lightburn-prefs.ini"), "--listen", listen, "--approval", "click", "--no-token", "--web-dir", webDir], { stdio: "ignore" });
  const url = `http://${listen}`;
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`${url}/api/machine`)).ok) break;
    } catch {
      /* not up yet */
    }
    await sleep(150);
  }
  return {
    url,
    stop() {
      try {
        proc.kill();
      } catch {
        /* gone */
      }
      setTimeout(() => rmSync(tmp, { recursive: true, force: true }), 500);
    },
  };
}

/** Refuse anything but a simulator engine on this computer. */
async function checkEngine(url) {
  let u;
  try {
    u = new URL(url);
  } catch {
    fail(`--url ${url} is not a URL`);
  }
  if (!["127.0.0.1", "localhost", "[::1]"].includes(u.hostname)) fail("--url must be an engine on this computer (127.0.0.1): shots replace its document");
  if (RESERVED_PORTS.has(Number(u.port || 80))) fail(`port ${u.port} is reserved for the real laser's engine`);
  const m = await api(url, "/api/machine").catch(() => null);
  if (!m || typeof m !== "object") fail(`no OpenEtch engine answers at ${url}`);
  if (!(m.simulated ?? m.sim)) fail("the engine is not a simulator (GET /api/machine): guide screenshots run only against the simulator");
  if (m.device?.source === "lightburn-prefs")
    fail("the engine's device came from this computer's LightBurn prefs: start it with a fresh --config-dir and --lightburn-prefs <a file that does not exist> (or use --engine)");
  return u;
}

// --- in the page ---------------------------------------------------------------------------------------------------

/** A fixed clock for the page (before any script runs). */
function clockScript(t) {
  return `(() => {
    const Real = Date, t0 = Real.now(), FIXED = ${t};
    const now = () => FIXED + (Real.now() - t0);
    const D = new Proxy(Real, {
      construct(target, a, nt) { return a.length ? Reflect.construct(target, a, nt) : Reflect.construct(target, [now()], nt); },
      apply() { return new Real(now()).toString(); },
      get(target, p, r) { return p === "now" ? now : Reflect.get(target, p, r); },
    });
    globalThis.Date = D;
  })();`;
}

/** Replace the engine's address and anything machine-specific on screen; return what still looks private. */
function scrubPage(o) {
  const subs = [
    [new RegExp(o.origin.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "g"), "http://openetch.local"],
    [new RegExp(o.host.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "g"), "openetch.local"],
    [/[A-Za-z]:\\Users\\[^\\\s"']+/g, "C:\\Users\\you"],
    [/\/(Users|home)\/[^/\s"']+/g, "/$1/you"],
    [/\b(127\.0\.0\.1|localhost)(:\d+)?\b/g, "openetch.local"],
  ];
  const fix = (s) => subs.reduce((acc, [re, to]) => acc.replace(re, to), s);
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const v = n.nodeValue;
    if (v && v.trim()) {
      const f = fix(v);
      if (f !== v) n.nodeValue = f;
    }
  }
  for (const el of document.querySelectorAll("input, textarea")) {
    const f = fix(el.value);
    if (f !== el.value) el.value = f;
    const ph = el.getAttribute("placeholder");
    if (ph && fix(ph) !== ph) el.setAttribute("placeholder", fix(ph));
  }
  // What is left: on screen text and field values.
  const text = `${document.body.innerText}\n${[...document.querySelectorAll("input, textarea")].map((e) => e.value).join("\n")}`;
  const bad = [
    /[A-Za-z]:\\Users\\(?!you\b)[^\\\s]+/,
    /\/(Users|home)\/(?!you\b)[^/\s]+/,
    /\b(10\.\d{1,3}|192\.168|172\.(1[6-9]|2\d|3[01]))\.\d{1,3}\.\d{1,3}\b/,
    ...o.private.map((w) => new RegExp(`\\b${w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\b`, "i")),
  ];
  return bad.map((re) => re.exec(text)?.[0]).filter(Boolean);
}

/** Close the example's notes, the "Use as an app" offer and any menu. */
async function closeDialogs(page, d) {
  await escape(page);
  if (d.kind !== "desktop") {
    for (let i = 0; i < 12; i++) {
      await sleep(250);
      const offer = await page.evaluate(() => [...document.querySelectorAll("[role=dialog]")].some((e) => /as an app/i.test(e.textContent ?? "")));
      if (offer) break;
    }
    await escape(page);
  }
}

const CONTROLS = "button, a[href], [role=button], [role=menuitem], [role=menuitemcheckbox], [role=menuitemradio], [role=tab], [role=option], [role=switch], [role=checkbox], summary, label, input, select, textarea";

/** The centre of a target: an accessible name, { css, text } or { object } (a shape on the canvas). */
async function locate(page, target, within) {
  if (target && typeof target === "object" && target.object) return objectPoint(page, target.object);
  if (target && typeof target === "object" && Array.isArray(target.point)) return { ...(await canvasPoint(page, target.point[0], target.point[1])), hit: true };
  const scope = (sel) => (within ? sel.split(",").map((s) => `${within} ${s.trim()}`).join(", ") : sel);
  const c = typeof target === "string" ? await find(page, scope(CONTROLS), target) : await find(page, scope(target.css), target.text);
  if (!c) throw new Error(`not found: ${JSON.stringify(target)}${within ? ` in ${within}` : ""}`);
  if (!c.hit) throw new Error(`not hit-testable: ${JSON.stringify(target)} is under ${c.cover}`);
  return c;
}

let docUrl = "";

/** A point on a shape (by name or id): its centre, else the middles of its bounding box's edges, whichever selects it. */
async function objectPoint(page, nameOrId) {
  const r = await execute(docUrl, "doc.inspect");
  const objects = r?.result?.objects ?? r?.objects ?? [];
  const o = objects.find((x) => x.id === nameOrId) ?? objects.find((x) => x.name === nameOrId);
  if (!o?.bounds) throw new Error(`no shape "${nameOrId}" in the document`);
  const b = o.bounds;
  // doc.inspect gives { min: [x, y], max: [x, y], center, size } (older shapes: [x0, y0, x1, y1] or named fields).
  const [x0, y0, x1, y1] = Array.isArray(b)
    ? b
    : Array.isArray(b.min) && Array.isArray(b.max)
      ? [b.min[0], b.min[1], b.max[0], b.max[1]]
      : [b.x0 ?? b.min_x, b.y0 ?? b.min_y, b.x1 ?? b.max_x, b.y1 ?? b.max_y];
  if (![x0, y0, x1, y1].every(Number.isFinite)) throw new Error(`shape "${nameOrId}" has no usable bounds`);
  const cx = (x0 + x1) / 2;
  const cy = (y0 + y1) / 2;
  const cands = [
    [cx, cy],
    [x0, cy],
    [cx, y1],
    [x1, cy],
    [cx, y0],
  ];
  // Text and thin art: points across the middle too (a glyph's stroke, not the gap between letters).
  for (const fx of [0.12, 0.25, 0.37, 0.5, 0.63, 0.75, 0.88]) for (const fy of [0.5, 0.35, 0.65]) cands.push([x0 + (x1 - x0) * fx, y0 + (y1 - y0) * fy]);
  return { id: o.id, candidates: await Promise.all(cands.map(([x, y]) => canvasPoint(page, x, y))) };
}

/** Whether the shape `id` is what got selected: its Browser row when the Browser is on the page, else any selection. */
async function selected(page, id) {
  if ((await selectionCount(page)) === 0) return false;
  if (!id) return true;
  return page.evaluate((oid) => {
    const rows = document.querySelectorAll("[data-kind=object][data-id]");
    if (!rows.length) return true;
    const row = document.querySelector(`[data-kind=object][data-id="${CSS.escape(oid)}"]`);
    return !row || row.classList.contains("selected");
  }, id);
}

async function pressTarget(page, d, target, within, how) {
  const c = await locate(page, target, within);
  const points = c.candidates ?? [c];
  for (const p of points) {
    if (how === "dbl") {
      if (d.touch) {
        await page.tap(p.x, p.y);
        await sleep(80);
        await page.tap(p.x, p.y);
      } else await page.click(p.x, p.y, { clickCount: 2 });
    } else if (how === "context") {
      if (d.touch) await page.longPress(p.x, p.y, 750);
      else await page.click(p.x, p.y, { button: "right" });
    } else await page.press(p.x, p.y);
    await sleep(350);
    // A shape: stop at the first point that selected it.
    if (!c.candidates || (await selected(page, c.id))) return;
    // Something else got selected (a shape under the one asked for): clear it and try the next point.
    if ((await selectionCount(page)) > 0) await page.key("Escape");
  }
  if (c.candidates) throw new Error(`could not select ${JSON.stringify(target)} on the canvas`);
}

async function runStep(page, d, step) {
  if ("exec" in step) {
    const r = await execute(docUrl, step.exec, step.params ?? {});
    if (r && typeof r === "object" && (r.error || r.ok === false)) throw new Error(`exec ${step.exec}: ${JSON.stringify(r.error ?? r).slice(0, 200)}`);
    await sleep(600);
  } else if ("click" in step) await pressTarget(page, d, step.click, step.in, "press");
  else if ("dblclick" in step) await pressTarget(page, d, step.dblclick, step.in, "dbl");
  else if ("set" in step) {
    // A field a person sets with a native picker (a select, a colour, a date): its value, as React sees an edit.
    const c = await locate(page, step.into);
    const ok = await page.evaluate(
      (x, y, value) => {
        const at = document.elementFromPoint(x, y);
        const el = at?.matches("input, select, textarea") ? at : at?.closest("label, .field")?.querySelector("input, select, textarea");
        if (!el) return false;
        const proto = el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
        Object.getOwnPropertyDescriptor(proto, "value").set.call(el, value);
        el.dispatchEvent(new Event("input", { bubbles: true }));
        el.dispatchEvent(new Event("change", { bubbles: true }));
        return true;
      },
      c.x,
      c.y,
      step.set,
    );
    if (!ok) throw new Error(`set: no field at ${JSON.stringify(step.into)}`);
  } else if ("hover" in step) {
    // The pointer stays there for the capture (a highlight under it), on a mouse device only.
    if (d.touch) throw new Error("hover: only devices with a mouse");
    const c = await locate(page, step.hover, step.in);
    const p = c.candidates?.[0] ?? c;
    await page.mouse("mouseMoved", p.x - 3, p.y, { button: "none" });
    await page.mouse("mouseMoved", p.x, p.y, { button: "none" });
  }
  else if ("context" in step) {
    if (step.context === "selection") {
      const box = await page.evaluate(() => {
        const r = document.querySelector(".viewport .sel-box, .viewport .sel-bounds")?.getBoundingClientRect();
        return r ? { x: r.left + r.width / 2, y: r.top + r.height / 2 } : null;
      });
      if (!box) throw new Error("context: nothing is selected");
      if (d.touch) await page.longPress(box.x, box.y, 750);
      else await page.click(box.x, box.y, { button: "right" });
    } else await pressTarget(page, d, step.context, step.in, "context");
  } else if ("menu" in step) await menuPath(page, step.menu);
  else if ("type" in step) {
    if (step.into) {
      const c = await locate(page, step.into);
      await page.press(c.x, c.y);
      await sleep(150);
      await page.evaluate(() => document.activeElement?.select?.());
    }
    await page.type(step.type);
  } else if ("key" in step) {
    const k = parseKey(step.key);
    await page.key(k.key, { modifiers: k.modifiers });
  } else if ("hash" in step) {
    await page.evaluate((h) => {
      history.pushState(null, "", h);
      window.dispatchEvent(new HashChangeEvent("hashchange"));
    }, step.hash);
  } else if ("wait" in step) {
    if (typeof step.wait === "number") await sleep(step.wait);
    else if (!(await page.waitFor(step.wait, { timeout: 10000 }))) throw new Error(`wait: ${step.wait} did not appear`);
  } else if ("select" in step) {
    if (step.select === "all") await page.key("a", { modifiers: 2 });
    else if (step.select === "none") await page.key("Escape");
    else {
      for (const [i, name] of step.select.entries()) {
        if (i > 0 && d.touch) throw new Error("select: more than one shape needs a keyboard (Shift); use \"all\" on touch devices");
        const c = await objectPoint(page, name);
        let done = false;
        for (const p of c.candidates) {
          if (d.touch) await page.tap(p.x, p.y);
          else await page.click(p.x, p.y, { modifiers: i > 0 ? 8 : 0 });
          await sleep(350);
          if (await selected(page, c.id)) {
            done = true;
            break;
          }
          if (i === 0 && (await selectionCount(page)) > 0) await page.key("Escape");
        }
        if (!done) throw new Error(`select: could not select "${name}"`);
      }
    }
  } else if ("workspace" in step) {
    if (d.kind === "phone") await phoneTab(page, step.workspace === "laser" ? "Job" : "Canvas");
    else await pressFind(page, ".workspaces button, nav[role=tablist] button", step.workspace === "laser" ? "Laser" : "Design");
    await sleep(600);
  } else if ("tab" in step) {
    if (d.kind !== "phone") throw new Error("tab: only phones have the bottom tabs");
    const active = await page.evaluate(() => document.querySelector(".phone-tab.active")?.textContent?.trim());
    if (active !== step.tab || !(await sheetOpen(page))) await phoneTab(page, step.tab);
    if (step.sheet === "closed") {
      if (await sheetOpen(page)) await pressFind(page, ".sheet-close");
    } else {
      if (!(await sheetOpen(page))) await phoneTab(page, step.tab);
      if (step.sheet && !(await sheetTo(page, step.sheet))) throw new Error(`tab: the sheet did not reach ${step.sheet}`);
    }
  } else if ("escape" in step) await escape(page);
  else if ("scroll" in step) {
    const ok = await page.evaluate((s) => {
      const e = document.querySelector(s);
      if (!e) return false;
      // The nearest scrolling box only (scrollIntoView would also shift the app's overflow-hidden frames).
      for (let p = e.parentElement; p; p = p.parentElement) {
        const oy = getComputedStyle(p).overflowY;
        if ((oy === "auto" || oy === "scroll") && p.scrollHeight > p.clientHeight) {
          p.scrollTop += e.getBoundingClientRect().top - p.getBoundingClientRect().top - 8;
          break;
        }
      }
      return true;
    }, step.scroll);
    if (!ok) throw new Error(`scroll: no ${step.scroll}`);
  }
  await sleep(250);
}

async function shoot(browser, shot, o) {
  const d = DEVICES.find((x) => x.id === shot.device);
  const page = await browser.newPage();
  try {
    await page.setDevice(d, { theme: shot.theme ?? "light" });
    await page.send("Emulation.setTimezoneOverride", { timezoneId: "UTC" });
    await page.send("Emulation.setLocaleOverride", { locale: "en-GB" }).catch(() => {});
    await page.send("Page.addScriptToEvaluateOnNewDocument", { source: clockScript(FIXED_TIME) });
    // No caret and no hover leftovers in a picture.
    await page.send("Page.addScriptToEvaluateOnNewDocument", {
      source: `addEventListener("DOMContentLoaded", () => { const s = document.createElement("style"); s.textContent = "*{caret-color:transparent!important}"; document.head.appendChild(s); });`,
    });
    // The simulator idle: no frame and no run left over from the shot before (one that started a job or waits at a
    // layer pause).
    await api(o.url, "/api/machine/frame/stop", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ origin: "ui" }) }).catch(() => {});
    await api(o.url, "/api/machine/stop", { method: "POST" }).catch(() => {});
    const opened = await execute(o.url, "doc.new", { example: shot.example });
    if (opened && typeof opened === "object" && opened.error) throw new Error(`doc.new ${shot.example}: ${JSON.stringify(opened.error)}`);
    await page.goto(`${o.url}/`, { waitFor: ".btn-stop", timeout: 20000 });
    if (d.safe && Object.values(d.safe).some((v) => v > 0)) await applySafeArea(page, d.safe);
    await sleep(1500);
    await closeDialogs(page, d);
    for (const [i, step] of shot.steps.entries()) {
      try {
        await runStep(page, d, step);
      } catch (e) {
        throw new Error(`step ${i + 1} ${JSON.stringify(step)}: ${e.message ?? e}`);
      }
    }
    await page.evaluate(() => document.activeElement instanceof HTMLElement && !document.activeElement.matches("input, textarea") && document.activeElement.blur());
    if (!("hover" in (shot.steps.at(-1) ?? {}))) await page.mouse("mouseMoved", 1, 1, { button: "none" }).catch(() => {});
    await page.settle(400);
    const left = await page.evaluate(scrubPage, { origin: o.url, host: o.host, private: o.privateWords });
    if (left.length) throw new Error(`refused: something private is on screen (${left.join(", ")})`);
    let clip;
    if (shot.crop) {
      clip = await page.evaluate((sel) => {
        const e = document.querySelector(sel);
        if (!e) return null;
        const r = e.getBoundingClientRect();
        const pad = 8;
        const x = Math.max(0, r.left - pad);
        const y = Math.max(0, r.top - pad);
        return { x, y, width: Math.min(innerWidth, r.right + pad) - x, height: Math.min(innerHeight, r.bottom + pad) - y };
      }, shot.crop);
      if (!clip || clip.width < 10 || clip.height < 10) throw new Error(`crop: no visible ${shot.crop}`);
    } else clip = { x: 0, y: 0, width: d.width, height: d.height };
    const maxW = shot.maxWidth ?? 1600;
    const scale = Math.min(1, maxW / (clip.width * d.dpr));
    const { data } = await page.send("Page.captureScreenshot", { format: "webp", quality: QUALITY, clip: { ...clip, scale }, captureBeyondViewport: false });
    const file = join(o.out, `${shot.name}.webp`);
    writeFileSync(file, Buffer.from(data, "base64"));
    return { file, bytes: Buffer.byteLength(data, "base64"), size: `${Math.round(clip.width * d.dpr * scale)}x${Math.round(clip.height * d.dpr * scale)}` };
  } finally {
    await page.close();
  }
}

async function main() {
  const a = args(process.argv.slice(2));
  if (a.help) {
    console.log(readFileSync(join(here, "..", "..", "web", "e2e", "README.md"), "utf8").split("## Guide screenshots")[1]?.split("\n## ")[0] ?? "see web/e2e/README.md");
    return;
  }
  const shotsFile = resolve(a.shots ?? join(GUIDE, "shots.json"));
  const shots = JSON.parse(readFileSync(shotsFile, "utf8"));
  const problems = shotsProblems(shots, { devices: DEVICES.map((x) => x.id), examples: builtinExamples() });
  if (problems.length) fail(`${relative(REPO, shotsFile)}:\n  ${problems.join("\n  ")}`);
  const picked = a.only ? shots.filter((s) => a.only.includes(s.name)) : shots;
  if (a.only) for (const n of a.only) if (!shots.some((s) => s.name === n)) fail(`--only: no shot "${n}"`);
  if (a.check) {
    console.log(`${relative(REPO, shotsFile)}: ${shots.length} shots, no problems`);
    return;
  }
  if (!a.url && !a.engine) fail("give --url http://127.0.0.1:<port> (a simulator engine) or --engine <openetch binary>");
  const engine = a.engine ? await startEngine(a) : null;
  const url = (engine?.url ?? a.url).replace(/\/$/, "");
  const out = resolve(a.out ?? join(GUIDE, "img"));
  mkdirSync(out, { recursive: true });
  let failed = 0;
  try {
    const u = await checkEngine(url);
    docUrl = url;
    // Words that must never show in a picture: this computer's name and the user's login.
    const privateWords = [hostname(), userInfo().username].filter((w) => w && w.length >= 3 && !/^(localhost|user|admin|root)$/i.test(w));
    const browser = await launch({ port: a.cdpPort ?? 9335, headless: !a.headful });
    try {
      for (const shot of picked) {
        const t0 = Date.now();
        try {
          const r = await shoot(browser, shot, { url, host: u.host, out, privateWords });
          console.log(`${shot.name}: ${relative(REPO, r.file)} ${r.size} ${(r.bytes / 1024).toFixed(0)} KB (${((Date.now() - t0) / 1000).toFixed(1)} s)`);
        } catch (e) {
          failed++;
          console.error(`${shot.name}: FAILED ${e.message ?? e}`);
        }
      }
    } finally {
      await browser.close();
    }
  } finally {
    engine?.stop();
  }
  if (failed) {
    console.error(`${failed} of ${picked.length} shots failed`);
    process.exitCode = 1;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url)))
  main().catch((e) => {
    console.error(e);
    process.exit(1);
  });
