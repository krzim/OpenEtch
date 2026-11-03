#!/usr/bin/env node
// End-to-end performance profile (web/e2e/README.md, "Performance profile"): headless Edge or Chrome over the DevTools
// Protocol walks the everyday actions on a heavy document and records, per action, the wall time until the UI settles,
// every HTTP request (method, path, bytes on the wire, server time, transfer time), main-thread long tasks, the time
// spent in JSON.parse on big payloads and the renderer's task / script / layout time. It also times the engine's big
// GETs directly (no browser) and how much gzip / brotli would shrink them.
//
//   node e2e/perf.mjs --url http://127.0.0.1:7931 --doc heavy.lbrn2 [--label pc-local] [--out ../target/perf/pc-local]
//                     [--devices macbook-air-1470,iphone15-portrait] [--only load,move,undo] [--quiet 1500]
//                     [--timeout 240000] [--cdp-port 9334] [--headful] [--throttle-mbps 20] [--latency-ms 5]
//
// --throttle-mbps limits the page's network to that many megabits per second each way (DevTools network emulation,
// with --latency-ms of added round-trip latency, default 5), so a local engine shows what Wi-Fi to a Raspberry Pi
// would: 20 Mbit/s is about 2.5 MB/s. The quick loop while improving something:
//
//   node e2e/perf.mjs --url http://127.0.0.1:7931 --doc heavy.lbrn2 --throttle-mbps 20 --only move,undo,to-laser-preview,layer-speed
//
// It needs an OpenEtch engine with the SIMULATOR (never a real machine: it starts framing, and it replaces the engine's
// document and makes edits). Results: <out>/perf.json and <out>/perf.md. Keep private documents out of the repository:
// the default output folder is under target/, which git ignores.
//
// "Settled" means: no HTTP request of the action is still in flight and nothing happened (request finished, long task
// ended) for --quiet ms; the wall time runs from the input to the last such activity. GET /api/machine (the status
// poll) is listed but does not hold the settle open. Actions the phone cannot reach the same way as the desktop are
// sent as the same engine request from inside the page (`via: "api"`), so the app's reaction (event, refetches,
// render) is still what is measured.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync, gzipSync, brotliCompressSync, constants as zc } from "node:zlib";
import { launch, sleep } from "./cdp.mjs";
import { pickDevices } from "./devices.mjs";
import { api, applySafeArea, canvasPoint, escape, execute, find, phoneTab, pressFind, selectionCount, sheetOpen, TARGETS } from "./scenarios.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));

function args(argv) {
  const a = { devices: "macbook-air-1470,iphone15-portrait", quiet: 1500, timeout: 240000, port: 9334 };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--url") a.url = v().replace(/\/$/, "");
    else if (k === "--doc") a.doc = v();
    else if (k === "--label") a.label = v();
    else if (k === "--out") a.out = v();
    else if (k === "--devices") a.devices = v();
    else if (k === "--only") a.only = v();
    else if (k === "--quiet") a.quiet = Number(v());
    else if (k === "--timeout") a.timeout = Number(v());
    else if (k === "--cdp-port") a.port = Number(v());
    else if (k === "--headful") a.headful = true;
    else if (k === "--throttle-mbps") a.throttleMbps = Number(v());
    else if (k === "--latency-ms") a.latencyMs = Number(v());
    else if (k === "--no-direct") a.noDirect = true;
    else if (k === "--help" || k === "-h") a.help = true;
  }
  return a;
}

// --- recording -----------------------------------------------------------------------------------------------------

/** Installed before the app's own scripts: long tasks and JSON.parse of big strings, in epoch ms. */
const IN_PAGE = `(() => {
  const P = (window.__perf = { lt: [], parse: [] });
  try {
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) P.lt.push([performance.timeOrigin + e.startTime, e.duration]);
    }).observe({ type: "longtask", buffered: true });
  } catch {}
  const parse = JSON.parse;
  JSON.parse = function (s, r) {
    if (typeof s === "string" && s.length > 50000) {
      const t0 = performance.now();
      const v = parse.call(this, s, r);
      P.parse.push([performance.timeOrigin + t0, performance.now() - t0, s.length]);
      return v;
    }
    return parse.call(this, s, r);
  };
})();`;

/** Network events of one page, with node-side arrival times (epoch ms). */
class NetLog {
  constructor(page) {
    this.reqs = new Map();
    this.lastActivity = 0;
    page.conn.on((m) => {
      if (m.sessionId !== page.sessionId) return;
      const p = m.params;
      const now = Date.now();
      switch (m.method) {
        case "Network.requestWillBeSent": {
          if (p.type === "WebSocket" || /^(data|blob):/.test(p.request.url)) return;
          const u = new URL(p.request.url);
          this.reqs.set(p.requestId, { id: p.requestId, method: p.request.method, path: u.pathname + u.search, start: now, type: p.type });
          this.lastActivity = now;
          break;
        }
        case "Network.responseReceived": {
          const r = this.reqs.get(p.requestId);
          if (!r) return;
          const t = p.response.timing;
          r.status = p.response.status;
          r.encoding = p.response.headers?.["content-encoding"] ?? p.response.headers?.["Content-Encoding"] ?? null;
          if (t) {
            r.serverMs = Math.max(0, t.receiveHeadersEnd - t.sendEnd);
            r.headersAt = t.requestTime + t.receiveHeadersEnd / 1000;
            r.connectMs = t.connectEnd >= 0 ? t.connectEnd - t.connectStart : 0;
          }
          break;
        }
        case "Network.loadingFinished": {
          const r = this.reqs.get(p.requestId);
          if (!r) return;
          r.end = now;
          r.bytes = p.encodedDataLength;
          if (r.headersAt) r.transferMs = Math.max(0, (p.timestamp - r.headersAt) * 1000);
          this.lastActivity = now;
          break;
        }
        case "Network.loadingFailed": {
          const r = this.reqs.get(p.requestId);
          if (!r) return;
          r.end = now;
          r.failed = p.errorText;
          this.lastActivity = now;
          break;
        }
      }
    });
  }

  since(t0) {
    return [...this.reqs.values()].filter((r) => r.start >= t0);
  }
}

/** The status poll: listed, but it does not keep an action open. */
const isPoll = (r) => r.method === "GET" && r.path.startsWith("/api/machine") && !r.path.startsWith("/api/machine/");

async function pageState(page) {
  return page.evaluate(() => ({ lt: window.__perf?.lt.length ?? 0, parse: window.__perf?.parse.length ?? 0 }));
}

async function metrics(page) {
  const { metrics } = await page.send("Performance.getMetrics");
  const m = Object.fromEntries(metrics.map((x) => [x.name, x.value]));
  return { task: m.TaskDuration * 1000, script: m.ScriptDuration * 1000, layout: m.LayoutDuration * 1000, style: m.RecalcStyleDuration * 1000, heapMB: m.JSHeapUsedSize / 1048576 };
}

/** Run `fn` (the input) and wait until the action settles; returns the record. */
async function measure(ctx, name, fn, { via = "ui", note, always = false } = {}) {
  const { page, net, opts } = ctx;
  if (opts.only && !opts.only.split(",").includes(name)) {
    // Not measured, but steps the later ones build on (the page load, a workspace switch) still run.
    if (always) await fn();
    return null;
  }
  const before = await pageState(page);
  const m0 = await metrics(page);
  const t0 = Date.now();
  let error = null;
  let frameMs = null;
  try {
    const r = await fn();
    if (r && typeof r === "object" && r.via) via = r.via;
    // Input to the next painted frame.
    await page.evaluate("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))").catch(() => {});
    frameMs = Date.now() - t0;
  } catch (e) {
    error = String(e?.message ?? e);
  }
  // Settle.
  let timedOut = false;
  let lastSeen = t0;
  for (;;) {
    await sleep(100);
    await page.evaluate("new Promise(r => requestAnimationFrame(() => r(1)))").catch(() => {});
    const now = Date.now();
    const reqs = net.since(t0).filter((r) => !isPoll(r));
    const inflight = reqs.filter((r) => !r.end).length;
    const lt = await page.evaluate(() => window.__perf.lt.slice(-3)).catch(() => []);
    const ltEnd = Math.max(0, ...lt.map(([s, d]) => s + d).filter((e) => e >= t0));
    const netEnd = Math.max(0, ...reqs.map((r) => r.end ?? now));
    lastSeen = Math.max(t0 + (frameMs ?? 0), ltEnd, netEnd);
    if (!inflight && now - lastSeen >= opts.quiet) break;
    if (now - t0 > opts.timeout) {
      timedOut = true;
      break;
    }
  }
  const wallMs = Math.round(lastSeen - t0);
  const m1 = await metrics(page);
  const perf = await page.evaluate((b) => ({ lt: window.__perf.lt.slice(b.lt), parse: window.__perf.parse.slice(b.parse) }), before);
  const lts = perf.lt.filter(([s]) => s >= t0 - 50);
  const requests = net
    .since(t0)
    .filter((r) => r.start <= lastSeen + 1)
    .map((r) => ({
      method: r.method,
      path: r.path,
      status: r.status ?? null,
      bytes: r.bytes ?? null,
      encoding: r.encoding ?? null,
      startMs: r.start - t0,
      totalMs: r.end ? r.end - r.start : null,
      serverMs: r.serverMs != null ? Math.round(r.serverMs) : null,
      transferMs: r.transferMs != null ? Math.round(r.transferMs) : null,
      poll: isPoll(r) || undefined,
      failed: r.failed,
    }));
  const rec = {
    name,
    via,
    wallMs,
    frameMs,
    timedOut: timedOut || undefined,
    error,
    note,
    requests,
    bytes: requests.reduce((s, r) => s + (r.bytes ?? 0), 0),
    longTasks: { count: lts.length, totalMs: Math.round(lts.reduce((s, [, d]) => s + d, 0)), maxMs: Math.round(Math.max(0, ...lts.map(([, d]) => d))) },
    jsonParse: perf.parse.map(([, ms, len]) => ({ ms: Math.round(ms), chars: len })),
    renderer: {
      taskMs: Math.round(m1.task - m0.task),
      scriptMs: Math.round(m1.script - m0.script),
      layoutMs: Math.round(m1.layout - m0.layout),
      styleMs: Math.round(m1.style - m0.style),
      heapMB: Math.round(m1.heapMB),
    },
  };
  const big = requests.filter((r) => (r.bytes ?? 0) > 100000).map((r) => `${r.method} ${r.path.split("?")[0]} ${(r.bytes / 1e6).toFixed(1)}MB ${r.totalMs}ms`);
  console.log(`  ${name.padEnd(20)} ${String(wallMs).padStart(7)} ms  ${String(requests.length).padStart(2)} req ${(rec.bytes / 1e6).toFixed(1).padStart(6)} MB  lt ${rec.longTasks.totalMs} ms${error ? `  ERROR ${error}` : ""}${timedOut ? "  TIMEOUT" : ""}  ${big.join("; ")}`);
  ctx.results.push(rec);
  return rec;
}

// --- page actions --------------------------------------------------------------------------------------------------

/** An engine request sent from inside the page (recorded by the Network domain; the app reacts to its events). */
async function inPage(page, method, path, body) {
  return page.evaluate(
    async (m, p, b) => {
      const r = await fetch(p, { method: m, headers: b ? { "content-type": "application/json" } : {}, body: b ? JSON.stringify(b) : undefined });
      return { status: r.status, text: (await r.text()).slice(0, 400) };
    },
    method,
    path,
    body ?? null,
  );
}

const inPageExec = (page, id, params) => inPage(page, "POST", "/api/execute", { id, params, origin: "ui" });

async function clickIf(page, selector, text) {
  const c = await find(page, selector, text);
  if (!c || !c.hit) return false;
  await page.press(c.x, c.y);
  return true;
}

// Disc centres of a 2 x 3 jig around the origin (TARGETS first), tried in order for a point not under an overlay.
const PICKS = [...TARGETS, [-11.9, -49.9], [28.1, -49.9], [-51.9, -49.9]];

/** Whether a viewport point reaches the canvas (not a toolbar, sheet or modifier bar over it). */
async function onCanvas(page, p) {
  return page.evaluate((x, y) => {
    const e = document.elementFromPoint(x, y);
    return !!e && !!e.closest(".viewport") && !e.closest("button, .vp-overlay, .touch-mods, .et-bar, [role=toolbar]");
  }, p.x, p.y);
}

/** A small deterministic RGB PNG (a gradient with rings), for the image import. */
function testPng(w = 400, h = 300) {
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (w * 3 + 1)] = 0;
    for (let x = 0; x < w; x++) {
      const v = Math.round(127 + 120 * Math.sin(Math.hypot(x - w / 2, y - h / 2) / 9)) & 255;
      const o = y * (w * 3 + 1) + 1 + x * 3;
      raw[o] = v;
      raw[o + 1] = (x * 255) / w;
      raw[o + 2] = (y * 255) / h;
    }
  }
  const crc = (buf) => {
    let c = ~0;
    for (const b of buf) {
      c ^= b;
      for (let k = 0; k < 8; k++) c = c & 1 ? (c >>> 1) ^ 0xedb88320 : c >>> 1;
    }
    return ~c >>> 0;
  };
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const c = Buffer.alloc(4);
    c.writeUInt32BE(crc(td));
    return Buffer.concat([len, td, c]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

/** A mid-size SVG (a grid of stars and circles, ~150 objects) for the SVG import. */
function testSvg() {
  const parts = [];
  for (let r = 0; r < 6; r++)
    for (let c = 0; c < 12; c++) {
      const cx = 5 + c * 6;
      const cy = 5 + r * 6;
      parts.push(`<circle cx="${cx}" cy="${cy}" r="2.5" fill="none" stroke="#f00"/>`);
      let d = "";
      for (let k = 0; k < 10; k++) {
        const a = (k / 10) * Math.PI * 2;
        const rr = k % 2 ? 0.8 : 2;
        d += `${k ? "L" : "M"}${(cx + rr * Math.cos(a)).toFixed(2)},${(cy + rr * Math.sin(a)).toFixed(2)}`;
      }
      parts.push(`<path d="${d}Z" fill="#00f"/>`);
    }
  return `<svg xmlns="http://www.w3.org/2000/svg" width="75mm" height="40mm" viewBox="0 0 75 40">${parts.join("")}</svg>`;
}

/** Hand files to the app's hidden import picker (File > Import), as a user picking them would. */
async function pickFiles(page, files) {
  const { root } = await page.send("DOM.getDocument", { depth: 0 });
  const { nodeIds } = await page.send("DOM.querySelectorAll", { nodeId: root.nodeId, selector: "input[type=file][multiple]" });
  if (!nodeIds.length) throw new Error("no import file input");
  await page.send("DOM.setFileInputFiles", { nodeId: nodeIds[nodeIds.length - 1], files });
}

async function docInfo(url) {
  const d = await api(url, "/api/document");
  const h = await api(url, "/api/history");
  return { doc: d, history: h };
}

/** Accept whatever import dialog shows up (image settings and the like); returns as soon as the import was sent. */
async function acceptDialog(page, net, t0) {
  for (let i = 0; i < 40; i++) {
    await sleep(50);
    if (net.since(t0).some((r) => r.method === "POST")) return null;
    const open = await page.evaluate(() => !![...document.querySelectorAll("[role=dialog]")].find((e) => e.getBoundingClientRect().width > 0));
    if (!open) continue;
    for (const l of ["Import", "OK", "Create", "Place", "Add", "Apply"]) if (await clickIf(page, "[role=dialog] .modal-foot button, [role=dialog] button", l)) return l;
  }
  return null;
}

// --- the walk ------------------------------------------------------------------------------------------------------

async function walk(browser, d, opts) {
  const page = await browser.newPage();
  const net = new NetLog(page);
  const ctx = { page, net, opts, results: [], device: d.id };
  const phone = d.kind === "phone";
  const url = opts.url;
  await page.send("Network.enable", { maxResourceBufferSize: 1024, maxTotalBufferSize: 1 << 20 });
  await page.send("Network.setCacheDisabled", { cacheDisabled: false });
  if (opts.throttleMbps > 0) {
    // Bytes per second each way; the latency is added to every request.
    const bps = (opts.throttleMbps * 1e6) / 8;
    await page.send("Network.emulateNetworkConditions", { offline: false, latency: opts.latencyMs ?? 5, downloadThroughput: bps, uploadThroughput: bps });
  }
  await page.send("Performance.enable", { timeDomain: "timeTicks" });
  await page.send("Page.addScriptToEvaluateOnNewDocument", { source: IN_PAGE });
  await page.send("DOM.enable");
  await page.setDevice(d, { theme: "dark" });
  // Skip first-visit offers and hints that would cover the canvas (as in journeys.mjs).
  await page.goto(url + "/api/machine");
  await page.evaluate(() => {
    try {
      localStorage.setItem("openetch.appModeOffered", "1");
    } catch {}
  });

  // The engine's document: a fresh heavy one (not timed here; see `direct` for the import itself).
  await api(url, "/api/machine/frame/stop", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ origin: "ui" }) }).catch(() => {});
  await execute(url, "doc.new");
  const up = await api(url, `/api/document/upload?format=lbrn2&mode=replace&name=${encodeURIComponent(basename(opts.doc))}`, { method: "POST", body: readFileSync(opts.doc) });
  if (!up?.ok) throw new Error(`upload failed: ${JSON.stringify(up).slice(0, 200)}`);

  // 1. Initial load (the app shell, then document, history, commands, machine...).
  await measure(ctx, "load", async () => {
    await page.goto(url + "/", { waitFor: ".btn-stop", timeout: opts.timeout });
  }, { always: true });
  if (Object.values(d.safe ?? {}).some((v) => v > 0)) await applySafeArea(page, d.safe);
  // Close the "use as an app" offer if it came.
  await sleep(800);
  await clickIf(page, "[role=dialog] .modal-foot button", "Close");
  await escape(page);
  if (phone && (await sheetOpen(page))) await clickIf(page, ".sheet-close");
  const fit = await find(page, "button", "Fit field");
  if (fit?.hit) await page.press(fit.x, fit.y);
  await sleep(800);

  // 2. Select an object (found first, then the press on it timed), then move it (drag 4 mm right).
  let sel = null;
  for (const [x, y] of PICKS) {
    const p = await canvasPoint(page, x, y);
    if (!(await onCanvas(page, p))) continue;
    await page.press(p.x, p.y);
    await sleep(400);
    // Selected, and still reachable once the selection bars appear (the phone shows its modifier bar over the canvas).
    if ((await selectionCount(page)) && (await onCanvas(page, await canvasPoint(page, x, y)))) {
      sel = [x, y];
      break;
    }
    await escape(page);
  }
  await escape(page);
  await sleep(300);
  if (sel) {
    await measure(ctx, "select", async () => {
      const p = await canvasPoint(page, sel[0], sel[1]);
      await page.press(p.x, p.y);
    });
  }
  if (sel) {
    // On touch a first drag can end up changing the selection instead of moving it: the timed move is the first
    // drag that sends an edit (at most three tries; the failed ones are dropped from the results).
    for (let i = 0; i < 3; i++) {
      const rec = await measure(ctx, "move", async () => {
        const p = await canvasPoint(page, sel[0], sel[1]);
        const q = await canvasPoint(page, sel[0] + 4, sel[1]);
        await page.drag(p.x, p.y, q.x, q.y, { steps: 20 });
      });
      if (!rec || rec.requests.some((r) => r.method === "POST")) break;
      ctx.results.pop();
      if (!(await selectionCount(page))) {
        const p = await canvasPoint(page, sel[0], sel[1]);
        await page.press(p.x, p.y);
        await sleep(400);
      }
    }
  }
  await escape(page);

  // 3. Fill Region: hover (path.fill_region_preview), then create (path.fill_region).
  const fillAt = sel ? [sel[0] + 9.5, sel[1] + 9.5] : [TARGETS[0][0] + 9.5, TARGETS[0][1] + 9.5];
  const fillTool = (await clickIf(page, "button", "Fill Region")) || (await (async () => {
    // The phone keeps it on the Modify strip of the Canvas tab, a tablet on the rail: try the More / Modify routes.
    if (phone) await clickIf(page, "button", "Modify");
    await sleep(300);
    return clickIf(page, "button", "Fill Region");
  })());
  await sleep(400);
  const fp = await canvasPoint(page, fillAt[0], fillAt[1]);
  await measure(ctx, "fill-hover", async () => {
    if (!fillTool) {
      await inPageExec(page, "path.fill_region_preview", { points: [fillAt] });
      return { via: "api" };
    }
    if (phone) await page.tap(fp.x, fp.y);
    else {
      await page.mouse("mouseMoved", fp.x - 3, fp.y - 3, { button: "none" });
      await page.mouse("mouseMoved", fp.x, fp.y, { button: "none" });
    }
  });
  await measure(ctx, "fill-create", async () => {
    if (!fillTool) {
      await inPageExec(page, "path.fill_region", { points: [fillAt] });
      return { via: "api" };
    }
    await page.press(fp.x, fp.y);
  });
  await escape(page);
  await escape(page);

  // 4. Undo and redo (the toolbar buttons; Ctrl+Z / Ctrl+Shift+Z on a desktop without them).
  const undoRedo = async (label, path, mod) => {
    if (await clickIf(page, "button[aria-label], button[title]", label)) return { via: "ui" };
    if (!phone) {
      await page.key("z", { modifiers: mod });
      return { via: "key" };
    }
    await inPage(page, "POST", path, { origin: "ui" });
    return { via: "api" };
  };
  await measure(ctx, "undo", () => undoRedo("Undo", "/api/undo", 2));
  await measure(ctx, "redo", () => undoRedo("Redo", "/api/redo", 2 | 8));

  // 5. A QR code (Barcode tool: click, the dialog's Create).
  const blank = [TARGETS[1][0] + 2, TARGETS[1][1] - 25];
  const qrTool = await clickIf(page, "button", "Barcode");
  if (qrTool) {
    await sleep(300);
    const bp = await canvasPoint(page, blank[0], blank[1]);
    await page.press(bp.x, bp.y);
    await sleep(600);
    await page.evaluate(() => {
      const ta = document.querySelector("[role=dialog] textarea, [role=dialog] input[type=text]");
      if (ta && !ta.value) {
        const set = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(ta), "value").set;
        set.call(ta, "https://example.com/openetch-perf");
        ta.dispatchEvent(new Event("input", { bubbles: true }));
      }
    });
    await sleep(200);
  }
  await measure(ctx, "qr-code", async () => {
    if (qrTool && (await clickIf(page, "[role=dialog] button", "Create"))) return { via: "ui" };
    await inPageExec(page, "shape.barcode", { kind: "qr", data: "https://example.com/openetch-perf", center: blank });
    return { via: "api" };
  });
  await escape(page);

  // 6. Text (Text tool: click, type, Esc to finish).
  const textAt = [TARGETS[1][0] - 20, TARGETS[1][1] - 25];
  const textTool = await clickIf(page, "button", "Text");
  if (textTool) {
    await sleep(300);
    const tp = await canvasPoint(page, textAt[0], textAt[1]);
    await page.press(tp.x, tp.y);
    await sleep(400);
    await page.type("Perf 123");
    await sleep(200);
  }
  await measure(ctx, "text", async () => {
    if (textTool && (await page.evaluate(() => !!document.activeElement?.matches("textarea, [contenteditable]")))) {
      await page.key("Escape");
      return { via: "ui" };
    }
    await inPageExec(page, "shape.text", { text: "Perf 123", center: textAt, height_mm: 5 });
    return { via: "api" };
  });
  await escape(page);

  // 7. Imports through the File > Import picker.
  mkdirSync(opts.out, { recursive: true });
  const svgPath = resolve(opts.out, "perf-import.svg");
  const pngPath = resolve(opts.out, "perf-import.png");
  writeFileSync(svgPath, testSvg());
  writeFileSync(pngPath, testPng());
  await measure(ctx, "import-svg", async () => {
    const t1 = Date.now();
    await pickFiles(page, [svgPath]);
    await acceptDialog(page, net, t1);
  });
  await escape(page);
  await measure(ctx, "import-image", async () => {
    const t1 = Date.now();
    await pickFiles(page, [pngPath]);
    await acceptDialog(page, net, t1);
  });
  await escape(page);

  // 8. Timeline: roll back four steps (to before the fill, QR, text and imports), then forward to the end.
  const h0 = (await docInfo(url)).history;
  const steps = (h0?.steps ?? h0?.entries ?? []).length || (h0?.marker ?? 0);
  const marker = Math.max(1, (h0?.marker ?? steps) - 4);
  await measure(ctx, "history-back4", async () => {
    if (!phone) {
      const c = await find(page, `.timeline-strip button[data-step="${marker - 1}"]`);
      if (c?.hit) {
        await page.click(c.x, c.y);
        return { via: "ui" };
      }
    }
    await inPage(page, "POST", "/api/history/goto", { marker, origin: "ui" });
    return { via: "api" };
  });
  await measure(ctx, "history-end", async () => {
    if (!phone && (await clickIf(page, "button", "Go to end"))) return { via: "ui" };
    const h = (await docInfo(url)).history;
    await inPage(page, "POST", "/api/history/goto", { marker: (h?.steps ?? h?.entries ?? []).length, origin: "ui" });
    return { via: "api" };
  });

  // 9. Design to Laser: the preview is fetched and drawn.
  await measure(ctx, "to-laser-preview", async () => {
    if (phone) {
      await phoneTab(page, "Job");
      if (!(await sheetOpen(page))) await phoneTab(page, "Job");
      return { via: "ui" };
    }
    await pressFind(page, ".workspaces button, nav[role=tablist] button", "Laser");
  }, { always: true });

  // 10. A layer setting (speed of the first output layer) in the Laser workspace: an edit, then the preview again.
  const doc = (await docInfo(url)).doc;
  const layer = doc.layers.find((l) => l.settings && l.output !== false) ?? doc.layers[0];
  await measure(ctx, "layer-speed", async () => {
    await inPageExec(page, "layer.update", { id: layer.id, settings: { speed_mm_s: (layer.settings.speed_mm_s ?? 100) + 1 } });
    return { via: "api" };
  }, { note: "layer.update from the page: the settings field's own path differs per layout" });

  // 11. Undo in the Laser workspace (the preview refetch is part of it).
  await measure(ctx, "undo-laser", () => undoRedo("Undo", "/api/undo", 2));

  // 12. Start framing (simulator), then stop it.
  await measure(ctx, "frame-start", async () => {
    if (await clickIf(page, "button", "Frame")) return { via: "ui" };
    await inPage(page, "POST", "/api/machine/frame", { origin: "ui" });
    return { via: "api" };
  });
  await sleep(1000);
  await measure(ctx, "frame-stop", async () => {
    if (await clickIf(page, "button", "Stop frame")) return { via: "ui" };
    await inPage(page, "POST", "/api/machine/frame/stop", { origin: "ui" });
    return { via: "api" };
  });

  // 13. Back to Design (the preview is dropped).
  await measure(ctx, "to-design", async () => {
    if (phone) {
      await phoneTab(page, "Canvas");
      return { via: "ui" };
    }
    await pressFind(page, ".workspaces button, nav[role=tablist] button", "Design");
  }, { always: true });

  if (page.errors.length) ctx.pageErrors = page.errors.slice(0, 20);
  await page.close();
  return ctx;
}

// --- direct timings (no browser) -----------------------------------------------------------------------------------

async function timeGet(url, path) {
  const t0 = performance.now();
  const r = await fetch(url + path);
  const ttfb = performance.now() - t0;
  const buf = Buffer.from(await r.arrayBuffer());
  const total = performance.now() - t0;
  return { status: r.status, bytes: buf.length, ttfbMs: Math.round(ttfb), totalMs: Math.round(total), buf };
}

async function direct(url, opts) {
  const out = {};
  // The import itself (fresh document each time).
  await execute(url, "doc.new");
  let t0 = performance.now();
  await api(url, `/api/document/upload?format=lbrn2&mode=replace&name=${encodeURIComponent(basename(opts.doc))}`, { method: "POST", body: readFileSync(opts.doc) });
  out.uploadMs = Math.round(performance.now() - t0);
  t0 = performance.now();
  await execute(url, "doc.new");
  out.docNewMs = Math.round(performance.now() - t0);
  await api(url, `/api/document/upload?format=lbrn2&mode=replace&name=${encodeURIComponent(basename(opts.doc))}`, { method: "POST", body: readFileSync(opts.doc) });
  out.gets = {};
  for (const path of ["/api/document", "/api/history", "/api/commands", "/api/machine", "/api/fill_regions", "/api/preview", "/api/preview"]) {
    const r = await timeGet(url, path);
    const key = out.gets[path] ? `${path} (again)` : path;
    const e = { status: r.status, bytes: r.bytes, ttfbMs: r.ttfbMs, totalMs: r.totalMs };
    if (r.bytes > 100000 && !out.gets[path]) {
      let c = performance.now();
      e.gzip6Bytes = gzipSync(r.buf, { level: 6 }).length;
      e.gzip6MsPc = Math.round(performance.now() - c);
      c = performance.now();
      e.gzip1Bytes = gzipSync(r.buf, { level: 1 }).length;
      e.gzip1MsPc = Math.round(performance.now() - c);
      c = performance.now();
      e.br4Bytes = brotliCompressSync(r.buf, { params: { [zc.BROTLI_PARAM_QUALITY]: 4 } }).length;
      e.br4MsPc = Math.round(performance.now() - c);
      try {
        const j = JSON.parse(r.buf.toString());
        e.topKeys = Object.fromEntries(Object.entries(j).map(([k, v]) => [k, JSON.stringify(v).length]));
      } catch {}
    }
    out.gets[key] = e;
  }
  // A no-op-ish edit and its undo, from outside the browser (server time of an edit).
  t0 = performance.now();
  const d = await api(url, "/api/document");
  const id = d.objects[d.objects.length - 1].id;
  t0 = performance.now();
  await execute(url, "object.move", { ids: [id], from_point: [0, 0], to_point: [0.5, 0] }).catch(() => null);
  out.editMs = Math.round(performance.now() - t0);
  t0 = performance.now();
  await api(url, "/api/undo", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ origin: "ui" }) });
  out.undoMs = Math.round(performance.now() - t0);
  return out;
}

// --- report --------------------------------------------------------------------------------------------------------

function markdown(report) {
  const net = report.throttle ? `, network throttled to ${report.throttle.mbps} Mbit/s with ${report.throttle.latencyMs} ms latency` : "";
  const L = [`# Performance profile: ${report.label}`, "", `Engine ${report.url}, document ${report.docName}, ${report.when}${net}.`, ""];
  if (report.direct) {
    L.push("## Engine GETs (no browser)", "", "| path | bytes | TTFB ms | total ms | gzip-1 | brotli-4 |", "|---|---:|---:|---:|---:|---:|");
    for (const [p, g] of Object.entries(report.direct.gets))
      L.push(`| ${p} | ${g.bytes} | ${g.ttfbMs} | ${g.totalMs} | ${g.gzip1Bytes ?? ""} | ${g.br4Bytes ?? ""} |`);
    L.push("", `Upload ${report.direct.uploadMs} ms, edit ${report.direct.editMs} ms, undo ${report.direct.undoMs} ms (round trips).`, "");
  }
  for (const dev of report.devices) {
    L.push(`## ${dev.device}`, "", "| action | via | wall ms | frame ms | requests | MB | long tasks ms (max) | JSON.parse ms | biggest request |", "|---|---|---:|---:|---:|---:|---:|---:|---|");
    for (const a of dev.actions) {
      const big = [...a.requests].sort((x, y) => (y.bytes ?? 0) - (x.bytes ?? 0))[0];
      const bigTxt = big ? `${big.method} ${big.path.split("?")[0]} ${((big.bytes ?? 0) / 1e6).toFixed(2)} MB, server ${big.serverMs} ms, transfer ${big.transferMs} ms` : "";
      L.push(
        `| ${a.name} | ${a.via} | ${a.wallMs}${a.timedOut ? " (timeout)" : ""}${a.error ? " (error)" : ""} | ${a.frameMs ?? ""} | ${a.requests.length} | ${(a.bytes / 1e6).toFixed(2)} | ${a.longTasks.totalMs} (${a.longTasks.maxMs}) | ${a.jsonParse.reduce((s, p) => s + p.ms, 0)} | ${bigTxt} |`,
      );
    }
    L.push("");
  }
  return L.join("\n");
}

async function main() {
  const opts = args(process.argv.slice(2));
  if (opts.help || !opts.url || !opts.doc) {
    console.log("usage: node e2e/perf.mjs --url http://127.0.0.1:7931 --doc heavy.lbrn2 [--label name] [--out dir] [--devices ids] [--only actions] [--quiet ms] [--timeout ms] [--no-direct] [--throttle-mbps n] [--latency-ms n]");
    process.exit(opts.help ? 0 : 2);
  }
  opts.label ??= new URL(opts.url).hostname === "127.0.0.1" ? "local" : "remote";
  opts.out = resolve(opts.out ?? join(here, "..", "..", "target", "perf", opts.label));
  mkdirSync(opts.out, { recursive: true });
  const throttle = opts.throttleMbps > 0 ? { mbps: opts.throttleMbps, latencyMs: opts.latencyMs ?? 5 } : null;
  const report = { label: opts.label, url: opts.url, docName: basename(opts.doc), when: new Date().toISOString(), throttle, devices: [] };
  if (!opts.noDirect && !opts.only) {
    console.log("direct timings...");
    report.direct = await direct(opts.url, opts);
    for (const [p, g] of Object.entries(report.direct.gets)) console.log(`  GET ${p.padEnd(24)} ${String(g.bytes).padStart(9)} B  ttfb ${g.ttfbMs} ms  total ${g.totalMs} ms`);
  }
  const browser = await launch({ port: opts.port, headless: !opts.headful });
  try {
    for (const d of pickDevices(opts.devices)) {
      console.log(`${d.id}:`);
      const ctx = await walk(browser, d, opts).catch((e) => ({ device: d.id, results: [], error: String(e?.stack ?? e) }));
      if (ctx.error) console.log(`  FAILED: ${ctx.error}`);
      report.devices.push({ device: d.id, name: d.name, width: d.width, height: d.height, dpr: d.dpr, touch: d.touch, actions: ctx.results, error: ctx.error, pageErrors: ctx.pageErrors });
    }
  } finally {
    await browser.close();
  }
  writeFileSync(join(opts.out, "perf.json"), JSON.stringify(report, null, 1));
  writeFileSync(join(opts.out, "perf.md"), markdown(report));
  console.log(`report: ${join(opts.out, "perf.md")}`);
}

main().then(
  // Exit at once: a closing browser's pipes or a keep-alive socket would otherwise hold node open for minutes.
  () => process.exit(0),
  (e) => {
    console.error(e);
    process.exit(1);
  },
);
