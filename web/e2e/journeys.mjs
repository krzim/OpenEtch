#!/usr/bin/env node
// Longer, adversarial user journeys for the device harness (web/e2e/README.md): sequences a real person does on a
// phone, an iPad or a MacBook (rotate mid-task, a sheet over an active tool, a long-press inside a pinch, the keyboard
// up while editing text, every dialog on the phone, GRBL and back, Beginner Mode, a very long project name, warnings
// and toasts together, framing on the tablet, a small MacBook with every dock open, ...). Each journey runs in a fresh
// browser context, takes a screenshot and a layout check (overlap.mjs) after every step, and writes
// <out>/<journey>/NN-step.png plus <out>/journeys.json and <out>/journeys.md.
//
//   node e2e/journeys.mjs --url http://127.0.0.1:7931 [--doc file.lbrn2] [--only rotate,grbl] [--out dir] [--cdp-port 9333]
//
// Like scenarios.mjs it needs an engine on the simulator (never a real machine: a journey runs a job on the simulator
// and switches the engine's device) and it replaces that engine's document.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, sleep } from "./cdp.mjs";
import { DEVICES } from "./devices.mjs";
import { runCheck } from "./overlap.mjs";
import {
  api,
  applySafeArea,
  canvasCenter,
  canvasPoint,
  escape,
  execute,
  find,
  loadHeavy,
  menuPath,
  moreSheet,
  openSection,
  phoneTab,
  pressFind,
  selectSomething,
  selectionCount,
  sheetOpen,
  sheetTo,
  TARGETS,
} from "./scenarios.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));
const dev = (id) => {
  const d = DEVICES.find((x) => x.id === id);
  if (!d) throw new Error(`no device ${id}`);
  return d;
};
/** A device of the matrix with other sizes (a rotation, an iPad split view). */
const variant = (id, patch) => ({ ...dev(id), ...patch });

function args(argv) {
  const a = {};
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--url") a.url = v();
    else if (k === "--out") a.out = v();
    else if (k === "--doc") a.doc = v();
    else if (k === "--only") a.only = v();
    else if (k === "--cdp-port") a.port = Number(v());
  }
  return a;
}

const LONG_NAME =
  "Halloween coaster set for the school fundraiser - final revision with the corrected disc spacing and the new ghost artwork v12";

async function closeAppOffer(page) {
  // The "Use as an app" offer comes a moment after the first touch visit.
  await page.waitFor(`[...document.querySelectorAll("[role=dialog]")].some((e) => /as an app/i.test(e.textContent))`, { expr: true, timeout: 2500 });
  const close = await find(page, ".modal-foot button, [role=dialog] button", "Close");
  if (close?.hit) {
    await page.press(close.x, close.y);
    await sleep(400);
  }
}

async function closeSheet(page) {
  if (await sheetOpen(page)) await pressFind(page, ".sheet-close");
  await sleep(300);
}

async function setPrefs(page, patch) {
  await page.evaluate((p) => {
    let cur = {};
    try {
      cur = JSON.parse(localStorage.getItem("openetch.prefs") || "{}");
    } catch {
      /* fresh */
    }
    localStorage.setItem("openetch.prefs", JSON.stringify({ ...cur, ...p }));
  }, patch);
}

/** Visible text that is cut off (ellipsis or overflow) in buttons, tabs and headings: unreadable labels. */
async function clippedText(page) {
  return page.evaluate(() => {
    const out = [];
    for (const e of document.querySelectorAll("button, [role=tab], [role=menuitem], label, h1, h2, h3, .modal-title, .topbar-title, .doc-title, summary, th")) {
      const r = e.getBoundingClientRect();
      if (r.width < 4 || r.height < 4 || r.bottom < 0 || r.top > innerHeight) continue;
      const cs = getComputedStyle(e);
      if (cs.visibility === "hidden") continue;
      const over = e.scrollWidth > e.clientWidth + 1 && cs.overflowX !== "visible" && cs.overflowX !== "auto" && cs.overflowX !== "scroll";
      if (over) out.push(`${e.tagName.toLowerCase()}.${String(e.className).split(" ")[0]} "${(e.textContent || "").trim().slice(0, 40)}" ${Math.round(e.clientWidth)}/${e.scrollWidth}`);
    }
    return out.slice(0, 15);
  });
}

// --- journeys ----------------------------------------------------------------------------------------------------
// Each: { id, device, theme?, run(j) } where j = { page, ctx, shot(name), d, setDevice(d) }.

const JOURNEYS = [
  {
    id: "phone-rotate-mid-task",
    device: "iphone15-portrait",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await j.shot("portrait-heavy");
      j.ctx.sel = await selectSomething(page);
      await phoneTab(page, "Canvas");
      if (!(await sheetOpen(page))) await phoneTab(page, "Canvas");
      await pressFind(page, "button", "Rectangle");
      await sleep(300);
      await closeSheet(page);
      await j.shot("portrait-rect-tool-with-selection");
      await j.setDevice(dev("iphone15-landscape"));
      await j.shot("rotated-landscape-rect-tool");
      await phoneTab(page, "Job");
      if (!(await sheetOpen(page))) await phoneTab(page, "Job");
      await j.shot("landscape-job-sheet");
      await sheetTo(page, "full");
      await j.shot("landscape-job-sheet-full");
      await j.setDevice(dev("iphone15-portrait"));
      await j.shot("rotated-back-portrait-job-full");
      await closeSheet(page);
      await j.shot("portrait-job-closed");
    },
  },
  {
    id: "phone-sheet-over-active-tool",
    device: "iphone15-portrait",
    theme: "light",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await phoneTab(page, "Canvas");
      if (!(await sheetOpen(page))) await phoneTab(page, "Canvas");
      await pressFind(page, "button", "Line");
      await sleep(300);
      await closeSheet(page);
      const a = await canvasPoint(page, -60, 60);
      const b = await canvasPoint(page, -20, 40);
      await page.press(a.x, a.y);
      await sleep(150);
      await page.press(b.x, b.y);
      await j.shot("line-in-progress");
      await phoneTab(page, "Layers");
      if (!(await sheetOpen(page))) await phoneTab(page, "Layers");
      await sheetTo(page, "half");
      await j.shot("layers-half-over-line-tool");
      await phoneTab(page, "Job");
      await sheetTo(page, "half");
      await j.shot("job-half-over-line-tool");
      await phoneTab(page, "More");
      await sheetTo(page, "peek");
      await j.shot("more-peek-over-line-tool");
      await closeSheet(page);
      await page.key("Escape");
      await j.shot("line-tool-escaped");
    },
  },
  {
    id: "phone-longpress-inside-pinch",
    device: "pixel8",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await closeSheet(page);
      const c = await canvasCenter(page);
      // One finger down and held (a long-press starts), the second lands, the pinch spreads, then both hold still.
      await page.touch("touchStart", [{ x: c.x - 30, y: c.y, id: 0 }]);
      await sleep(350);
      await page.touch("touchStart", [{ x: c.x - 30, y: c.y, id: 0 }, { x: c.x + 30, y: c.y, id: 1 }]);
      for (let i = 1; i <= 10; i++) {
        await page.touch("touchMove", [{ x: c.x - 30 - i * 6, y: c.y, id: 0 }, { x: c.x + 30 + i * 6, y: c.y, id: 1 }]);
        await sleep(16);
      }
      await sleep(900);
      await j.shot("pinch-held-900ms");
      await page.touch("touchEnd", [{ x: c.x + 90, y: c.y, id: 1 }]);
      await sleep(900);
      await j.shot("one-finger-left-held");
      await page.touch("touchEnd", []);
      await sleep(400);
      j.note("context menu open after the pinch", await page.evaluate(() => !!document.querySelector(".ctx-menu, [role=menu]")));
      j.note("page zoom", await page.evaluate(() => window.visualViewport?.scale ?? 1));
      await j.shot("after-release");
    },
  },
  {
    id: "phone-select-all-many",
    device: "iphone-se",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await closeSheet(page);
      await page.key("a", { modifiers: 2 });
      await sleep(600);
      await j.shot("select-all");
      // A marquee over half the jig with one finger (touchOneFinger "select").
      const a = await canvasPoint(page, -70, 20);
      const b = await canvasPoint(page, 5, -60);
      await page.key("Escape");
      await sleep(200);
      await page.drag(a.x, a.y, b.x, b.y, { steps: 16 });
      await sleep(500);
      await j.shot("marquee-half");
      await phoneTab(page, "Canvas");
      if (!(await sheetOpen(page))) await phoneTab(page, "Canvas");
      await sheetTo(page, "half");
      await j.shot("canvas-sheet-half-with-many");
      await sheetTo(page, "full");
      await j.shot("canvas-sheet-full-with-many");
    },
  },
  {
    id: "phone-text-keyboard",
    device: "iphone15-portrait",
    async run(j) {
      const { page, d } = j;
      await j.heavy();
      await phoneTab(page, "Canvas");
      if (!(await sheetOpen(page))) await phoneTab(page, "Canvas");
      const ok = await pressFind(page, "button", "Text").catch(() => null);
      if (!ok) await pressFind(page, "button", "Add text");
      await sleep(300);
      await closeSheet(page);
      // A spot of bare canvas (the tool's own option bar covers the top-left).
      const p = await canvasCenter(page);
      await page.press(p.x, p.y);
      await sleep(500);
      await j.shot("text-tool-tapped");
      j.note("focused", await page.evaluate(() => `${document.activeElement?.tagName} ${document.activeElement?.className ?? ""}`));
      await page.setViewportHeight(d.height - d.keyboard);
      await sleep(400);
      await j.shot("keyboard-up");
      await page.type("Happy Halloween from the whole class");
      await sleep(400);
      await j.shot("typed-with-keyboard");
      await page.key("Enter");
      await sleep(300);
      await page.setViewportHeight(d.height);
      await page.evaluate(() => document.activeElement?.blur?.());
      await sleep(400);
      await j.shot("keyboard-down");
      j.clip = await clippedText(page);
    },
  },
  {
    id: "phone-every-dialog",
    device: "iphone15-portrait",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await moreSheet(page);
      const sections = await page.evaluate(() => [...document.querySelectorAll(".phone-more details > summary")].map((s) => s.textContent.trim()));
      j.note("More sections", sections);
      const seen = new Set();
      for (const sec of sections) {
        await escape(page);
        await moreSheet(page);
        await openSection(page, sec);
        const labels = await page.evaluate((t) => {
          const s = [...document.querySelectorAll(".phone-more details > summary")].find((x) => x.textContent.trim() === t);
          return [...(s?.parentElement.querySelectorAll("button") ?? [])].map((b) => (b.getAttribute("aria-label") || b.textContent || "").trim()).filter(Boolean);
        }, sec);
        for (const l of labels) {
          if (seen.has(l) || /^(new|open|quit|exit|delete|remove|reset|start|stop|home|sign|log ?out|reload|frame|clear)/i.test(l)) continue;
          if (!/…|\.\.\.|settings|preferences|devices|library|about|help|shortcuts|keys|console|move|optimi|rotary|material|art|template|export|save as|import/i.test(l)) continue;
          seen.add(l);
          await escape(page);
          await moreSheet(page);
          await openSection(page, sec);
          const b = await find(page, ".phone-more button", l);
          if (!b?.hit) {
            j.note(`not hit-testable: ${l}`, b?.cover ?? "missing");
            continue;
          }
          await page.press(b.x, b.y);
          await sleep(700);
          await j.shot(`dialog-${l.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 30)}`);
          j.clip = [...(j.clip ?? []), ...(await clippedText(page)).map((c) => `${l}: ${c}`)];
        }
      }
      await escape(page);
    },
  },
  {
    id: "grbl-and-back-phone",
    device: "iphone15-portrait",
    async run(j) {
      await grblJourney(j);
    },
  },
  {
    id: "grbl-and-back-macbook",
    device: "macbook-air-1470",
    async run(j) {
      await grblJourney(j);
    },
  },
  {
    id: "phone-theme-switch-in-app",
    device: "iphone15-portrait",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await moreSheet(page);
      await openSection(page, "Settings");
      await pressFind(page, ".phone-more button", "Preferences");
      await sleep(500);
      await j.shot("preferences");
      await pressFind(page, "[role=dialog] [role=radio]", "Light");
      await sleep(400);
      await j.shot("preferences-light");
      await escape(page);
      await phoneTab(page, "Job");
      await sheetTo(page, "half");
      await j.shot("light-job-half");
      await closeSheet(page);
      j.ctx.sel = await selectSomething(page);
      await j.shot("light-canvas-selection");
    },
  },
  {
    id: "beginner-mode-phone",
    device: "iphone15-portrait",
    async run(j) {
      const { page } = j;
      await setPrefs(page, { beginnerMode: true });
      await j.reload();
      await j.heavy();
      await phoneTab(page, "Job");
      if (!(await sheetOpen(page))) await phoneTab(page, "Job");
      await sheetTo(page, "half");
      await j.shot("beginner-job-half");
      await sheetTo(page, "full");
      await j.shot("beginner-job-full");
    },
  },
  {
    id: "beginner-mode-ipad",
    device: "ipad-air-landscape",
    async run(j) {
      const { page } = j;
      await setPrefs(page, { beginnerMode: true });
      await j.reload();
      await j.heavy();
      await pressFind(page, ".workspaces button, nav[role=tablist] button", "Laser");
      await sleep(600);
      await j.shot("beginner-laser-workspace");
    },
  },
  {
    id: "long-name-phone",
    device: "iphone-se",
    async run(j) {
      await j.heavy(LONG_NAME);
      await j.shot("long-name");
      await phoneTab(j.page, "Job");
      await j.shot("long-name-job");
      j.clip = await clippedText(j.page);
      // A tap on the cut name shows it in full (with Save as…).
      await closeSheet(j.page);
      await pressFind(j.page, ".project-btn");
      await sleep(400);
      j.note("project menu", await j.page.evaluate(() => document.querySelector(".project-pop-name")?.textContent ?? null));
      await j.shot("long-name-tapped");
      await escape(j.page);
    },
  },
  {
    id: "long-name-ipad-portrait",
    device: "ipad-air-portrait",
    theme: "light",
    async run(j) {
      await j.heavy(LONG_NAME);
      await j.shot("long-name");
      j.clip = await clippedText(j.page);
    },
  },
  {
    id: "long-name-macbook-1280",
    device: "macbook-air-1280",
    async run(j) {
      await j.heavy(LONG_NAME);
      await j.shot("long-name");
      j.clip = await clippedText(j.page);
    },
  },
  {
    id: "warnings-and-toasts-phone",
    device: "iphone15-portrait",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await closeSheet(page);
      // Off-field: drag the right-hand object 45 mm right.
      const [x, y] = TARGETS[1];
      const p0 = await canvasPoint(page, x, y);
      await page.press(p0.x, p0.y);
      await sleep(700);
      const p = await canvasPoint(page, x, y);
      const q = await canvasPoint(page, x + 45, y);
      await page.drag(p.x, p.y, q.x, q.y, { steps: 14 });
      await sleep(900);
      await j.shot("off-field-warning");
      // Toasts: copy, paste, duplicate in a row (each says what it did), then an import with warnings.
      await page.key("c", { modifiers: 2 });
      await page.key("v", { modifiers: 2 });
      await sleep(150);
      await page.key("d", { modifiers: 2 });
      await sleep(150);
      await page.key("z", { modifiers: 2 });
      await j.shot("warning-plus-toasts");
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="400mm" height="400mm" viewBox="0 0 400 400"><image href="nope.png" x="0" y="0" width="10" height="10"/><text x="10" y="20" font-family="NoSuchFont">Hi</text><rect x="300" y="300" width="80" height="80" fill="none" stroke="#000"/><filter id="f"/><path d="M 10 10 Q" stroke="#000"/></svg>`;
      j.note("import", await api(j.ctx.url, `/api/document/upload?format=svg&name=warnings.svg`, { method: "POST", body: svg }));
      await sleep(1500);
      await j.shot("warning-toasts-import-warnings");
      await phoneTab(page, "Job");
      await sleep(400);
      await j.shot("all-messages-job-tab");
    },
  },
  {
    id: "ipad-laser-framing-portrait",
    device: "ipad-air-portrait",
    async run(j) {
      await framingTablet(j);
    },
  },
  {
    id: "ipad-laser-framing-landscape",
    device: "ipad-air-landscape",
    theme: "light",
    async run(j) {
      await framingTablet(j);
    },
  },
  {
    id: "macbook-1280-all-docks",
    device: "macbook-air-1280",
    async run(j) {
      const { page } = j;
      await j.heavy();
      // Every dock: the Browser panel on the left, every section of the right-hand panels expanded, the timeline.
      if (!(await page.evaluate(() => document.querySelector('button[aria-label="Browser panel"]')?.classList.contains("active"))))
        await pressFind(page, "button.panel-toggle", "Browser panel");
      await sleep(400);
      for (let i = 0; i < 8; i++) {
        const c = await find(page, ".dock-toggle[aria-expanded=false]");
        if (!c?.hit) break;
        await page.press(c.x, c.y);
        await sleep(300);
      }
      j.note("collapsed docks left", await page.evaluate(() => document.querySelectorAll(".dock-toggle[aria-expanded=false]").length));
      await j.shot("all-docks-open");
      j.ctx.sel = await selectSomething(page);
      await j.shot("all-docks-selection");
      await pressFind(page, ".workspaces button, nav[role=tablist] button", "Laser");
      await sleep(600);
      await j.shot("all-docks-laser");
      j.clip = await clippedText(page);
    },
  },
  {
    id: "ipad-split-view",
    // iPad Air portrait in Split View, half the screen (and Slide Over's phone width): the iPad UA at narrow sizes.
    device: "ipad-air-portrait",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await j.setDevice(variant("ipad-air-portrait", { width: 507, height: 1106 }));
      await j.shot("split-half-507");
      j.ctx.sel = await selectSomething(page);
      await j.shot("split-half-selection");
      await j.setDevice(variant("ipad-air-landscape", { width: 375, height: 746 }));
      await j.shot("slide-over-375");
      await j.setDevice(variant("ipad-air-landscape", { width: 678, height: 746 }));
      await j.shot("split-landscape-678");
    },
  },
  {
    id: "phone-job-running",
    device: "iphone15-portrait",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await phoneTab(page, "Job");
      if (!(await sheetOpen(page))) await phoneTab(page, "Job");
      await sheetTo(page, "half");
      await j.shot("job-half-before-start");
      // Start on the simulator (the app's own Start button: the only way a job starts).
      const s = await find(page, ".btn-start", null);
      if (!s?.hit) throw new Error(`Start not hit-testable (${s?.cover ?? "missing"})`);
      await page.press(s.x, s.y);
      await sleep(1200);
      await j.shot("after-start");
      // --approval click: an approval prompt may ask first.
      const ap = await find(page, "[role=dialog] button, [role=alertdialog] button", "Start");
      if (ap?.hit) {
        await page.press(ap.x, ap.y);
        await sleep(1500);
      }
      await j.shot("running");
      await closeSheet(page);
      await j.shot("running-sheet-closed");
      await pressFind(page, ".btn-stop", null);
      await sleep(1200);
      await j.shot("stopped");
    },
  },
  {
    id: "context-menu-at-edges",
    device: "iphone15-landscape",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await closeSheet(page);
      const vp = await page.evaluate(() => {
        const r = document.querySelector(".viewport").getBoundingClientRect();
        return { x: r.left, y: r.top, w: r.width, h: r.height };
      });
      for (const [n, fx, fy] of [
        ["bottom-right", 0.92, 0.9],
        ["top-left", 0.1, 0.12],
      ]) {
        await escape(page);
        await page.longPress(vp.x + vp.w * fx, vp.y + vp.h * fy, 750);
        await sleep(400);
        await j.shot(`ctx-${n}`);
      }
      await escape(page);
    },
  },
  {
    id: "ui-scale-large",
    device: "pixel8",
    async run(j) {
      const { page } = j;
      await setPrefs(page, { uiScale: 1.5 });
      await j.reload();
      await j.heavy();
      await j.shot("scale-1.5");
      j.ctx.sel = await selectSomething(page);
      await phoneTab(page, "Job");
      await closeSheet(page);
      await j.shot("scale-1.5-job-selection");
      await phoneTab(page, "Job");
      await sheetTo(page, "half");
      await j.shot("scale-1.5-job-half");
    },
  },
  {
    id: "macbook-resize-mid-session",
    device: "macbook-air-1470",
    theme: "light",
    async run(j) {
      const { page } = j;
      await j.heavy();
      j.ctx.sel = await selectSomething(page);
      await j.shot("1470");
      await j.setDevice(variant("macbook-air-1470", { width: 1000, height: 700 }));
      await j.shot("window-1000x700");
      await j.setDevice(variant("macbook-air-1470", { width: 760, height: 600 }));
      await j.shot("window-760x600");
      await j.setDevice(dev("macbook-air-1470"));
      await j.shot("back-1470");
    },
  },
  {
    id: "ipad-keyboard-then-rotate",
    device: "ipad-air-portrait",
    async run(j) {
      const { page, d } = j;
      await j.heavy();
      j.ctx.sel = await selectSomething(page);
      const pill = await find(page, ".properties-pill");
      if (pill?.hit) await page.press(pill.x, pill.y);
      await sleep(400);
      const inp = await find(page, ".dock-overlay input, input[inputmode=decimal], input.num");
      if (!inp) throw new Error("no input");
      await page.press(inp.x, inp.y);
      await page.setViewportHeight(d.height - d.keyboard);
      await sleep(500);
      await j.shot("portrait-keyboard");
      const L = dev("ipad-air-landscape");
      await j.setDevice({ ...L, height: L.height - L.keyboard });
      await j.shot("landscape-keyboard");
      await j.setDevice(L);
      await page.evaluate(() => document.activeElement?.blur?.());
      await sleep(400);
      await j.shot("landscape-keyboard-down");
    },
  },
  {
    id: "phone-undo-tools-palette",
    device: "iphone15-standalone",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await closeSheet(page);
      j.ctx.sel = await selectSomething(page);
      const p = await canvasPoint(page, ...TARGETS[0]);
      const q = await canvasPoint(page, TARGETS[0][0] + 10, TARGETS[0][1] + 10);
      await page.drag(p.x, p.y, q.x, q.y, { steps: 8 });
      await sleep(400);
      // Two undos: the move, then the import itself (an easy slip on a phone): Redo must still be there.
      for (let i = 0; i < 2; i++) {
        await pressFind(page, ".phone-hud-btn", "Undo");
        await sleep(400);
      }
      await j.shot("after-undo-import-undone");
      await pressFind(page, ".phone-hud-btn", "Redo");
      await sleep(1500);
      await j.shot("redone");
      await pressFind(page, ".phone-hud-btn", "Tools");
      await sleep(500);
      await j.shot("tools-palette");
    },
  },
  {
    id: "phone-guides-and-corner",
    device: "iphone15-portrait",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await closeSheet(page);
      await j.shot("no-guides-corner");
      // A guide dragged out of the top ruler, then one out of the left ruler: the corner turns into Clear guides.
      const top = await page.evaluate(() => {
        const r = document.querySelector(".et-ruler-top")?.getBoundingClientRect();
        return r ? { x: r.left + r.width * 0.6, y: r.top + r.height / 2 } : null;
      });
      const left = await page.evaluate(() => {
        const r = document.querySelector(".et-ruler-left")?.getBoundingClientRect();
        return r ? { x: r.left + r.width / 2, y: r.top + r.height * 0.6 } : null;
      });
      if (!top || !left) throw new Error("no rulers");
      await page.drag(top.x, top.y, top.x, top.y + 160, { steps: 12 });
      await sleep(400);
      await page.drag(left.x, left.y, left.x + 150, left.y, { steps: 12 });
      await sleep(500);
      await j.shot("two-guides");
      const corner = await find(page, ".et-ruler-corner");
      j.note("corner", corner ? { w: corner.w, h: corner.h, hit: corner.hit } : null);
      if (corner?.hit) await page.press(corner.x, corner.y);
      await sleep(500);
      await j.shot("guides-cleared");
    },
  },
  {
    id: "phone-menu-button-and-landscape-sheets",
    device: "iphone15-landscape",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await pressFind(page, ".topbar-menu");
      await sleep(500);
      await j.shot("menu-button");
      await escape(page);
      j.ctx.sel = await selectSomething(page);
      await sleep(400);
      await j.shot("selection-peek");
      await phoneTab(page, "Job");
      if (!(await sheetOpen(page))) await phoneTab(page, "Job");
      await sheetTo(page, "peek");
      await j.shot("job-peek");
      const s = await find(page, ".bottom-sheet .btn-start");
      j.note("start in the peek", s ? { hit: s.hit, cover: s.cover, h: s.h } : null);
      await sheetTo(page, "half");
      await j.shot("job-half");
    },
  },
  {
    id: "phone-se-action-bar",
    device: "iphone-se",
    async run(j) {
      const { page } = j;
      await j.heavy();
      // One small square, so the bar has room above it.
      await execute(j.ctx.url, "doc.new");
      await execute(j.ctx.url, "shape.rect", { width_mm: 20, height_mm: 20, center: [0, -20] });
      await sleep(1200);
      await closeSheet(page);
      const p = await canvasPoint(page, 10, -20);
      await page.press(p.x, p.y);
      await sleep(700);
      await closeSheet(page);
      const bar = () =>
        page.evaluate(() => {
          const b = document.querySelector(".touch-actions");
          if (!b) return null;
          const r = b.getBoundingClientRect();
          return { buttons: b.querySelectorAll("button").length, scroll: b.scrollWidth - b.clientWidth, left: Math.round(r.left), right: Math.round(r.right), vw: innerWidth };
        });
      j.note("action bar", await bar());
      await j.shot("action-bar");
      const more = await find(page, ".touch-actions button", "More…");
      if (more?.hit) {
        await page.press(more.x, more.y);
        await sleep(400);
      }
      j.note("overflow menu", await page.evaluate(() => [...document.querySelectorAll("[role=menu] [role=menuitem]")].map((e) => e.textContent.trim())));
      await j.shot("action-bar-overflow");
      await escape(page);
    },
  },
  {
    id: "macbook-multi-trace",
    device: "macbook-air-1470",
    async run(j) {
      const { page } = j;
      await j.heavy();
      await pressFind(page, "button", "Command search");
      await sleep(400);
      await page.type("Multi-File Trace");
      await sleep(400);
      await page.key("Enter");
      await sleep(700);
      j.note("number field widths", await page.evaluate(() => [...document.querySelectorAll("[role=dialog] .tools-cols .field-input")].map((e) => Math.round(e.getBoundingClientRect().width))));
      await j.shot("multi-trace");
      await escape(page);
    },
  },
  // The Material Library's Edit Cut with a preset's pauses for the operator (kept beside the .clb), on each kind of
  // screen: a generic one-preset library is loaded, a preset is saved from a layer that pauses before its sub-layer,
  // and Edit Cut turns the layer's own pause on.
  ...["iphone15-portrait", "ipad-air-portrait", "macbook-air-1470", "win-1920"].map((device) => ({
    id: `library-edit-cut-${device}`,
    device,
    theme: "light",
    run: (j) => libraryEditCut(j),
  })),
];

/** A one-preset LightBurn library (generic values, no private settings). */
const E2E_CLB = [
  '<?xml version="1.0" encoding="UTF-8"?>',
  "<LightBurnLibrary>",
  '  <Material name="Steel">',
  '    <Entry Thickness="-1.0000" Desc="Engrave" NoThickTitle="Finish">',
  '      <CutSetting type="Scan">',
  '        <index Value="0" />',
  '        <name Value="" />',
  '        <LinkPath Value="Steel/Finish/Engrave" />',
  '        <maxPower Value="38" />',
  '        <speed Value="1000" />',
  '        <frequency Value="25000" />',
  '        <interval Value="0.025" />',
  '        <priority Value="0" />',
  "      </CutSetting>",
  "    </Entry>",
  "  </Material>",
  "</LightBurnLibrary>",
  "",
].join(String.fromCharCode(13, 10));

async function libraryEditCut(j) {
  const { page } = j;
  const url = j.ctx.url;
  const json = (body) => ({ method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const lib = `e2e-${j.d.id}`;
  await api(url, "/api/library/unload", json({ name: lib, discard: true }));
  const loaded = await api(url, `/api/library/load?name=${lib}`, { method: "POST", body: E2E_CLB });
  if (!loaded?.selected) throw new Error(`library load: ${JSON.stringify(loaded).slice(0, 200)}`);
  const doc = await api(url, "/api/document");
  const layer = doc?.layers?.[0]?.id ?? "layer-1";
  await execute(url, "shape.rect", { width_mm: 20, height_mm: 20 });
  await execute(url, "layer.sub_add", { layer });
  await execute(url, "layer.sub_update", { layer, index: 0, settings: { options: { operator_pause: true } } });
  const made = await api(url, `/api/library/${lib}/entries`, json({ layer, material: "Steel", title: "Finish", description: "Two passes" }));
  if (!made?.id) throw new Error(`create preset: ${JSON.stringify(made).slice(0, 200)}`);
  await j.reload();
  // The Material Library as a person opens it: command search on a computer, the dock tab at the iPad's right edge,
  // the Layers sheet on the phone (the library sits under the layers).
  const treeShown = async () => !!(await find(page, "[role=treeitem]", "Two passes"))?.hit;
  if (j.d.kind === "phone") {
    await phoneTab(page, "Layers");
    if (!(await sheetOpen(page))) await phoneTab(page, "Layers");
    await sheetTo(page, "full");
    await page.evaluate(() => document.querySelector('[data-dock="library"]')?.scrollIntoView({ block: "start" }));
    await sleep(400);
    if (!(await treeShown())) await pressFind(page, '[data-dock="library"] button', "Material Library");
  } else if (j.d.kind === "tablet") {
    await pressFind(page, "button, [role=tab]", "Material Library");
  } else {
    await pressFind(page, "button", "Command search");
    await sleep(400);
    await page.type("Material Library");
    await sleep(400);
    await page.key("Enter");
  }
  await sleep(900);
  await page.evaluate(() => [...document.querySelectorAll("[role=treeitem]")].find((e) => /Two passes/.test(e.textContent))?.scrollIntoView({ block: "center" }));
  await sleep(300);
  await pressFind(page, "[role=treeitem]", "Two passes");
  await sleep(400);
  await j.shot("library-preset-picked");
  await pressFind(page, "button", "Edit Cut");
  await sleep(600);
  j.note("pause boxes", await page.evaluate(() => [...document.querySelectorAll("[role=dialog] label")].map((l) => [l.textContent.trim(), !!l.querySelector("input:checked")]).filter(([t]) => /^Pause/.test(t))));
  await j.shot("edit-cut-sub-layer-pause");
  await pressFind(page, "[role=dialog] label", "Pause before this layer");
  await sleep(300);
  await j.shot("edit-cut-layer-pause-on");
  await pressFind(page, "[role=dialog] button", "OK");
  await sleep(1200);
  const tree = await api(url, `/api/library/${lib}`);
  const entries = (tree?.tree ?? tree)?.materials?.flatMap((m) => m.groups.flatMap((g) => g.entries)) ?? [];
  const s = entries.find((e) => e.path === "Steel/Finish/Two passes")?.summary;
  j.note("saved pauses", { operator_pause: s?.operator_pause, sub_layer_pauses: s?.sub_layer_pauses });
  if (!(s?.operator_pause === true && JSON.stringify(s?.sub_layer_pauses) === "[0]")) throw new Error(`the pauses were not saved: ${JSON.stringify(s)}`);
  await j.shot("edit-cut-saved");
  await escape(page);
  await api(url, "/api/library/unload", json({ name: lib, discard: true }));
}

async function grblJourney(j) {
  const { page, d } = j;
  await j.heavy();
  const phone = d.kind === "phone";
  // The galvo to come back to: the device active before the journey (on a fresh simulator engine the built-in
  // profile), never a LightBurn device that only exists where LightBurn is installed.
  const before = await api(j.ctx.url, "/api/devices");
  const devs = Array.isArray(before?.devices) ? before.devices : [];
  const galvo = devs.find((x) => x.id === before.active && x.kind !== "gantry") ?? devs.find((x) => /galvo/.test(x.kind ?? ""));
  if (!galvo) throw new Error(`no galvo device to switch back to: ${JSON.stringify(devs.map((x) => x.id))}`);
  j.note("galvo before", galvo.id);
  const openDevices = async () => {
    await escape(page);
    if (phone) {
      await moreSheet(page);
      await openSection(page, "Desktop features");
      await pressFind(page, ".phone-more button", "Devices");
    } else await menuPath(page, ["File", "Laser", "Devices"]);
    await sleep(600);
  };
  await openDevices();
  await j.shot("devices");
  // The ways to add a device are under Add device.
  await pressFind(page, "[role=dialog] .modal-foot button", "Add device");
  await sleep(400);
  await j.shot("devices-add-menu");
  await pressFind(page, "[role=menuitem]", "New GRBL");
  await sleep(500);
  await j.shot("grbl-form");
  await pressFind(page, "[role=dialog] [role=group] button.btn-primary", null);
  await sleep(1200);
  await j.shot("grbl-added");
  // Make it the active device (the new row is selected after Add).
  await pressFind(page, "[role=dialog] button", "Make active");
  await sleep(700);
  await j.shot("switch-confirm");
  const ok = await find(page, ".dev-confirm button.btn-primary");
  j.note("switch confirm", ok ? { hit: ok.hit, cover: ok.cover } : null);
  if (ok?.hit) await page.press(ok.x, ok.y);
  await sleep(2000);
  j.note("active after switch", (await api(j.ctx.url, "/api/devices")).active);
  await escape(page);
  await j.shot("grbl-active-canvas");
  if (phone) {
    await phoneTab(page, "Job");
    if (!(await sheetOpen(page))) await phoneTab(page, "Job");
    await sheetTo(page, "full");
    await j.shot("grbl-job-full");
  } else {
    await pressFind(page, ".workspaces button, nav[role=tablist] button", "Laser");
    await sleep(600);
    await j.shot("grbl-laser-workspace");
    // The layer quick-edit and Cut settings show a gantry's own settings: no Frequency or Q-Pulse.
    const row = await find(page, "tr.cuts-row");
    if (row?.hit) {
      await page.press(row.x, row.y);
      await sleep(500);
    }
    j.note("grbl quick-edit fields", await page.evaluate(() => [...document.querySelectorAll(".cuts-quick-grid .field-label")].map((e) => e.textContent.trim())));
    await j.shot("grbl-layer-quick-edit");
    // The Design workspace has the gantry's Move and Console windows (and Window menu entries) too, as the Laser one.
    await pressFind(page, ".workspaces button, nav[role=tablist] button", "Design");
    await sleep(600);
    j.note("design windows", await page.evaluate(() => [...document.querySelectorAll("section.dock")].map((e) => e.getAttribute("aria-label"))));
    await j.shot("grbl-design-windows");
  }
  // And back to the galvo.
  await openDevices();
  await pressFind(page, "[role=dialog] tr, [role=dialog] [role=row]", galvo.display_name ?? galvo.name);
  await sleep(400);
  await pressFind(page, "[role=dialog] button", "Make active");
  await sleep(700);
  await j.shot("switch-back-confirm");
  const ok2 = await find(page, ".dev-confirm button.btn-primary");
  if (ok2?.hit) await page.press(ok2.x, ok2.y);
  await sleep(2000);
  const after = (await api(j.ctx.url, "/api/devices")).active;
  j.note("active after switching back", after);
  if (after !== galvo.id) throw new Error(`switching back made ${after} active, not ${galvo.id}`);
  await escape(page);
  if (phone) await closeSheet(page);
  await j.shot("galvo-again");
}

async function framingTablet(j) {
  const { page } = j;
  await j.heavy();
  j.ctx.sel = await selectSomething(page);
  await pressFind(page, ".workspaces button, nav[role=tablist] button", "Laser");
  await sleep(700);
  await j.shot("laser-workspace");
  await pressFind(page, "button", "Frame");
  await sleep(1500);
  await j.shot("framing");
  const b = await find(page, "button", "Stop frame");
  j.note("stop frame", b ? { hit: b.hit, cover: b.cover } : null);
  await j.shot("framing-2");
  if (b?.hit) await page.press(b.x, b.y);
  await sleep(800);
  await j.shot("framing-stopped");
}

// --- runner ------------------------------------------------------------------------------------------------------

async function runJourney(browser, J, base) {
  const d0 = dev(J.device);
  const theme = J.theme ?? "dark";
  const dir = join(base.out, J.id);
  mkdirSync(dir, { recursive: true });
  const page = await browser.newPage();
  const res = { id: J.id, device: J.device, theme, steps: [], notes: {}, error: null };
  const ctx = { url: base.url, doc: base.doc };
  let cur = d0;
  const j = {
    page,
    ctx,
    get d() {
      return cur;
    },
    note: (k, v) => (res.notes[k] = v),
    async setDevice(d) {
      cur = d;
      await page.setDevice(d, { theme });
      await sleep(700);
      if (Object.values(d.safe ?? {}).some((v) => v > 0)) await applySafeArea(page, d.safe);
      else await applySafeArea(page, { top: 0, right: 0, bottom: 0, left: 0 });
    },
    async reload() {
      await page.goto(base.url + "/", { waitFor: ".btn-stop", timeout: 20000 });
      await sleep(1200);
      if (Object.values(cur.safe ?? {}).some((v) => v > 0)) await applySafeArea(page, cur.safe);
      await closeAppOffer(page);
    },
    async heavy(name) {
      if (name && base.doc) {
        // The top bar shows the name the file came in with: upload it under the long name.
        await execute(base.url, "doc.new");
        const r = await api(base.url, `/api/document/upload?format=lbrn2&mode=replace&name=${encodeURIComponent(`${name}.lbrn2`)}`, { method: "POST", body: readFileSync(base.doc) });
        res.notes.upload = JSON.stringify(r).slice(0, 120);
        await execute(base.url, "doc.rename", { name });
      } else await loadHeavy(base.url, base.doc);
      await sleep(2500);
      const f = await find(page, "button", "Fit field");
      if (f?.hit) await page.press(f.x, f.y);
      await sleep(500);
    },
    async shot(name) {
      const file = join(dir, `${String(res.steps.length).padStart(2, "0")}-${name}.png`);
      await page.settle(250);
      await page.screenshot(file);
      const c = await runCheck(page, { touch: cur.touch, safe: cur.safe }).catch((e) => ({ error: String(e), problems: [] }));
      res.steps.push({ step: name, shot: file, problems: (c.problems ?? []).filter((p) => p.type !== "small"), small: (c.problems ?? []).find((p) => p.type === "small")?.count ?? 0, error: c.error });
    },
  };
  try {
    await page.setDevice(d0, { theme });
    await api(base.url, "/api/machine/frame/stop", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ origin: "ui" }) });
    await execute(base.url, "doc.new");
    await j.reload();
    await J.run(j);
  } catch (e) {
    res.error = String(e.message ?? e);
    try {
      await j.shot("error-state");
    } catch {
      /* page gone */
    }
  } finally {
    if (j.clip) res.clipped = j.clip;
    res.consoleErrors = page.errors.slice(0, 10);
    await page.close();
  }
  return res;
}

function markdown(all) {
  const L = ["# Journeys", ""];
  for (const r of all) {
    L.push(`## ${r.id} (${r.device}, ${r.theme})${r.error ? ` ERROR: ${r.error}` : ""}`, "");
    for (const [k, v] of Object.entries(r.notes)) L.push(`- note ${k}: ${JSON.stringify(v).slice(0, 300)}`);
    for (const s of r.steps) {
      L.push(`- **${s.step}** \`${s.shot}\`${s.small ? ` (${s.small} small)` : ""}${s.error ? ` check error ${s.error}` : ""}`);
      for (const p of s.problems) L.push(`  - ${p.severity} ${p.type}: ${p.detail}`);
    }
    for (const c of r.clipped ?? []) L.push(`- clipped text: ${c}`);
    for (const c of r.consoleErrors ?? []) L.push(`- console: ${String(c).slice(0, 200)}`);
    L.push("");
  }
  return L.join("\n");
}

async function main() {
  const a = args(process.argv.slice(2));
  if (!a.url) {
    console.log("usage: node e2e/journeys.mjs --url http://127.0.0.1:7931 [--doc file] [--only a,b] [--out dir] [--cdp-port n]");
    process.exitCode = 2;
    return;
  }
  const url = a.url.replace(/\/$/, "");
  const out = resolve(a.out ?? join(here, "..", "..", "target", "ux-evidence", "journeys"));
  mkdirSync(out, { recursive: true });
  const only = a.only?.split(",").filter(Boolean);
  const list = JOURNEYS.filter((J) => !only || only.some((o) => J.id.includes(o)));
  const browser = await launch({ port: a.port ?? 9334 });
  const all = [];
  try {
    for (const J of list) {
      const t0 = Date.now();
      const r = await runJourney(browser, J, { url, out, doc: a.doc });
      all.push(r);
      const n = r.steps.reduce((k, s) => k + s.problems.length, 0);
      console.log(`${J.id}: ${r.steps.length} steps, ${n} problems${r.error ? `, ERROR ${r.error}` : ""} (${((Date.now() - t0) / 1000).toFixed(0)} s)`);
      writeFileSync(join(out, "journeys.json"), JSON.stringify(all, null, 1));
    }
  } finally {
    await browser.close();
  }
  writeFileSync(join(out, "journeys.md"), markdown(all));
  console.log(`report: ${join(out, "journeys.md")}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
