#!/usr/bin/env node
// The text and template screens on the device harness (web/e2e/README.md): example projects, the fill-in form, the
// template fields dialog, text auto-fit, the font picker's favourites and samples, and the contact sheet, each with a
// screenshot and the layout check (overlap.mjs) on a phone, an iPad and a MacBook.
//
//   node e2e/templates.mjs --url http://127.0.0.1:7964 [--devices iphone15-portrait,ipad-air-landscape,macbook-air-1470]
//                          [--out dir] [--theme dark|light] [--cdp-port 9334]
//
// It needs an engine with the SIMULATOR on a spare port: it replaces that engine's document (and frames nothing).

import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, sleep } from "./cdp.mjs";
import { pickDevices } from "./devices.mjs";
import { runCheck } from "./overlap.mjs";
import { api, canvasPoint, execute, find, menuPath, pressFind } from "./scenarios.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));

function args(argv) {
  const a = { devices: "iphone15-portrait,ipad-air-landscape,macbook-air-1470", theme: "dark" };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--url") a.url = v();
    else if (k === "--out") a.out = v();
    else if (k === "--devices") a.devices = v();
    else if (k === "--theme") a.theme = v();
    else if (k === "--cdp-port") a.port = Number(v());
  }
  return a;
}

async function closeDialogs(page) {
  for (let i = 0; i < 3; i++) {
    const open = await page.evaluate(() => !!document.querySelector(".modal-backdrop, .oe-dialog"));
    if (!open) return;
    await page.key("Escape");
    await sleep(300);
  }
}

async function runDevice(browser, d, theme, url, out) {
  const dir = join(out, `${d.id}-${theme}`);
  mkdirSync(dir, { recursive: true });
  const page = await browser.newPage();
  const results = [];
  const phone = d.kind === "phone";
  const desktop = d.kind === "desktop";
  try {
    await page.setDevice(d, { theme });
    await execute(url, "doc.new", { example: "two-tone-coin" });
    await page.goto(url + "/", { waitFor: ".btn-stop", timeout: 20000 });
    await sleep(1800);
    const check = async (name, error) => {
      const shot = join(dir, `${String(results.length).padStart(2, "0")}-${name}.png`);
      await page.settle(300);
      await page.screenshot(shot);
      const c = await runCheck(page, { touch: d.touch, safe: d.safe }).catch((e) => ({ error: String(e), problems: [] }));
      const problems = (c.problems ?? []).filter((p) => p.type !== "small");
      results.push({ step: name, shot, error: error ?? c.error, problems });
    };
    const step = async (name, fn) => {
      let error;
      try {
        await fn();
      } catch (e) {
        error = String(e.message ?? e);
      }
      await sleep(400);
      await check(name, error);
    };

    // The example opens with its walkthrough (the project notes, shown on open).
    await check("example-notes");
    await closeDialogs(page);
    // The "Use as an app" offer on a first touch visit.
    const offer = await find(page, ".modal-foot button, [role=dialog] button", "Close");
    if (offer) await page.press(offer.x, offer.y);
    await closeDialogs(page);
    await step("canvas-with-fill-in", async () => {});
    await step("fill-in-form", async () => {
      await pressFind(page, "button.tpl-hud", "Fill in");
    });
    await step("fill-in-filled", async () => {
      const inp = await find(page, "#tpl-name");
      if (!inp) throw new Error("no name field");
      await page.press(inp.x, inp.y);
      await page.evaluate(() => {
        const el = document.querySelector("#tpl-name");
        el.select?.();
      });
      await page.type("Grace Hopper");
      await pressFind(page, ".tpl-choice", "Blue");
      await pressFind(page, ".tpl-choice", "Script");
    });
    await step("fill-in-applied", async () => {
      await pressFind(page, ".oe-dialog .btn-primary, .modal-foot .btn-primary", "Apply");
      await sleep(800);
      await closeDialogs(page);
    });
    if (desktop) {
      await step("text-fit-properties", async () => {
        // Press on a stroke of the name (its outline's first point): a press inside the rim selects the rim.
        const doc = await api(url, "/api/document");
        const o = doc.objects.find((x) => x.name === "Name");
        const first = o.geometry.outline.find((el) => el && el.MoveTo)?.MoveTo ?? { x: 0, y: 0 };
        const [a, b, c, dd, e, f] = o.transform;
        const p = await canvasPoint(page, a * first.x + c * first.y + e, b * first.x + dd * first.y + f);
        await page.press(p.x, p.y);
        await sleep(500);
        await page.evaluate(() => {
          const h = [...document.querySelectorAll(".tx-section h4")].find((e) => e.textContent?.trim() === "Fit");
          h?.scrollIntoView({ block: "start" });
        });
      });
      await step("font-picker-samples", async () => {
        await page.evaluate(() => {
          const h = [...document.querySelectorAll(".tx-section .field-label")].find((e) => e.textContent?.trim() === "Font");
          h?.scrollIntoView({ block: "center" });
        });
        await sleep(200);
        await pressFind(page, ".tx-section .tx-font-btn");
        await sleep(1500);
        const star = await find(page, ".tx-fav", null);
        if (star) await page.press(star.x, star.y);
        await sleep(800);
      });
      await page.key("Escape");
      await sleep(300);
      await step("template-fields", async () => {
        await menuPath(page, ["File", "Project", "Template fields…"]);
      });
      await closeDialogs(page);
    }

    // Every example as it opens (the walkthrough closed).
    if (desktop)
      for (const id of ["text-medallion", "dither-sample", "palette-coin", "serial-tags"]) {
        await execute(url, "doc.new", { example: id });
        await sleep(1500);
        await closeDialogs(page);
        await step(`example-${id}`, async () => {});
      }

    // The tag set: the contact sheet of every jig load.
    await execute(url, "doc.new", { example: "serial-tags" });
    await sleep(1500);
    await closeDialogs(page);
    await step("contact-sheet", async () => {
      await page.evaluate(() => window.dispatchEvent(new CustomEvent("openetch:contact-sheet")));
      await sleep(1500);
    });
    await closeDialogs(page);

    // Examples from the empty canvas.
    await execute(url, "doc.new");
    await sleep(1200);
    await closeDialogs(page);
    await step("empty-card-examples", async () => {});
    await step("examples-dialog", async () => {
      await pressFind(page, ".empty-card button", "Examples");
      await sleep(800);
    });
    await closeDialogs(page);
    if (page.errors.length) results.push({ step: "console-errors", errors: page.errors.slice(0, 20), problems: [] });
  } finally {
    await page.close();
  }
  return { device: d.id, theme, results };
}

async function main() {
  const a = args(process.argv.slice(2));
  if (!a.url) {
    console.log("usage: node e2e/templates.mjs --url http://127.0.0.1:<port> [--devices ...] [--out dir]");
    process.exitCode = 2;
    return;
  }
  const url = a.url.replace(/\/$/, "");
  const out = resolve(a.out ?? join(here, "..", "..", "target", "ux-evidence", "templates"));
  mkdirSync(out, { recursive: true });
  const browser = await launch({ port: a.port ?? 9334, headless: true });
  const runs = [];
  try {
    for (const d of pickDevices(a.devices)) {
      const r = await runDevice(browser, d, a.theme, url, out);
      runs.push(r);
      const n = r.results.reduce((k, s) => k + (s.problems ?? []).length, 0);
      console.log(`${d.id}: ${r.results.length} steps, ${n} layout problems, ${r.results.filter((s) => s.error).length} step errors`);
      for (const s of r.results) {
        if (s.error) console.log(`  ${s.step}: ERROR ${s.error}`);
        for (const p of s.problems ?? []) console.log(`  ${s.step}: ${p.severity} ${p.type}: ${p.detail}`);
        if (s.errors) for (const e of s.errors) console.log(`  console: ${String(e).slice(0, 200)}`);
      }
    }
  } finally {
    await browser.close();
  }
  writeFileSync(join(out, "report.json"), JSON.stringify(runs, null, 1));
  await api(url, "/api/execute", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id: "doc.new", params: {}, origin: "ui" }) });
}

main().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
