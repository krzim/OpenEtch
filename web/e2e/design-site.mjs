#!/usr/bin/env node
// The PUBLISHED OpenEtch Design site (web/dist-design, `npm run build:design`) on the device matrix: served as GitHub
// Pages serves it (static files under a base path, e2e/static.mjs), with the real in-browser backend (the worker
// shell and crates/webcore's design wasm) and no engine at all. Each device walks the editor end to end, with a
// screenshot and the layout check (overlap.mjs) after every step:
//
//   start page, every built-in example, drawing on the canvas, text in an uploaded font, image import and trace,
//   Fill Region, fit to stock, templates, the fixture LightBurn file imported, exported as .lbrn2 and imported
//   again, SVG / DXF / PDF export, saving in the browser, reload and reopen, the reload recovery copy, two tabs
//   (the other tab's save is refused until confirmed), Preview and the time estimate, the guide without the machine
//   pages, and no machine controls anywhere.
//
// Every network request the page makes is logged (Network domain): the site may load only its own files.
//
//   npm run build:design   (with VITE_BASE=/OpenEtch/ for the github.io base path)
//   node e2e/design-site.mjs [--dir dist-design] [--base /OpenEtch/] [--port 7976] [--devices iphone15-portrait,...]
//     [--out dir] [--cdp-port 9336] [--theme dark|light] [--headful] [--serve-only]
//
// Results: <out>/<device>-<theme>/NN-step.png, report.json and report.md (default out: the system temp folder).

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, Page, sleep } from "./cdp.mjs";
import { pickDevices } from "./devices.mjs";
import { runCheck } from "./overlap.mjs";
import { applySafeArea, escape, find, menuPath, moreSheet, phoneTab, pressFind } from "./scenarios.mjs";
import { serveSite } from "./static.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));
const repo = resolve(here, "..", "..");
const DEFAULT_DEVICES = "iphone15-portrait,ipad-air-landscape,macbook-air-1470,win-1920";

function args(argv) {
  const a = { dir: join(here, "..", "dist-design"), base: "/OpenEtch/", port: 7976, cdpPort: 9336 };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--dir") a.dir = v();
    else if (k === "--base") a.base = v();
    else if (k === "--port") a.port = Number(v());
    else if (k === "--out") a.out = v();
    else if (k === "--devices") a.devices = v();
    else if (k === "--theme") a.theme = v();
    else if (k === "--cdp-port") a.cdpPort = Number(v());
    else if (k === "--headful") a.headful = true;
    else if (k === "--serve-only") a.serveOnly = true;
  }
  a.out = resolve(a.out ?? join(tmpdir(), `oe-design-site-${new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19)}`));
  return a;
}

const b64 = (file) => readFileSync(file).toString("base64");
const FIXTURES = {
  lbrn2: join(repo, "crates", "lbrn", "tests", "fixtures", "shapes.lbrn2"),
  font: join(repo, "crates", "text", "tests", "fixtures", "ttf", "openetch-box.ttf"),
};

/** A 64 x 64 PNG with a dark disc on white (for import and trace), made here: no file needed. */
function discPng() {
  const w = 64;
  const raw = Buffer.alloc((w * 3 + 1) * w);
  for (let y = 0; y < w; y++) {
    raw[y * (w * 3 + 1)] = 0;
    for (let x = 0; x < w; x++) {
      const dark = (x - 32) ** 2 + (y - 32) ** 2 < 20 ** 2;
      raw.fill(dark ? 20 : 250, y * (w * 3 + 1) + 1 + x * 3, y * (w * 3 + 1) + 4 + x * 3);
    }
  }
  return pngOf(w, w, raw);
}

function crc32(buf) {
  let c = ~0;
  for (const b of buf) {
    c ^= b;
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}

async function pngOf(w, h, raw) {
  const { deflateSync } = await import("node:zlib");
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

/** A second tab in the same browser context (same IndexedDB, BroadcastChannel and sessionStorage rules as a tab). */
async function sameContextTab(page) {
  const conn = page.conn;
  const { targetId } = await conn.send("Target.createTarget", { url: "about:blank", browserContextId: page.browserContextId });
  const { sessionId } = await conn.send("Target.attachToTarget", { targetId, flatten: true });
  const p = new Page(conn, sessionId, targetId, page.browserContextId);
  await p.send("Page.enable");
  await p.send("Runtime.enable");
  conn.on((m) => {
    if (m.sessionId !== sessionId) return;
    if (m.method === "Runtime.exceptionThrown") p.errors.push(m.params.exceptionDetails?.exception?.description ?? m.params.exceptionDetails?.text);
  });
  // Only this tab goes: the browser context (and the first tab) stays.
  p.close = () => conn.send("Target.closeTarget", { targetId }).catch(() => {});
  return p;
}

/** Log every request of a page (the Network domain). */
async function logNetwork(page, into) {
  await page.send("Network.enable");
  page.conn.on((m) => {
    if (m.sessionId === page.sessionId && m.method === "Network.requestWillBeSent") into.push(m.params.request.url);
  });
}

const api = (page) => ({
  req: (method, path, body) => page.evaluate((m, p, b) => window.__oeDesign.request(m, p, b ?? undefined), method, path, body ?? null),
  bytes: (method, path, base64) => page.evaluate((m, p, b) => window.__oeDesign.bytes(m, p, b ?? undefined), method, path, base64 ?? null),
});

async function exec(page, id, params = {}) {
  const r = await api(page).req("POST", "/api/execute", { id, params, origin: "ui" });
  if (r.status !== 200) throw new Error(`${id}: ${r.status} ${JSON.stringify(r.body).slice(0, 300)}`);
  return r.body;
}

async function ready(page) {
  if (!(await page.waitFor(".topbar", { timeout: 30000 }))) throw new Error("the editor did not appear");
  if (!(await page.waitFor("!!window.__oeDesign", { expr: true, timeout: 30000 }))) throw new Error("no design backend");
  // The backend answers once its worker is ready.
  for (let i = 0; i < 100; i++) {
    const r = await api(page).req("GET", "/api/project").catch(() => null);
    if (r?.status === 200) return;
    await sleep(200);
  }
  throw new Error("the backend did not answer");
}

async function closeStartDialogs(page) {
  for (let i = 0; i < 3; i++) {
    const c = await find(page, ".modal-foot button, [role=dialog] button", "Close");
    if (!c) return;
    await page.press(c.x, c.y);
    await sleep(400);
  }
}

/** Button texts that would be machine control (none may show in OpenEtch Design). */
const MACHINE_WORDS = /^(start|start here|frame|framing|stop|jog|home|connect|find my laser|camera|run|approve)$/i;

function stepsFor(d) {
  const S = [];
  const add = (name, fn, kinds = ["phone", "tablet", "desktop"]) => kinds.includes(d.kind) && S.push({ name, fn });
  const wide = d.kind !== "phone";

  add("examples", async ({ page, note }) => {
    const list = (await exec(page, "example.list")).result;
    if (!Array.isArray(list) || list.length === 0) throw new Error("no examples");
    for (const ex of list) {
      const r = await exec(page, "doc.new", { example: ex.id });
      const doc = (await api(page).req("GET", "/api/document")).body;
      if (!doc.objects?.length) throw new Error(`example ${ex.id} has no objects`);
      note(`example ${ex.id}: ${doc.objects.length} objects (revision ${r.revision})`);
    }
    await sleep(800);
  });
  add("example-from-menu", async ({ page }) => {
    await exec(page, "doc.new");
    await escape(page);
    if (wide) {
      await menuPath(page, ["File", "New from example"]);
      await sleep(600);
    } else {
      await moreSheet(page);
    }
  });
  add("draw-rectangle", async ({ page }) => {
    await escape(page);
    await exec(page, "doc.new");
    await sleep(500);
    // The empty canvas's "Start a design" card has the rectangle tool on every layout.
    const tool = (await find(page, "button", "Draw a rectangle")) ?? (await find(page, ".toolbar button, [role=toolbar] button", "Rectangle"));
    if (!tool) throw new Error("no Rectangle tool");
    await page.press(tool.x, tool.y);
    await sleep(300);
    // Below and left of the "Start a design" card in the middle of an empty canvas.
    const c = await page.evaluate(() => {
      const el = document.querySelector("canvas.scene, .canvas-wrap canvas, canvas");
      const r = el.getBoundingClientRect();
      return { x0: r.left + r.width * 0.4, y0: r.top + r.height * 0.55, x1: r.left + r.width * 0.6, y1: r.top + r.height * 0.7 };
    });
    await page.drag(c.x0, c.y0, c.x1, c.y1);
    await sleep(800);
    const doc = (await api(page).req("GET", "/api/document")).body;
    if (!doc.objects?.length) throw new Error("drawing made no object");
  });
  add("text-uploaded-font", async ({ page, note }) => {
    await escape(page);
    const r = await api(page).bytes("POST", "/api/fonts/user?name=openetch-box.ttf", b64(FIXTURES.font));
    if (r.status !== 200) throw new Error(`font upload ${r.status}: ${Buffer.from(r.base64, "base64").toString()}`);
    const fonts = (await exec(page, "font.list")).result;
    if (!fonts.some((f) => f.family === "OpenEtch Box")) throw new Error("the uploaded font is not in font.list");
    const t = await exec(page, "shape.text", { text: "OE", font: { family: "OpenEtch Box" }, height_mm: 10, center: [0, 30] });
    note(`text in the uploaded font: ${t.result.created?.[0]}`);
    await exec(page, "shape.text", { text: "Hershey {date}", height_mm: 6, center: [0, -30] });
    await sleep(600);
  });
  add("font-picker", async ({ page, note }) => {
    // The text tool's bar has the font picker; OpenEtch Design adds "Add font…" there.
    await exec(page, "doc.new");
    await sleep(600);
    const tool = (await find(page, "button", "Add text")) ?? (await find(page, ".toolbar button, [role=toolbar] button", "Text"));
    if (!tool) throw new Error("no Text tool");
    await page.press(tool.x, tool.y);
    await sleep(500);
    const btn = await find(page, ".tx-font-btn");
    if (!btn) throw new Error("no font picker in the text bar");
    await page.press(btn.x, btn.y);
    await sleep(600);
    const add = await find(page, ".tx-font-pop button", "Add font");
    if (!add) throw new Error("no Add font button in the picker");
    const listed = await page.evaluate(() => document.querySelector(".tx-font-list")?.textContent?.includes("OpenEtch Box") ?? false);
    if (!listed) throw new Error("the uploaded font is not in the picker");
    note("font picker lists the uploaded font and offers Add font");
  });
  add("image-import-trace", async ({ page, note }) => {
    await escape(page);
    await escape(page);
    const png = (await discPng()).toString("base64");
    const up = await api(page).bytes("POST", "/api/document/upload?format=image&name=disc.png", png);
    if (up.status !== 200) throw new Error(`image upload ${up.status}`);
    const doc = (await api(page).req("GET", "/api/document")).body;
    const img = doc.objects.find((o) => o.geometry?.kind === "image");
    if (!img) throw new Error("no image object after the import");
    const tr = await exec(page, "image.trace", { id: img.id });
    note(`traced: ${JSON.stringify(tr.result).slice(0, 120)}`);
    await sleep(800);
  });
  add("fill-region", async ({ page, note }) => {
    await exec(page, "doc.new");
    await exec(page, "shape.rect", { width_mm: 40, height_mm: 30, center: [0, 0] });
    await exec(page, "shape.ellipse", { rx_mm: 8, ry_mm: 8, center: [0, 0] });
    const f = await exec(page, "path.fill_region", { points: [[15, 10]] });
    note(`fill region: ${f.result.created?.length ?? 0} shape(s)`);
    await sleep(600);
  });
  add("fit-to-stock", async ({ page }) => {
    await exec(page, "stock.define", { shape: { kind: "rect", width_mm: 80, height_mm: 50 }, thickness_mm: 3 });
    const doc = (await api(page).req("GET", "/api/document")).body;
    await exec(page, "art.place", { ids: doc.objects.map((o) => o.id), anchor: "center", fit: { mode: "contain", margin_mm: 5 } });
    await sleep(600);
  });
  add("templates", async ({ page, note }) => {
    const s = await api(page).req("POST", "/api/templates/save", { name: "Harness plate", overwrite: true, origin: "ui" });
    if (s.status !== 200) throw new Error(`template save ${s.status} ${JSON.stringify(s.body)}`);
    const list = (await api(page).req("GET", "/api/templates")).body;
    const o = await api(page).req("POST", "/api/templates/open", { name: "Harness plate", origin: "ui" });
    if (o.status !== 200) throw new Error(`template open ${o.status} ${JSON.stringify(o.body)}`);
    note(`templates: ${JSON.stringify(list).slice(0, 160)}`);
    await sleep(500);
  });
  add("lightburn-import", async ({ page, note }) => {
    const up = await api(page).bytes("POST", "/api/document/upload?format=lbrn2&mode=replace&name=shapes.lbrn2", b64(FIXTURES.lbrn2));
    if (up.status !== 200) throw new Error(`lbrn2 import ${up.status}: ${Buffer.from(up.base64, "base64").toString().slice(0, 300)}`);
    const doc = (await api(page).req("GET", "/api/document")).body;
    note(`shapes.lbrn2: ${doc.objects.length} objects, ${doc.layers.length} layers`);
    await sleep(800);
  });
  add("lightburn-round-trip", async ({ page, note }) => {
    const before = (await api(page).req("GET", "/api/document")).body;
    const dl = await api(page).bytes("GET", "/api/document/download?format=lbrn2");
    if (dl.status !== 200 || !Buffer.from(dl.base64, "base64").toString().includes("<LightBurnProject")) throw new Error(`lbrn2 export ${dl.status}`);
    const up = await api(page).bytes("POST", "/api/document/upload?format=lbrn2&mode=replace&name=round-trip.lbrn2", dl.base64);
    if (up.status !== 200) throw new Error(`re-import ${up.status}`);
    const after = (await api(page).req("GET", "/api/document")).body;
    if (after.objects.length !== before.objects.length) throw new Error(`round trip: ${before.objects.length} objects became ${after.objects.length}`);
    note(`.lbrn2 round trip: ${after.objects.length} objects, ${dl.disposition}`);
    if (wide) {
      await escape(page);
      await menuPath(page, ["File", "Export LightBurn"]);
    }
  });
  add("exports", async ({ page, note }) => {
    await escape(page);
    for (const format of ["svg", "dxf", "pdf", "ai"]) {
      const r = await api(page).bytes("GET", `/api/export?format=${format}`);
      const n = Buffer.from(r.base64, "base64").length;
      if (r.status !== 200 || n < 50) throw new Error(`${format} export: ${r.status}, ${n} bytes`);
      note(`${format}: ${n} bytes, ${r.type}, ${r.disposition}`);
    }
    if (wide) await menuPath(page, ["File", "Export"]);
  });
  add("save-in-browser", async ({ page, note }) => {
    await escape(page);
    const s = await api(page).req("POST", "/api/projects/save", { name: "Harness", origin: "ui" });
    if (s.status !== 200) throw new Error(`save ${s.status} ${JSON.stringify(s.body)}`);
    note(`saved ${JSON.stringify(s.body)}`);
    if (wide) await menuPath(page, ["File", "Open"]);
    await sleep(1200);
  });
  add("reload-and-reopen", async ({ page, note }) => {
    await escape(page);
    await page.send("Page.reload");
    await sleep(500);
    await ready(page);
    const list = (await api(page).req("GET", "/api/projects")).body;
    if (!list.some((p) => p.name === "Harness")) throw new Error(`after a reload: ${JSON.stringify(list)}`);
    const o = await api(page).req("POST", "/api/projects/open", { name: "Harness", origin: "ui" });
    if (o.status !== 200) throw new Error(`open ${o.status}`);
    const p = (await api(page).req("GET", "/api/project")).body;
    note(`reopened: ${JSON.stringify(p)}`);
    await sleep(800);
  });
  add("recovery-after-reload", async ({ page, note }) => {
    await exec(page, "shape.rect", { width_mm: 10, height_mm: 10, center: [30, 30], name: "Unsaved square" });
    await sleep(4000);
    await page.send("Page.reload");
    await sleep(500);
    await ready(page);
    await sleep(800);
    const doc = (await api(page).req("GET", "/api/document")).body;
    const p = (await api(page).req("GET", "/api/project")).body;
    if (!doc.objects.some((o) => o.name === "Unsaved square")) throw new Error("the unsaved edit did not come back after a reload");
    if (p.name !== "Harness" || p.dirty !== true) throw new Error(`after recovery the project is ${JSON.stringify(p)}`);
    const toast = await page.evaluate(() => [...document.querySelectorAll(".toast, [role=status]")].map((e) => e.textContent).join(" | "));
    note(`recovered: ${JSON.stringify(p)}; toast: ${toast}`);
  });
  add("two-tabs", async ({ page, note, ctx }) => {
    const other = await sameContextTab(page);
    try {
      await other.setDevice(d, { theme: ctx.theme });
      await other.goto(ctx.url);
      await ready(other);
      const o = await api(other).req("POST", "/api/projects/open", { name: "Harness", origin: "ui" });
      if (o.status !== 200) throw new Error(`the other tab could not open the project: ${o.status}`);
      await exec(other, "shape.rect", { width_mm: 6, height_mm: 6, center: [-30, -30] });
      const s1 = await api(other).req("POST", "/api/projects/save", { name: "Harness", origin: "ui" });
      if (s1.status !== 200) throw new Error(`the other tab's save ${s1.status}`);
    } finally {
      await other.close();
    }
    // Back to the first tab (a background tab gets no input events or frames).
    await page.conn.send("Target.activateTarget", { targetId: page.targetId });
    await page.send("Page.bringToFront").catch(() => {});
    await sleep(500);
    // This tab opened Harness before the other tab saved it: its save is refused until confirmed.
    const s2 = await api(page).req("POST", "/api/projects/save", { name: "Harness", origin: "ui" });
    note(`this tab's save after the other's: ${s2.status} ${s2.body?.code}`);
    if (s2.status !== 409 || s2.body?.code !== "changed_elsewhere") throw new Error(`expected 409 changed_elsewhere, got ${s2.status} ${JSON.stringify(s2.body)}`);
    if (wide) {
      await escape(page);
      await menuPath(page, ["File", "Save"]);
      await sleep(800);
      if (!(await find(page, "[role=dialog] button, .modal button", "Replace it"))) throw new Error("no confirmation before replacing the other tab's save");
    }
  });
  add("confirm-replace", async ({ page, note }) => {
    if (wide) {
      const b = await find(page, "[role=dialog] button, .modal button", "Replace it");
      if (b) await page.press(b.x, b.y);
      await sleep(1200);
    } else {
      await api(page).req("POST", "/api/projects/save", { name: "Harness", overwrite: true, origin: "ui" });
    }
    const p = (await api(page).req("GET", "/api/project")).body;
    if (p.dirty !== false) throw new Error(`not saved after confirming: ${JSON.stringify(p)}`);
    note(`saved over the other tab's version: ${JSON.stringify(p)}`);
  });
  add("preview-estimate", async ({ page, note }) => {
    await escape(page);
    if (d.kind === "phone") await phoneTab(page, "Preview");
    else await pressFind(page, ".workspaces button, nav[role=tablist] button", "Preview");
    await sleep(2500);
    const text = await page.evaluate(() => document.body.innerText);
    const m = /Estimate for [^\n]*/.exec(text);
    if (!m) throw new Error("no time estimate shown");
    const worker = await page.evaluate(() => (window.__oeWorker?.jobs ?? []).map((j) => `${j.kind}@${j.rev}`).slice(-4));
    note(`${m[0]}; preview jobs ${worker.join(", ")}`);
  });
  add("back-to-design", async ({ page }) => {
    await escape(page);
    if (d.kind === "phone") await phoneTab(page, "Canvas");
    else await pressFind(page, ".workspaces button, nav[role=tablist] button", "Design");
    await sleep(600);
  });
  add("no-machine-ui", async ({ page, note }) => {
    const hits = await page.evaluate((src) => {
      const re = new RegExp(src, "i");
      return [...document.querySelectorAll("button, a, [role=button], [role=menuitem]")]
        .filter((e) => {
          const r = e.getBoundingClientRect();
          return r.width > 0 && r.height > 0 && re.test((e.textContent ?? "").trim());
        })
        .map((e) => (e.textContent ?? "").trim());
    }, MACHINE_WORDS.source);
    if (hits.length) throw new Error(`machine controls shown: ${hits.join(", ")}`);
    note("no machine controls on screen");
  });
  add("guide", async ({ page, note, ctx }) => {
    const g = await sameContextTab(page);
    try {
      await g.setDevice(d, { theme: ctx.theme });
      await g.goto(`${ctx.url}?guide`);
      await sleep(2500);
      const text = await g.evaluate(() => document.body.innerText);
      const leaked = ["Running a job", "AI agents", "Framing", "Connecting the laser", "Find My Laser"].filter((s) => text.includes(s));
      await g.screenshot(join(ctx.dir, "guide.png"));
      if (leaked.length) throw new Error(`the guide shows machine pages: ${leaked.join(", ")}`);
      note(`guide contents shown without machine pages (${text.length} characters)`);
    } finally {
      await g.close();
      await page.conn.send("Target.activateTarget", { targetId: page.targetId });
    }
  });
  return S;
}

async function runDevice(browser, d, theme, ctx) {
  const dir = join(ctx.out, `${d.id}-${theme}`);
  mkdirSync(dir, { recursive: true });
  const page = await browser.newPage();
  const net = [];
  const results = [];
  const notes = [];
  try {
    await logNetwork(page, net);
    await page.setDevice(d, { theme });
    const t0 = Date.now();
    await page.goto(ctx.url, { timeout: 30000 });
    await ready(page);
    const firstLoadMs = Date.now() - t0;
    if (d.safe && Object.values(d.safe).some((v) => v > 0)) await applySafeArea(page, d.safe);
    const check = async (name, extra = {}) => {
      const shot = join(dir, `${String(results.length).padStart(2, "0")}-${name}.png`);
      await page.settle(250);
      await page.screenshot(shot);
      const c = await runCheck(page, { touch: d.touch, safe: d.safe }).catch((e) => ({ error: String(e), problems: [] }));
      results.push({ step: name, shot, ...extra, problems: c.problems, error: extra.error ?? c.error });
    };
    if (!(await page.evaluate(() => document.documentElement.hasAttribute("data-design-only")))) throw new Error("not the design build");
    await sleep(1200);
    await check("start-page", { notes: [`first load ${firstLoadMs} ms`] });
    await closeStartDialogs(page);
    for (const s of stepsFor(d)) {
      let error;
      const mine = [];
      try {
        await s.fn({ page, d, ctx: { ...ctx, theme, dir }, note: (n) => mine.push(n) });
      } catch (e) {
        error = String(e.message ?? e);
      }
      notes.push(...mine.map((n) => `${s.name}: ${n}`));
      await sleep(300);
      await check(s.name, { error, notes: mine });
    }
    const origin = new URL(ctx.url).origin;
    const foreign = [...new Set(net.filter((u) => !u.startsWith(`${origin}${ctx.base}`) && !u.startsWith("blob:") && !u.startsWith("data:")))];
    results.push({ step: "network", requests: net.length, foreign, problems: [], error: foreign.length ? `requests outside the site: ${foreign.join(", ")}` : undefined });
    if (page.errors.length) results.push({ step: "console-errors", errors: page.errors.slice(0, 20), problems: [] });
  } finally {
    await page.close();
  }
  return { device: d.id, name: d.name, theme, dir, results, notes };
}

function markdown(runs, ctx) {
  const lines = ["# OpenEtch Design site on the device matrix", "", `Site: ${ctx.url} (static files from ${ctx.dir})`, ""];
  for (const r of runs) {
    lines.push(`## ${r.name} (${r.device}, ${r.theme})`, "");
    for (const s of r.results) {
      const probs = (s.problems ?? []).filter((p) => p.type !== "small");
      const small = (s.problems ?? []).find((p) => p.type === "small");
      lines.push(`- **${s.step}** ${s.shot ? `\`${s.shot}\`` : ""}${s.error ? ` ERROR: ${s.error}` : ""}${small ? ` (${small.count} small targets: ${small.detail ?? ""})` : ""}`);
      for (const n of s.notes ?? []) lines.push(`  - ${n}`);
      for (const p of probs) lines.push(`  - ${p.severity} ${p.type}: ${p.detail}`);
      if (s.step === "network") lines.push(`  - ${s.requests} requests, outside the site: ${s.foreign.length ? s.foreign.join(", ") : "none"}`);
      if (s.errors) for (const e of s.errors) lines.push(`  - console: ${String(e).slice(0, 200)}`);
    }
    lines.push("");
  }
  return lines.join("\n");
}

async function main() {
  const a = args(process.argv.slice(2));
  const site = await serveSite({ dir: a.dir, port: a.port, base: a.base });
  if (a.serveOnly) {
    console.log(`serving ${a.dir} at ${site.url} (Ctrl+C to stop)`);
    return;
  }
  mkdirSync(a.out, { recursive: true });
  const browser = await launch({ port: a.cdpPort, headless: !a.headful });
  const runs = [];
  const ctx = { url: site.url, base: a.base, out: a.out, dir: a.dir };
  let failed = 0;
  try {
    for (const d of pickDevices(a.devices ?? DEFAULT_DEVICES)) {
      const theme = a.theme ?? d.themes[0];
      const r = await runDevice(browser, d, theme, ctx).catch((e) => ({ device: d.id, name: d.name, theme, results: [{ step: "run", error: String(e.stack ?? e), problems: [] }], notes: [] }));
      runs.push(r);
      const errors = r.results.filter((s) => s.error);
      const n = r.results.reduce((k, s) => k + (s.problems ?? []).filter((p) => p.type !== "small").length, 0);
      const small = r.results.reduce((k, s) => k + ((s.problems ?? []).find((p) => p.type === "small")?.count ?? 0), 0);
      failed += errors.length + n;
      console.log(`${d.id} ${theme}: ${r.results.length} steps, ${errors.length} step errors, ${n} layout problems, ${small} small targets`);
      for (const e of errors) console.log(`  ${e.step}: ${e.error}`);
      writeFileSync(join(a.out, "report.json"), JSON.stringify(runs, null, 1));
    }
  } finally {
    await browser.close();
    await site.close();
  }
  writeFileSync(join(a.out, "report.md"), markdown(runs, ctx));
  console.log(`report: ${join(a.out, "report.md")}`);
  if (failed) process.exitCode = 1;
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
