#!/usr/bin/env node
// The production screens on emulated devices (web/e2e/README.md): the cockpit idle, waiting at a layer pause and after
// Continue, the foot switch cycle framing ("press the foot switch"), the Laser workspace's Production window, the serial
// ledger, the Preferences sections (notifications, maintenance, extractor) and the design lock refusing an edit, with a
// screenshot and the layout check (overlap.mjs) after each step.
//
//   node e2e/production.mjs --url http://127.0.0.1:7961 [--devices ipad-air-landscape,iphone15-portrait] [--out dir]
//
// It needs an engine with the SIMULATOR and `--approval click` (it presses Start, the simulated foot switch and STOP,
// and replaces the engine's document), started with the B6 fixture profile (`--profile fixtures/b6-mopa-60w.toml`:
// the foot switch is input 14, active low). Never a real machine.

import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, sleep } from "./cdp.mjs";
import { pickDevices } from "./devices.mjs";
import { runCheck } from "./overlap.mjs";
import { api, applySafeArea, execute, find } from "./scenarios.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));

function args(argv) {
  const a = {};
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--url") a.url = v();
    else if (k === "--out") a.out = v();
    else if (k === "--devices") a.devices = v();
    else if (k === "--theme") a.theme = v();
    else if (k === "--cdp-port") a.port = Number(v());
    else if (k === "--headful") a.headful = true;
  }
  return a;
}

const json = (body) => ({ method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
const put = (body) => ({ method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

/** Six discs, each grouped with a serial text, on the first layer; a frost layer that pauses before it. */
async function setup(url) {
  await api(url, "/api/machine/stop", json({}));
  // The active device's foot switch and its level (a LightBurn device may be active high, the B6 fixture is low).
  const fs = (await api(url, "/api/machine")).approval?.foot_switch;
  if (!fs) throw new Error("the engine has no foot switch: start it with --approval click and a Start Marking input");
  const bit = 1 << fs.input_bit;
  const pedal = { released: fs.active_low ? bit : 0, pressed: fs.active_low ? 0 : bit };
  await api(url, "/api/sim/inputs", json({ bits: pedal.released }));
  await api(url, "/api/production/lock", json({ on: false, force: true, origin: "ui" }));
  await execute(url, "doc.new");
  const doc = await api(url, "/api/document");
  const first = doc.layers[0].id;
  for (let k = 0; k < 6; k++) {
    const x = -50 + (k % 3) * 50;
    const y = k < 3 ? 25 : -25;
    const disc = (await execute(url, "shape.ellipse", { center: [x, y], rx_mm: 15, ry_mm: 15, layer: first })).result.created[0];
    const text = (
      await execute(url, "shape.text", {
        text: "'SN '0ddd",
        font: { family: "Hershey Sans", kind: "stroke" },
        height_mm: 5,
        center: [x, y],
        layer: first,
        variable: { mode: "serial", offset: k },
      })
    ).result.created[0];
    await execute(url, "group.create", { ids: [disc, text], name: `Pocket ${k + 1}` });
  }
  const frost = (await execute(url, "layer.add", { name: "Frost" })).result.id;
  await execute(url, "shape.rect", { width_mm: 100, height_mm: 8, center: [0, 0], layer: frost });
  await execute(url, "layer.update", { id: frost, settings: { options: { operator_pause: true } } });
  await execute(url, "var.set_serial", { start: 1, end: 60, current: 41, advance_by: 6, auto_advance: true, empty_slots: [3] });
  await api(url, "/api/job/settings", put({ stop_at: 12 }));
  // Each device burns once: a reused engine's counter would reach Stop At and refuse the next Start.
  await api(url, "/api/job/count/reset", json({}));
  await api(url, "/api/production/maintenance", put({ reminders: [{ name: "Clean the lens", every_hours: 40 }, { name: "Check the jig", every_runs: 1 }], origin: "ui" }));
  return pedal;
}

async function main() {
  const a = args(process.argv.slice(2));
  if (!a.url) {
    console.error("usage: node e2e/production.mjs --url http://127.0.0.1:7961 [--devices ...] [--out dir]");
    process.exit(2);
  }
  const out = resolve(a.out ?? join(here, "..", "..", "target", "ux-evidence", "production"));
  const devices = pickDevices(a.devices ?? "ipad-air-landscape,ipad-air-portrait,iphone15-portrait,macbook-air-1470");
  const browser = await launch({ port: a.port ?? 9341, headless: !a.headful });
  const report = [];
  try {
    for (const d of devices) {
      const theme = a.theme ?? d.themes[0];
      const dir = join(out, `${d.id}-${theme}`);
      mkdirSync(dir, { recursive: true });
      const pedal = await setup(a.url);
      const page = await browser.newPage();
      await page.setDevice(d, { theme });
      let n = 0;
      const shot = async (name) => {
        await page.settle(300);
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
        await sleep(400);
      };
      try {
        await page.goto(`${a.url}/#/cockpit`, { waitFor: ".cockpit", timeout: 20000 });
        await applySafeArea(page, d.safe);
        // The "Use as an app" offer of the first touch visit comes a moment after the page.
        for (let i = 0; i < 12; i++) {
          await sleep(250);
          const close = await find(page, ".modal-foot button, [role=dialog] button", "Close");
          if (close) {
            await page.press(close.x, close.y);
            await sleep(400);
            break;
          }
        }
        await shot("cockpit-idle");
        // The design is locked while the cockpit is open.
        const refused = await api(a.url, "/api/execute", json({ id: "shape.rect", params: { width_mm: 2, height_mm: 2 }, origin: "ui" }));
        report.push({ device: d.id, step: "lock-refuses-edits", ok: refused.ok === false && /locked/.test(refused.error ?? "") });
        await press(".btn-start");
        for (let i = 0; i < 100 && !(await page.evaluate(() => !!document.querySelector(".cp-continue"))); i++) await sleep(100);
        await shot("cockpit-layer-pause");
        await press(".cp-continue");
        for (let i = 0; i < 100; i++) {
          const p = await api(a.url, "/api/production");
          if (!p.job.active && p.ledger.length > 0) break;
          await sleep(100);
        }
        await sleep(800);
        await shot("cockpit-after-run");
        // The foot switch, released then pressed: the cycle frames the job.
        await api(a.url, "/api/sim/inputs", json({ bits: pedal.released }));
        await sleep(300);
        await api(a.url, "/api/sim/inputs", json({ bits: pedal.pressed }));
        await sleep(1200);
        await shot("cockpit-foot-ready");
        await press(".cockpit .btn-stop");
        await api(a.url, "/api/sim/inputs", json({ bits: pedal.released }));
        await sleep(500);
        await press(".cockpit-exit");
        await sleep(800);
        // The Laser workspace: the Production window (on the phone, in the Job sheet).
        if (d.kind === "phone") {
          await press(".phone-tabs button, .bottom-nav button, [role=tab]", "Job");
        } else {
          const laser = await find(page, "button, [role=tab]", "Laser");
          if (laser) await page.press(laser.x, laser.y);
        }
        await sleep(1000);
        await page.evaluate(() => document.querySelector(".prod-dock")?.scrollIntoView({ block: "center" }));
        await shot("laser-production-window");
        await page.evaluate(() => document.querySelector(".prod-chip")?.scrollIntoView());
        await shot("maintenance-chip");
        await page.evaluate(() => {
          const b = [...document.querySelectorAll(".prod-dock button")].find((x) => /ledger/i.test(x.textContent ?? ""));
          b?.click();
        });
        await sleep(800);
        await shot("serial-ledger");
        await page.key("Escape");
        await sleep(300);
      } catch (e) {
        console.error(`${d.id}: ${e.message ?? e}`);
        report.push({ device: d.id, error: String(e.message ?? e) });
        await page.screenshot(join(dir, `${String(n++).padStart(2, "0")}-error.png`)).catch(() => undefined);
      }
      await page.close();
    }
  } finally {
    await browser.close();
    writeFileSync(join(out, "report.json"), JSON.stringify(report, null, 2));
    await api(a.url, "/api/machine/stop", json({})).catch(() => undefined);
  }
}

await main();
