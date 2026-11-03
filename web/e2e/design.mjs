#!/usr/bin/env node
// OpenEtch Design (the browser-only build, src/mode.ts) on the device matrix: the design UI walked on a phone, an iPad,
// a MacBook and a desktop with a screenshot and the layout check (overlap.mjs) after every step. It needs the
// design-engine build (`npm run build:design-engine`: the design UI with the simulator engine as its backend) served by
// a SIMULATOR engine on a spare port, and it replaces that engine's document:
//
//   openetch --config-dir <tmp>/cfg --projects-dir <tmp>/projects --lightburn-prefs <tmp>/none.ini \
//     --listen 127.0.0.1:7963 --approval click --no-token --web-dir web/dist-design-engine
//   node e2e/design.mjs --url http://127.0.0.1:7963 [--devices iphone15-portrait,ipad-air-landscape] [--out dir]
//
// Besides the layout problems it reports every machine route the design UI still called (the design backend answers
// those 404, designmode/routes.ts) and the console errors. Results: <out>/<device>-<theme>/NN-step.png, report.json
// and report.md.

import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, sleep } from "./cdp.mjs";
import { pickDevices } from "./devices.mjs";
import { runCheck } from "./overlap.mjs";
import { applySafeArea, escape, execute, find, menuPath, moreSheet, phoneTab, pressFind } from "./scenarios.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));
const DEFAULT_DEVICES = "iphone15-portrait,iphone-se,ipad-air-landscape,ipad-air-portrait,macbook-air-1470,win-1920";

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

/** A small design: a plate, a circle and a text, on the default layer. */
async function seed(url) {
  await execute(url, "doc.new");
  await execute(url, "shape.rect", { width_mm: 60, height_mm: 40, center: [0, 0], name: "Plate" });
  await execute(url, "shape.ellipse", { rx_mm: 12, ry_mm: 12, center: [-15, 0], name: "Hole" });
  await execute(url, "shape.text", { text: "OpenEtch", height_mm: 6, center: [8, 0], name: "Label" });
}

function stepsFor(d) {
  const S = [];
  const add = (name, fn, kinds = ["phone", "tablet", "desktop"]) => kinds.includes(d.kind) && S.push({ name, fn });
  add("design-with-shapes", async ({ ctx }) => {
    await seed(ctx.url);
    await sleep(1500);
  });
  add("workspace-preview", async ({ page }) => pressFind(page, ".workspaces button, nav[role=tablist] button", "Preview"), ["desktop", "tablet"]);
  add("preview-sheet", async ({ page }) => phoneTab(page, "Preview"), ["phone"]);
  add("menu-file", async ({ page }) => {
    await escape(page);
    await pressFind(page, "button", "File");
  }, ["desktop", "tablet"]);
  add("dialog-open", async ({ page }) => {
    await escape(page);
    await menuPath(page, ["File", "Open"]);
  }, ["desktop", "tablet"]);
  add("dialog-devices", async ({ page }) => {
    await escape(page);
    await menuPath(page, ["File", "Laser", "Devices"]);
  }, ["desktop", "tablet"]);
  add("dialog-export", async ({ page }) => {
    await escape(page);
    await menuPath(page, ["File", "Export LightBurn"]);
  }, ["desktop", "tablet"]);
  add("dialog-save-as", async ({ page }) => {
    await escape(page);
    await menuPath(page, ["File", "Save as"]);
  }, ["desktop", "tablet"]);
  add("menu-help", async ({ page }) => {
    await escape(page);
    // The Help menu on wide windows; the ⋯ overflow (with Help in it) on compact ones and tablets.
    const help = await find(page, ".topbar button.file-btn", "Help");
    if (help) await page.press(help.x, help.y);
    else await pressFind(page, ".topbar button", "More");
  }, ["desktop", "tablet"]);
  add("device-200mm", async ({ page }) => {
    await escape(page);
    if (d.kind === "phone") await phoneTab(page, "Preview");
    else await pressFind(page, ".workspaces button, nav[role=tablist] button", "Preview");
    await sleep(400);
    // Tablets: the Preview window sits on the right rail; open it.
    if (d.kind === "tablet" && !(await find(page, ".dev-select-bar select"))) {
      await pressFind(page, ".dock-rail-btn", "Preview");
      await sleep(500);
    }
    const ok = await page.evaluate(() => {
      const sel = document.querySelector(".dev-select-bar select");
      const opt = sel && [...sel.options].find((o) => o.textContent.includes("200 mm"));
      if (!sel || !opt) return false;
      sel.value = opt.value;
      sel.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    });
    if (!ok) throw new Error("no design device picker with a 200 mm field");
    await sleep(400);
    await pressFind(page, "[role=dialog] button, .modal button", "Switch");
    await sleep(1200);
  });
  add("more-sheet", async ({ page }) => moreSheet(page), ["phone"]);
  add("workspace-design", async ({ page }) => {
    await escape(page);
    if (d.kind === "phone") await phoneTab(page, "Canvas");
    else await pressFind(page, ".workspaces button, nav[role=tablist] button", "Design");
  });
  return S;
}

async function runDevice(browser, d, theme, ctx) {
  const dir = join(ctx.out, `${d.id}-${theme}`);
  mkdirSync(dir, { recursive: true });
  const page = await browser.newPage();
  const results = [];
  try {
    await page.setDevice(d, { theme });
    await execute(ctx.url, "doc.new");
    await page.goto(`${ctx.url}/`, { waitFor: ".topbar", timeout: 20000 });
    await sleep(1500);
    if (d.safe && Object.values(d.safe).some((v) => v > 0)) await applySafeArea(page, d.safe);
    const check = async (name, extra = {}) => {
      const shot = join(dir, `${String(results.length).padStart(2, "0")}-${name}.png`);
      await page.settle(250);
      await page.screenshot(shot);
      const c = await runCheck(page, { touch: d.touch, safe: d.safe }).catch((e) => ({ error: String(e), problems: [] }));
      results.push({ step: name, shot, ...extra, problems: c.problems, error: extra.error ?? c.error });
    };
    const design = await page.evaluate(() => document.documentElement.hasAttribute("data-design-only"));
    if (!design) throw new Error("not the design build (html[data-design-only] missing)");
    await check("first-visit");
    const close = await find(page, ".modal-foot button, [role=dialog] button", "Close");
    if (close) {
      await page.press(close.x, close.y);
      await sleep(400);
    }
    for (const s of stepsFor(d)) {
      let error;
      try {
        await s.fn({ page, ctx, d });
      } catch (e) {
        error = String(e.message ?? e);
      }
      await sleep(400);
      await check(s.name, { error });
    }
    const machineCalls = await page.evaluate(() => globalThis.__oeDesignMachineCalls ?? []);
    results.push({ step: "machine-calls", calls: [...new Set(machineCalls)], problems: [] });
    if (page.errors.length) results.push({ step: "console-errors", errors: page.errors.slice(0, 20), problems: [] });
  } finally {
    await page.close();
  }
  return { device: d.id, name: d.name, theme, dir, results };
}

function markdown(runs) {
  const lines = ["# OpenEtch Design on the device matrix", ""];
  for (const r of runs) {
    lines.push(`## ${r.name} (${r.device}, ${r.theme})`, "");
    for (const s of r.results) {
      const probs = (s.problems ?? []).filter((p) => p.type !== "small");
      const small = (s.problems ?? []).find((p) => p.type === "small");
      lines.push(`- **${s.step}** ${s.shot ? `\`${s.shot}\`` : ""}${s.error ? ` ERROR: ${s.error}` : ""}${small ? ` (${small.count} small targets: ${small.detail ?? ""})` : ""}`);
      for (const p of probs) lines.push(`  - ${p.severity} ${p.type}: ${p.detail}`);
      if (s.calls) lines.push(`  - machine routes called: ${s.calls.length ? s.calls.join(", ") : "none"}`);
      if (s.errors) for (const e of s.errors) lines.push(`  - console: ${String(e).slice(0, 200)}`);
    }
    lines.push("");
  }
  return lines.join("\n");
}

async function main() {
  const a = args(process.argv.slice(2));
  if (!a.url) {
    console.log("usage: node e2e/design.mjs --url http://127.0.0.1:7963 [--devices ...] [--out dir] [--theme dark|light]");
    process.exitCode = 2;
    return;
  }
  const url = a.url.replace(/\/$/, "");
  const run = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const out = resolve(a.out ?? join(here, "..", "..", "target", "ux-evidence", `design-${run}`));
  mkdirSync(out, { recursive: true });
  const browser = await launch({ port: a.port ?? 9334, headless: !a.headful });
  const runs = [];
  try {
    for (const d of pickDevices(a.devices ?? DEFAULT_DEVICES)) {
      const theme = a.theme ?? d.themes[0];
      const r = await runDevice(browser, d, theme, { url, out });
      runs.push(r);
      const n = r.results.reduce((k, s) => k + (s.problems ?? []).filter((p) => p.type !== "small").length, 0);
      const small = r.results.reduce((k, s) => k + ((s.problems ?? []).find((p) => p.type === "small")?.count ?? 0), 0);
      const calls = r.results.find((s) => s.step === "machine-calls")?.calls ?? [];
      console.log(`${d.id} ${theme}: ${r.results.length} steps, ${n} problems, ${small} small targets, machine calls: ${calls.join(", ") || "none"}`);
      writeFileSync(join(out, "report.json"), JSON.stringify(runs, null, 1));
    }
  } finally {
    await browser.close();
  }
  writeFileSync(join(out, "report.md"), markdown(runs));
  console.log(`report: ${join(out, "report.md")}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
