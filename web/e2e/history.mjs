// The job ledger and the 3D relief preview on emulated devices (phone, iPad, MacBook): runs a few jobs on a SIMULATOR
// engine (never the laser: it burns), marks them, then opens the Job log window (the list, a run, Storage) and the 3D
// relief preview of a 3D Sliced coin, with a screenshot and the layout check (overlap.mjs) after each step.
//
//   openetch --config-dir <tmp>/cfg --projects-dir <tmp>/projects --listen 127.0.0.1:7965 --approval click --web-dir web/dist
//   node e2e/history.mjs --url http://127.0.0.1:7965 [--doc some.lbrn2] [--devices iphone15-portrait,ipad-air-landscape,macbook-air-1470]
//
// Screenshots and report.json go under target/ux-evidence/history-<time>/ (git ignores target/). Look at them.

import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync, crc32 } from "node:zlib";
import { launch, sleep } from "./cdp.mjs";
import { pickDevices } from "./devices.mjs";
import { runCheck } from "./overlap.mjs";
import { api, applySafeArea, execute, find, loadHeavy, phoneTab, pressFind, sheetTo } from "./scenarios.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));

function args(argv) {
  const a = {};
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--url") a.url = v();
    else if (k === "--out") a.out = v();
    else if (k === "--devices") a.devices = v();
    else if (k === "--doc") a.doc = v();
    else if (k === "--cdp-port") a.port = Number(v());
  }
  return a;
}

/** An 8-bit grey PNG of `w` x `h` from `f(x, y)` in 0..255. */
function greyPng(w, h, f) {
  const raw = Buffer.alloc((w + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (w + 1)] = 0;
    for (let x = 0; x < w; x++) raw[y * (w + 1) + 1 + x] = Math.max(0, Math.min(255, Math.round(f(x, y))));
  }
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(td) >>> 0);
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 0;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

/** A coin's depth map: a raised rim, a dished field and a five-pointed star in relief (darker is deeper). */
function coinDepthMap(n = 160) {
  return greyPng(n, n, (x, y) => {
    const u = (x + 0.5) / n - 0.5;
    const v = (y + 0.5) / n - 0.5;
    const r = Math.hypot(u, v) * 2;
    if (r > 1) return 255;
    if (r > 0.88) return 235;
    const a = Math.atan2(v, u) + Math.PI / 2;
    const star = 0.34 + 0.2 * Math.cos(5 * a);
    const dish = 150 - 60 * (1 - r * r);
    return r < star ? 210 - 90 * (r / star) : dish;
  });
}

const sh = () => ({ "content-type": "application/json" });

/** Start the active document from inside a page (click mode needs the browser's Start), then wait for the end. */
async function burnFromPage(page, url) {
  const rev = (await api(url, "/api/document")).revision;
  const r = await page.evaluate(
    async (revision) => {
      const res = await fetch("/api/job/start", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ revision, origin: "ui" }) });
      return { status: res.status, body: await res.text() };
    },
    rev,
  );
  if (r.status !== 200) throw new Error(`start: ${r.status} ${r.body}`);
  for (let i = 0; i < 600; i++) {
    const m = await api(url, "/api/machine");
    if (!m.running && !(m.runs ?? []).some((x) => ["approved", "awaiting_confirm", "running"].includes(x.state))) return;
    await sleep(250);
  }
  throw new Error("the run did not end");
}

async function setup(browser, url, doc) {
  const page = await browser.newPage();
  await page.goto(url + "/", { waitFor: ".btn-stop", timeout: 20000 });
  await sleep(800);
  // 1. The owner's job (or a generated one) twice, then a variable-text tag.
  await loadHeavy(url, doc);
  await burnFromPage(page, url);
  await sleep(1100);
  await burnFromPage(page, url);
  await sleep(1100);
  await execute(url, "doc.new", { name: "Name tag" });
  await execute(url, "layer.update", { id: "layer-1", name: "Blue", settings: { mode: "fill", power_pct: 38, speed_mm_s: 1200, frequency_khz: 40, pulse_width_ns: 200 } });
  await execute(url, "shape.rect", { width_mm: 40, height_mm: 12 });
  await execute(url, "shape.text", { text: "0ddd", font: { family: "Hershey Sans", kind: "stroke" }, height_mm: 6, variable: { mode: "serial" }, name: "Serial" });
  await execute(url, "var.set_serial", { auto_advance: true, current: 41 });
  await burnFromPage(page, url);
  await sleep(1100);
  // 2. A 3D Sliced coin (12 slices).
  await execute(url, "doc.new", { name: "Star coin" });
  await execute(url, "art.import_image", { data_base64: coinDepthMap().toString("base64"), mime: "image/png", width_mm: 30, name: "Coin depth map" });
  await execute(url, "layer.update", { id: "layer-1", name: "Relief", settings: { power_pct: 60, speed_mm_s: 800, frequency_khz: 40, passes: 12, image: { dither: "slice3d", interval_mm: 0.1 } } });
  await burnFromPage(page, url);
  await page.close();
  // 3. Verdicts.
  const rows = (await api(url, "/api/job-log?limit=10")).entries;
  const mark = (id, verdict, note) => api(url, `/api/job-log/${id}/mark`, { method: "POST", headers: sh(), body: JSON.stringify({ verdict, note, origin: "ui" }) });
  if (rows[1]) await mark(rows[1].id, "ok", "Colour even, good blue");
  if (rows[2]) await mark(rows[2].id, "scrap", "Second pass offset 0.2 mm: jig moved");
  return rows.length;
}

async function runDevice(browser, d, url, out) {
  const dir = join(out, d.id);
  mkdirSync(dir, { recursive: true });
  const page = await browser.newPage();
  const results = [];
  const check = async (name, error) => {
    const shot = join(dir, `${String(results.length).padStart(2, "0")}-${name}.png`);
    await page.settle(300);
    await page.screenshot(shot);
    const c = await runCheck(page, { touch: d.touch, safe: d.safe }).catch((e) => ({ error: String(e), problems: [] }));
    results.push({ step: name, shot, error, problems: (c.problems ?? []).filter((p) => p.type !== "small"), small: (c.problems ?? []).find((p) => p.type === "small")?.count ?? 0 });
  };
  const step = async (name, fn) => {
    let error;
    try {
      await fn();
    } catch (e) {
      error = String(e.message ?? e);
    }
    await sleep(300);
    await check(name, error);
  };
  try {
    await page.setDevice(d, { theme: d.themes[0] });
    await page.goto(url + "/", { waitFor: ".btn-stop", timeout: 20000 });
    await applySafeArea(page, d.safe);
    // The "Use as an app" offer comes up a moment after the first touch visit.
    for (let i = 0; i < 8; i++) {
      await sleep(500);
      const close = await find(page, ".modal-foot button", "Close");
      if (close) {
        await page.press(close.x, close.y);
        await sleep(400);
        break;
      }
    }
    const phone = d.kind === "phone";
    const toLaser = async () => {
      if (phone) {
        await phoneTab(page, "Job");
        await sleep(600);
        await sheetTo(page, "half");
      } else {
        await pressFind(page, "button, [role=tab]", "Laser");
      }
      await sleep(1500);
    };
    await step("laser-preview", toLaser);
    await step("relief", async () => {
      if (phone) await sheetTo(page, "peek").catch(() => {});
      await sleep(400);
      await pressFind(page, "button", "3D relief");
      await page.waitFor(".rel-canvas", { timeout: 8000 });
      await sleep(2500);
    });
    await step("relief-half", async () => {
      await page.evaluate(() => {
        const r = document.querySelector(".rel-player input[type=range]");
        const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
        set.call(r, String(Number(r.max) * 0.45));
        r.dispatchEvent(new Event("input", { bubbles: true }));
      });
      await sleep(1500);
    });
    await step("relief-low", async () => {
      await pressFind(page, ".rel-views button", "Low");
      await sleep(800);
    });
    await step("relief-closed", async () => {
      await page.key("Escape");
      await sleep(500);
    });
    await step("job-log", async () => {
      if (phone) await sheetTo(page, "full").catch(() => {});
      await sleep(400);
      if (d.kind === "tablet" && !(await find(page, "button", "Job log…"))) {
        await pressFind(page, ".dock-rail-btn", "Laser");
        await sleep(800);
      }
      await pressFind(page, "button", "Job log…");
      await page.waitFor(".led-body", { timeout: 8000 });
      await sleep(1500);
    });
    if (phone) {
      await step("job-log-run", async () => {
        await pressFind(page, ".led-row", null);
        await sleep(1200);
      });
      await step("job-log-run-scrolled", async () => {
        await page.evaluate(() => document.querySelector(".modal-body")?.scrollBy(0, 600));
        await sleep(600);
      });
      await step("job-log-back", async () => {
        await pressFind(page, ".led-back", null);
        await sleep(600);
      });
    } else {
      await step("job-log-scrap", async () => {
        const rows = await page.evaluate(() => [...document.querySelectorAll(".led-row")].findIndex((r) => r.textContent.includes("Scrap")));
        await page.evaluate((i) => document.querySelectorAll(".led-row")[i]?.click(), Math.max(0, rows));
        await sleep(1200);
      });
    }
    await step("job-log-search", async () => {
      const c = await find(page, ".led-search input", null);
      await page.press(c.x, c.y);
      await page.type("blue");
      await sleep(1500);
      await page.evaluate(() => document.activeElement?.blur?.());
    });
    await step("storage", async () => {
      await pressFind(page, "button", "Storage…");
      await sleep(1200);
    });
    await step("storage-closed", async () => {
      await page.key("Escape");
      await sleep(400);
    });
    // Re-run as burned: a new tab with the job as it burned and a pending run in the Laser window (nothing starts).
    await step("rerun", async () => {
      if (phone) await sheetTo(page, "full").catch(() => {});
      if (d.kind === "tablet" && !(await find(page, "button", "Job log…"))) await pressFind(page, ".dock-rail-btn", "Laser");
      await sleep(400);
      await pressFind(page, "button", "Job log…");
      await page.waitFor(".led-body", { timeout: 8000 });
      await sleep(800);
      if (phone) await pressFind(page, ".led-row", null);
      await sleep(600);
      await pressFind(page, "button", "Re-run as burned");
      await sleep(2500);
      const m = await api(url, "/api/machine");
      if (!(m.pending_runs ?? []).length) throw new Error("no pending run after Re-run");
      if (m.running) throw new Error("Re-run started a burn");
    });
    // Leave nothing waiting for the next device.
    await api(url, "/api/machine/stop", { method: "POST", headers: sh(), body: "{}" });
    if (page.errors.length) results.push({ step: "console-errors", errors: page.errors.slice(0, 20), problems: [] });
  } finally {
    await page.close();
  }
  return { device: d.id, results };
}

async function main() {
  const a = args(process.argv.slice(2));
  if (!a.url) {
    console.log("usage: node e2e/history.mjs --url http://127.0.0.1:7965 [--doc file.lbrn2] [--devices ...] [--out dir]");
    process.exitCode = 2;
    return;
  }
  const url = a.url.replace(/\/$/, "");
  const run = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const out = resolve(a.out ?? join(here, "..", "..", "target", "ux-evidence", `history-${run}`));
  mkdirSync(out, { recursive: true });
  const browser = await launch({ port: a.port ?? 9334 });
  const runs = [];
  try {
    const n = await setup(browser, url, a.doc);
    console.log(`setup: ${n} ledger entries`);
    for (const d of pickDevices(a.devices ?? "iphone15-portrait,ipad-air-landscape,macbook-air-1470")) {
      const r = await runDevice(browser, d, url, out);
      runs.push(r);
      const probs = r.results.reduce((k, s) => k + (s.problems ?? []).length, 0);
      console.log(`${d.id}: ${r.results.length} steps, ${probs} problems, ${r.results.filter((s) => s.error).length} step errors`);
      writeFileSync(join(out, "report.json"), JSON.stringify(runs, null, 1));
    }
  } finally {
    await browser.close();
  }
  console.log(`out: ${out}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
