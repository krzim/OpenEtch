#!/usr/bin/env node
// The screens where the feature packages meet (ARCHITECTURE.md "Links between the production, jig, materials, text
// and ledger features"), on emulated devices: a jig whose pocket 2 is marked empty, the cockpit's slot chips showing
// the pocket numbers, the pre-flight warning from the contact sheet's check of the next row (a serial number too long
// for its tag) and Stop at End, and the Job log's run detail with each value's pocket and the jig's pockets.
//
//   node e2e/links.mjs --url http://127.0.0.1:7966 [--devices ipad-air-landscape,iphone15-portrait,macbook-air-1470]
//
// It needs an engine with the SIMULATOR and `--approval click` (it starts a run from the page and replaces the
// engine's document). Never a real machine. Output under target/ux-evidence/links/.

import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, sleep } from "./cdp.mjs";
import { pickDevices } from "./devices.mjs";
import { runCheck } from "./overlap.mjs";
import { api, applySafeArea, execute, find, phoneTab, sheetTo } from "./scenarios.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));

function args(argv) {
  const a = {};
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--url") a.url = v();
    else if (k === "--out") a.out = v();
    else if (k === "--devices") a.devices = v();
    else if (k === "--port") a.port = Number(v());
  }
  return a;
}

async function must(url, id, params) {
  const r = await execute(url, id, params);
  if (!r || r.ok !== true) throw new Error(`${id}: ${JSON.stringify(r)}`);
  return r.result;
}

/** A 1 x 3 tag jig with a serial number placed in every pocket, pocket 2 empty, and a tag whose number is too long. */
async function design(url) {
  await must(url, "doc.new", {});
  const jig = (await must(url, "jig.create", { rows: 1, cols: 3, pitch_x_mm: 32, name: "Tags", pocket: { kind: "circle", diameter_mm: 26 }, center: [0, 10] })).jig.id;
  const font = { family: "Hershey Sans", kind: "stroke" };
  const sn = (await must(url, "shape.text", { text: "'SN '0ddd", font, height_mm: 4, variable: { mode: "serial" }, name: "SN" })).created[0];
  const disc = (await must(url, "shape.ellipse", { rx_mm: 11, ry_mm: 11, name: "Rim" })).created[0];
  await must(url, "jig.place", { jig, ids: [sn, disc] });
  await must(url, "shape.rect", { width_mm: 14, height_mm: 6, center: [0, -25], name: "Tag" });
  await must(url, "shape.text", { text: "'LOT '0ddddd", font, height_mm: 4, variable: { mode: "serial", offset: 0 }, center: [0, -25], name: "Lot" });
  await must(url, "var.set_serial", { start: 1, end: 100, current: 41, advance_by: 3, auto_advance: true, empty_slots: [1] });
}

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

const a = args(process.argv.slice(2));
if (!a.url) {
  console.error("usage: node e2e/links.mjs --url http://127.0.0.1:7966 [--devices ...] [--out dir]");
  process.exit(2);
}
const out = resolve(a.out ?? join(here, "..", "..", "target", "ux-evidence", "links"));
const devices = pickDevices(a.devices ?? "ipad-air-landscape,iphone15-portrait,macbook-air-1470");
const browser = await launch({ port: a.port ?? 9345 });
const report = [];
try {
  // One burn for the ledgers, from a desktop page (click mode needs the browser's Start).
  await design(a.url);
  {
    const page = await browser.newPage();
    await page.goto(a.url + "/", { waitFor: ".btn-stop", timeout: 20000 });
    await sleep(600);
    await burnFromPage(page, a.url);
    await page.close();
  }
  for (const d of devices) {
    const theme = d.themes[0];
    const dir = join(out, `${d.id}-${theme}`);
    mkdirSync(dir, { recursive: true });
    const page = await browser.newPage();
    await page.setDevice(d, { theme });
    let n = 0;
    const shot = async (name) => {
      await page.settle(400);
      const file = join(dir, `${String(n++).padStart(2, "0")}-${name}.png`);
      await page.screenshot(file);
      const c = await runCheck(page, { touch: d.touch, safe: d.safe }).catch((e) => ({ error: String(e), problems: [] }));
      const serious = (c.problems ?? []).filter((p) => p.severity !== "low");
      report.push({ device: d.id, step: name, file, problems: serious });
      console.log(`${d.id} ${name}: ${serious.length} problem(s)${serious.length ? " " + JSON.stringify(serious.slice(0, 3)) : ""}`);
    };
    const press = async (sel, text) => {
      const el = await find(page, sel, text);
      if (!el) throw new Error(`no ${sel} ${text ?? ""}`);
      await page.press(el.x, el.y);
      await sleep(500);
    };
    const closeOffer = async () => {
      for (let i = 0; i < 12; i++) {
        await sleep(250);
        const close = await find(page, ".modal-foot button, [role=dialog] button", "Close");
        if (close) {
          await page.press(close.x, close.y);
          await sleep(400);
          return;
        }
      }
    };
    try {
      await page.goto(`${a.url}/#/cockpit`, { waitFor: ".cockpit", timeout: 20000 });
      await applySafeArea(page, d.safe);
      await closeOffer();
      await shot("cockpit-pocket-slots");
      await press(".cockpit-exit");
      await sleep(600);
      // The Laser workspace: the pre-flight warnings next to Start.
      const phone = d.kind === "phone";
      if (phone) {
        await phoneTab(page, "Job");
        await sheetTo(page, "full").catch(() => {});
      } else {
        await press("button", "Laser");
        if (d.kind === "tablet" && !(await find(page, "button", "Job log…"))) await press(".dock-rail-btn", "Laser");
      }
      await sleep(800);
      await page.evaluate(() => document.querySelector(".pf-notes, .pf-list")?.scrollIntoView({ block: "center" }));
      await shot("laser-preflight");
      // The Job log's newest run: each value with its pocket, the jig's pockets.
      await press("button", "Job log…");
      await page.waitFor(".led-body", { timeout: 8000 });
      await sleep(1200);
      if (phone) await press(".led-row");
      await sleep(800);
      await page.evaluate(() => {
        const s = [...document.querySelectorAll(".led-section h4")].find((h) => /Variable text|Jigs/.test(h.textContent ?? ""));
        s?.scrollIntoView({ block: "start" });
      });
      await shot("job-log-pockets");
    } catch (e) {
      console.log(`${d.id}: ${e.message}`);
      await shot("error").catch(() => undefined);
    }
    await page.close();
  }
} finally {
  writeFileSync(join(out, "report.json"), JSON.stringify(report, null, 2));
  await browser.close();
}
