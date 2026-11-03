#!/usr/bin/env node
// Scripted flows for the device-emulation harness (web/e2e/README.md): every device of devices.mjs (and each of its
// themes) walks the same scenario in a fresh browser context, with a screenshot and a layout check (overlap.mjs) after
// every step. Results: <out>/<device>-<theme>/NN-step.png, <out>/report.json and <out>/report.md.
//
//   node e2e/scenarios.mjs --url http://127.0.0.1:7931 [--out dir] [--devices iphone,ipad] [--doc file.lbrn2|svg]
//                          [--theme dark|light|all] [--steps zoom,job] [--cdp-port 9333] [--headful]
//
// It needs a running OpenEtch engine with the simulator (never a real machine: the flows start and stop framing) and
// changes that engine's document. Without --doc it loads a generated heavy SVG.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, extname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, sleep } from "./cdp.mjs";
import { pickDevices } from "./devices.mjs";
import { runCheck } from "./overlap.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));

function args(argv) {
  const a = { theme: "device" };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--url") a.url = v();
    else if (k === "--out") a.out = v();
    else if (k === "--devices") a.devices = v();
    else if (k === "--doc") a.doc = v();
    else if (k === "--theme") a.theme = v();
    else if (k === "--steps") a.steps = v();
    else if (k === "--cdp-port") a.port = Number(v());
    else if (k === "--headful") a.headful = true;
    else if (k === "--help" || k === "-h") a.help = true;
  }
  return a;
}

// --- engine ------------------------------------------------------------------------------------------------------

export async function api(url, path, init) {
  const r = await fetch(`${url}${path}`, init);
  const text = await r.text();
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

export async function execute(url, id, params = {}) {
  return api(url, "/api/execute", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id, params, origin: "ui" }) });
}

/** A generated heavy document: rings of small paths and circles, a few hundred objects. */
function syntheticSvg() {
  const parts = [];
  for (let r = 0; r < 6; r++)
    for (let c = 0; c < 6; c++) {
      const cx = 12 + c * 22;
      const cy = 12 + r * 22;
      parts.push(`<circle cx="${cx}" cy="${cy}" r="9" fill="none" stroke="#f00"/>`);
      let d = "";
      for (let k = 0; k < 24; k++) {
        const a = (k / 24) * Math.PI * 2;
        const rr = k % 2 ? 4 : 7;
        d += `${k ? "L" : "M"}${(cx + rr * Math.cos(a)).toFixed(2)},${(cy + rr * Math.sin(a)).toFixed(2)}`;
      }
      parts.push(`<path d="${d}Z" fill="#00f"/>`);
      for (let k = 0; k < 4; k++) parts.push(`<rect x="${cx - 2 + k}" y="${cy - 1}" width="0.6" height="2" fill="#0a0"/>`);
    }
  return `<svg xmlns="http://www.w3.org/2000/svg" width="140mm" height="140mm" viewBox="0 0 140 140">${parts.join("")}</svg>`;
}

export async function loadHeavy(url, doc) {
  await execute(url, "doc.new");
  if (doc) {
    const ext = extname(doc).slice(1).toLowerCase();
    const fmt = ext === "lbrn2" || ext === "lbrn" ? "lbrn2" : ext === "dxf" ? "dxf" : "svg";
    const mode = fmt === "lbrn2" ? "&mode=replace" : "";
    const body = readFileSync(doc);
    return api(url, `/api/document/upload?format=${fmt}${mode}&name=${encodeURIComponent(basename(doc))}`, { method: "POST", body });
  }
  return api(url, `/api/document/upload?format=svg&name=heavy.svg`, { method: "POST", body: syntheticSvg() });
}

// --- page helpers ------------------------------------------------------------------------------------------------

/** Find a visible, hit-testable element by selector and label (exact, then prefix, then substring). */
export async function find(page, selector, text) {
  return page.evaluate(
    (sel, txt) => {
      const norm = (s) => (s || "").replace(/\s+/g, " ").trim().toLowerCase();
      const all = [...document.querySelectorAll(sel)].filter((e) => {
        const r = e.getBoundingClientRect();
        if (r.width < 1 || r.height < 1) return false;
        const cs = getComputedStyle(e);
        if (cs.visibility === "hidden" || cs.display === "none") return false;
        // Inside a closed <details> (Chromium keeps the boxes): not on screen.
        const closed = e.closest("details:not([open])");
        return !closed || !!closed.querySelector(":scope > summary")?.contains(e);
      });
      const lab = (e) => [norm(e.getAttribute("aria-label")), norm(e.textContent), norm(e.getAttribute("title"))];
      // Start and STOP are pressed only when the selector names them: a fuzzy label match must never reach them.
      const guarded = /btn-start|btn-stop/.test(sel);
      let c = guarded ? all : all.filter((e) => !e.closest(".btn-start, .btn-stop, .topbar-stop"));
      if (txt != null) {
        const pool = c;
        const t = norm(txt);
        c = pool.filter((e) => lab(e).some((l) => l === t));
        if (!c.length) c = pool.filter((e) => lab(e).some((l) => l.startsWith(t)));
        if (!c.length) c = pool.filter((e) => lab(e).some((l) => l.includes(t)));
      }
      const hit = (e) => {
        const r = e.getBoundingClientRect();
        const x = Math.min(Math.max(r.left + r.width / 2, 0), innerWidth - 1);
        const y = Math.min(Math.max(r.top + r.height / 2, 0), innerHeight - 1);
        const h = document.elementFromPoint(x, y);
        return !!h && (h === e || e.contains(h));
      };
      const best = c.find(hit) ?? c[0];
      if (!best) return null;
      // Scrolled out of its panel: bring it into view first (as a person would scroll to it).
      if (!hit(best)) best.scrollIntoView({ block: "center", inline: "nearest" });
      const r = best.getBoundingClientRect();
      const ok = hit(best);
      let cover = null;
      if (!ok) {
        const h = document.elementFromPoint(Math.min(Math.max(r.left + r.width / 2, 0), innerWidth - 1), Math.min(Math.max(r.top + r.height / 2, 0), innerHeight - 1));
        cover = h ? `${h.tagName.toLowerCase()}.${String(h.className?.baseVal ?? h.className).split(" ")[0]} "${(h.getAttribute("aria-label") || h.textContent || "").trim().slice(0, 30)}"` : "nothing";
      }
      return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height, hit: ok, cover };
    },
    selector,
    text ?? null,
  );
}

/** Open a menu path: the first label is a button, the rest menu items (submenus open on press). */
export async function menuPath(page, labels) {
  await pressFind(page, "button", labels[0]);
  for (const l of labels.slice(1)) {
    await sleep(350);
    await pressFind(page, "[role=menuitem], .menu-item", l);
  }
  await sleep(500);
}

export async function pressFind(page, selector, text) {
  const c = await find(page, selector, text);
  if (!c) throw new Error(`not found: ${selector}${text ? ` "${text}"` : ""}`);
  // Never tap blindly: a control under something else would send the tap to whatever covers it.
  if (!c.hit) throw new Error(`not hit-testable: ${selector}${text ? ` "${text}"` : ""} is under ${c.cover}`);
  await page.press(c.x, c.y);
  return c;
}

/** Field millimetres to viewport px, from the rulers' major ticks. */
export async function mmToPx(page) {
  return page.evaluate(() => {
    const ticks = (sel, axis) => {
      const out = [];
      for (const g of document.querySelectorAll(`${sel} svg g`)) {
        const line = g.querySelector("line.major");
        const text = g.querySelector("text");
        if (!line || !text) continue;
        const v = parseFloat(text.textContent.replace("−", "-"));
        if (!Number.isFinite(v)) continue;
        const r = line.getBoundingClientRect();
        out.push([v, axis === "x" ? r.left : r.top]);
      }
      return out;
    };
    const fit = (t) => {
      if (t.length < 2) return null;
      const [a, b] = [t[0], t[t.length - 1]];
      const k = (b[1] - a[1]) / (b[0] - a[0]);
      return { k, o: a[1] - k * a[0] };
    };
    const fx = fit(ticks(".et-ruler-top", "x"));
    const fy = fit(ticks(".et-ruler-left", "y"));
    const vp = document.querySelector(".viewport")?.getBoundingClientRect();
    return { fx, fy, vp: vp ? { x: vp.left, y: vp.top, w: vp.width, h: vp.height } : null };
  });
}

export async function canvasPoint(page, xmm, ymm) {
  const m = await mmToPx(page);
  if (!m.fx || !m.fy) {
    if (!m.vp) throw new Error("no canvas");
    return { x: m.vp.x + m.vp.w / 2, y: m.vp.y + m.vp.h / 2 };
  }
  return { x: m.fx.k * xmm + m.fx.o, y: m.fy.k * ymm + m.fy.o };
}

/** A point on the bare canvas (nothing floating over it), near the middle of the view. */
export async function canvasCenter(page) {
  const p = await page.evaluate(() => {
    const vp = document.querySelector(".viewport");
    if (!vp) return null;
    const r = vp.getBoundingClientRect();
    for (const fy of [0.45, 0.35, 0.55, 0.25, 0.65])
      for (const fx of [0.5, 0.4, 0.6, 0.3, 0.7]) {
        const x = r.left + r.width * fx;
        const y = r.top + r.height * fy;
        const e = document.elementFromPoint(x, y);
        if (e && (e.matches("svg.vp-main, canvas.vp-layer") || e.closest("svg.vp-main"))) return { x, y };
      }
    return { x: r.left + r.width / 2, y: r.top + r.height * 0.45 };
  });
  if (!p) throw new Error("no canvas");
  return p;
}

export async function applySafeArea(page, safe) {
  await page.evaluate((s) => {
    let st = document.getElementById("e2e-safe-area");
    if (!st) {
      st = document.createElement("style");
      st.id = "e2e-safe-area";
      document.head.appendChild(st);
    }
    st.textContent = `:root{--safe-top:${s.top}px !important;--safe-right:${s.right}px !important;--safe-bottom:${s.bottom}px !important;--safe-left:${s.left}px !important;}`;
  }, safe);
}

/** Escape until no menu, modal dialog or drawer is open (the harness's housekeeping, also on touch devices); a drawer
 *  that stays gets a tap on its scrim. */
export async function escape(page) {
  const layers = () =>
    page.evaluate(() =>
      [...document.querySelectorAll("[role=menu], .ctx-menu, [role=dialog][aria-modal=true], .modal-backdrop, .drawer-scrim")].some(
        (e) => e.getBoundingClientRect().width > 0,
      ),
    );
  for (let i = 0; i < 6; i++) {
    await page.key("Escape");
    await sleep(150);
    if (!(await layers()) && i >= 1) return;
  }
  const scrim = await find(page, ".drawer-scrim");
  if (scrim) {
    await page.press(Math.max(scrim.x - scrim.w / 2 + 8, 1), scrim.y);
    await sleep(300);
  }
}

/** Open a collapsed section of the phone's More sheet. */
export async function openSection(page, title) {
  const open = await page.evaluate((t) => {
    const s = [...document.querySelectorAll(".phone-more details > summary")].find((x) => x.textContent.trim().toLowerCase().startsWith(t.toLowerCase()));
    return s ? s.parentElement.open : null;
  }, title);
  if (open === null) throw new Error(`no More section "${title}"`);
  if (!open) {
    await pressFind(page, ".phone-more summary", title);
    await sleep(300);
  }
}

export async function moreSheet(page) {
  const more = await page.evaluate(() => !!document.querySelector(".phone-more"));
  if (!more) await phoneTab(page, "More");
  if (!(await page.evaluate(() => !!document.querySelector(".phone-more")))) await phoneTab(page, "More");
  await sheetTo(page, "full");
}

export async function warnText(page) {
  return page.evaluate(() => document.querySelector(".vp-warn")?.textContent?.trim() ?? null);
}

/** Tap the phone sheet's handle until it is at `snap` (peek, half, full). */
export async function sheetTo(page, snap) {
  for (let i = 0; i < 4; i++) {
    const cur = await page.evaluate(() => {
      const s = document.querySelector(".bottom-sheet");
      if (!s) return null;
      const m = s.className.match(/snap-(\w+)/);
      return m ? m[1] : null;
    });
    if (cur === snap) return true;
    if (cur === null) return false;
    await pressFind(page, ".sheet-handle");
    await sleep(450);
  }
  return false;
}

export async function phoneTab(page, name) {
  await pressFind(page, "button.phone-tab", name);
  await sleep(500);
}

export async function sheetOpen(page) {
  return page.evaluate(() => !!document.querySelector(".bottom-sheet"));
}

// --- the scenario ------------------------------------------------------------------------------------------------

// Target positions in field mm (tried in order until something is selected): a 2 x 3 jig layout centred near the
// origin, and the generated SVG.
export const TARGETS = [
  [-11.9, -9.9],
  [28.1, -9.9],
  [-51.9, -9.9],
  [0, 0],
];

export async function selectSomething(page) {
  for (const [x, y] of TARGETS) {
    const p = await canvasPoint(page, x, y);
    await page.press(p.x, p.y);
    await sleep(500);
    const n = await selectionCount(page);
    if (n > 0) return { at: [x, y], px: p, n };
  }
  return null;
}

/** Objects selected on the canvas: the selection box the viewport draws (0 or 1, enough for the flows). */
export async function selectionCount(page) {
  return page.evaluate(() => (document.querySelector(".viewport .sel-box, .viewport .sel-bounds") ? 1 : 0));
}

function stepsFor(d) {
  const S = [];
  const add = (name, fn, kinds = ["phone", "tablet", "desktop"]) => {
    if (kinds.includes(d.kind)) S.push({ name, fn });
  };
  const touch = d.touch;

  add("empty", async () => {});
  add("heavy-fit", async ({ page, ctx }) => {
    ctx.upload = await loadHeavy(ctx.url, ctx.doc);
    await sleep(2500);
    const f = await find(page, "button", "Fit field");
    if (f) await page.press(f.x, f.y);
    await sleep(500);
  });

  // Desktop: workspaces, menus, dialogs, Cut settings.
  add("workspace-laser", async ({ page }) => pressFind(page, ".workspaces button, nav[role=tablist] button", "Laser"), ["desktop", "tablet"]);
  add("workspace-design", async ({ page }) => pressFind(page, ".workspaces button, nav[role=tablist] button", "Design"), ["desktop", "tablet"]);
  add("menu-file", async ({ page }) => pressFind(page, "button", "File"), ["desktop", "tablet"]);
  add("dialog-devices", async ({ page }) => {
    await escape(page);
    await menuPath(page, ["File", "Laser", "Devices"]);
  }, ["desktop", "tablet"]);
  add("menu-edit", async ({ page }) => {
    await escape(page);
    await pressFind(page, ".ribbon button, .toolbar button, button.tool-btn", "Edit");
  }, ["desktop"]);
  add("menu-window", async ({ page }) => {
    await escape(page);
    await pressFind(page, "button", "Window");
  }, ["desktop"]);
  add("dialog-preferences", async ({ page }) => {
    await escape(page);
    await menuPath(page, ["File", "Settings", "Preferences"]);
  }, ["desktop", "tablet"]);
  add("cut-settings", async ({ page }) => {
    await escape(page);
    let row = await find(page, "tr.cuts-row");
    if (!row && d.kind === "tablet") {
      await pressFind(page, ".dock-rail-btn", "Cuts / Layers");
      await sleep(500);
      row = await find(page, "tr.cuts-row");
    }
    if (!row) throw new Error("no Cuts / Layers row");
    // A spot on the row that is not one of its buttons (its centre can be the ⋯ menu button).
    const spot = await page.evaluate(() => {
      const tr = document.querySelector("tr.cuts-row");
      if (!tr) return null;
      const r = tr.getBoundingClientRect();
      for (let f = 0.15; f < 0.95; f += 0.05) {
        const x = r.left + r.width * f;
        const y = r.top + r.height / 2;
        const h = document.elementFromPoint(x, y);
        if (h && tr.contains(h) && !h.closest("button, input, select, [role=button]")) return { x, y };
      }
      return null;
    });
    if (spot) Object.assign(row, spot);
    if (touch) {
      await page.tap(row.x, row.y);
      await sleep(80);
      await page.tap(row.x, row.y);
    } else await page.click(row.x, row.y, { clickCount: 2 });
    await sleep(600);
  }, ["desktop", "tablet"]);

  // Tablet: rail groups, dock overlays.
  for (const g of ["Modify", "Arrange", "Insert", "Inspect", "Laser Tools"])
    add(`rail-${g.toLowerCase().replace(/\s+/g, "-")}`, async ({ page }) => {
      await escape(page);
      await pressFind(page, ".ribbon-group-btn", g);
    }, ["tablet"]);
  for (const o of ["Cuts / Layers", "Shape Properties"])
    add(`overlay-${o.toLowerCase().replace(/[^a-z]+/g, "-")}`, async ({ page }) => {
      await escape(page);
      await pressFind(page, ".dock-rail-btn", o);
      await sleep(400);
    }, ["tablet"]);

  // Phone: the four tabs and their sheets at peek / half / full.
  for (const tab of ["Canvas", "Layers", "Job", "More"])
    for (const snap of ["peek", "half", "full"])
      add(`tab-${tab.toLowerCase()}-${snap}`, async ({ page }) => {
        const active = await page.evaluate(() => document.querySelector(".phone-tab.active")?.textContent?.trim());
        if (active !== tab || !(await sheetOpen(page))) await phoneTab(page, tab);
        if (!(await sheetOpen(page))) await phoneTab(page, tab);
        if (!(await sheetTo(page, snap))) throw new Error(`sheet did not reach ${snap}`);
      }, ["phone"]);
  add("phone-dialog-preferences", async ({ page }) => {
    await moreSheet(page);
    await openSection(page, "Settings");
    await pressFind(page, ".phone-more button, .phone-more [role=button], .phone-more a", "Preferences");
    await sleep(500);
  }, ["phone"]);
  add("phone-dialog-devices", async ({ page }) => {
    await escape(page);
    await moreSheet(page);
    await openSection(page, "Desktop features");
    await pressFind(page, ".phone-more button, .phone-more [role=button], .phone-more a", "Devices");
    await sleep(500);
  }, ["phone"]);
  add("phone-close-sheets", async ({ page }) => {
    await escape(page);
    await phoneTab(page, "Canvas");
    if (await sheetOpen(page)) await pressFind(page, ".sheet-close");
  }, ["phone"]);

  // Every device: selection, tools, context menu, framing, off-field warning, zoom, keyboard.
  add("select", async ({ page, ctx }) => {
    await escape(page);
    if (d.kind === "phone" && (await sheetOpen(page))) await pressFind(page, ".sheet-close");
    ctx.sel = await selectSomething(page);
    if (!ctx.sel) throw new Error("nothing got selected");
  });
  // The reported phone case: Job tab, sheet closed, a selection, the off-field warning.
  const jobClosed = async ({ page }) => {
    await escape(page);
    await phoneTab(page, "Job");
    if (await sheetOpen(page)) await phoneTab(page, "Job");
    if (await sheetOpen(page)) await pressFind(page, ".sheet-close");
    // The job preview (and its player row) takes a moment for a heavy document.
    await page.waitFor(".pv-timeline", { timeout: 8000 });
    await sleep(300);
  };
  add("job-closed-with-selection", jobClosed, ["phone"]);
  add("properties-pill", async ({ page }) => {
    await pressFind(page, ".properties-pill");
    await sleep(400);
  }, ["tablet"]);
  add("tool-rectangle", async ({ page }) => {
    await escape(page);
    if (d.kind === "phone") {
      await phoneTab(page, "Canvas");
      if (!(await sheetOpen(page))) await phoneTab(page, "Canvas");
    }
    await pressFind(page, "button", "Rectangle");
    await sleep(300);
    if (d.kind === "phone" && (await sheetOpen(page))) await pressFind(page, ".sheet-close");
  });
  add("tool-rectangle-drawing", async ({ page }) => {
    const a = await canvasPoint(page, 40, 50);
    const b = await canvasPoint(page, 60, 35);
    await page.drag(a.x, a.y, b.x, b.y, { steps: 8 });
  });
  add("tool-line", async ({ page }) => {
    await escape(page);
    if (d.kind === "phone") {
      await phoneTab(page, "Canvas");
      if (!(await sheetOpen(page))) await phoneTab(page, "Canvas");
    }
    await pressFind(page, "button", "Line");
    await sleep(300);
    if (d.kind === "phone" && (await sheetOpen(page))) await pressFind(page, ".sheet-close");
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
    const [x, y] = ctx.sel?.at ?? TARGETS[0];
    const p = await canvasPoint(page, x, y);
    if (touch) await page.longPress(p.x, p.y, 750);
    else await page.click(p.x, p.y, { button: "right" });
    await sleep(400);
  });
  add("framing-start", async ({ page }) => {
    await escape(page);
    if (d.kind === "phone") {
      await phoneTab(page, "Job");
      if (!(await sheetOpen(page))) await phoneTab(page, "Job");
    } else {
      await pressFind(page, ".workspaces button, nav[role=tablist] button", "Laser");
      await sleep(600);
    }
    await pressFind(page, "button", "Frame");
    await sleep(1500);
  });
  add("framing-stop", async ({ page }) => {
    let b = await find(page, "button", "Stop frame");
    if ((!b || !b.hit) && d.kind === "phone") {
      await phoneTab(page, "Job");
      if (!(await sheetOpen(page))) await phoneTab(page, "Job");
      b = await find(page, "button", "Stop frame");
    }
    if (!b) throw new Error("no Stop frame button");
    if (!b.hit) throw new Error(`Stop frame is not hit-testable (under ${b.cover})`);
    await page.press(b.x, b.y);
    await sleep(800);
  });
  add("off-field", async ({ page, ctx }) => {
    await escape(page);
    if (d.kind === "phone") {
      await phoneTab(page, "Canvas");
      if (await sheetOpen(page)) await pressFind(page, ".sheet-close");
    } else {
      await pressFind(page, ".workspaces button, nav[role=tablist] button", "Design");
      await sleep(400);
    }
    const before = await warnText(page);
    // Select the right-hand object and drag it 45 mm to the right, past the field edge.
    const [x, y] = TARGETS[1];
    const p0 = await canvasPoint(page, x, y);
    await page.press(p0.x, p0.y);
    await sleep(700);
    // Selecting may pan the view (the phone reveals the selection above its sheet): measure again.
    const p = await canvasPoint(page, x, y);
    const q = await canvasPoint(page, x + 45, y);
    await page.drag(p.x, p.y, q.x, q.y, { steps: 14 });
    await sleep(900);
    ctx.offField = { before, after: await warnText(page) };
  });
  add("job-closed-off-field", jobClosed, ["phone"]);
  // A pinch that starts on the app's chrome (the touch modifier bar, else the tab bar or rail) instead of the canvas.
  add("pinch-on-chrome", async ({ page, ctx }) => {
    if (d.kind === "phone") {
      await phoneTab(page, "Canvas");
      if (await sheetOpen(page)) await pressFind(page, ".sheet-close");
    }
    const t = (await find(page, ".touch-mods")) ?? (await find(page, ".phone-tabs")) ?? (await find(page, ".ribbon-group-btn"));
    if (!t) throw new Error("no chrome to pinch on");
    await page.pinch(t.x, t.y, 40, 160);
    await sleep(400);
    ctx.chromePinchScale = await page.evaluate(() => window.visualViewport?.scale ?? 1);
  }, ["phone", "tablet"]);
  add("zoom-in", async ({ page }) => {
    if (d.kind === "phone") {
      await phoneTab(page, "Canvas");
      if (await sheetOpen(page)) await pressFind(page, ".sheet-close");
    }
    const c = await canvasCenter(page);
    if (touch) {
      await page.pinch(c.x, c.y, 60, 200);
      await sleep(300);
      await page.pinch(c.x, c.y, 60, 160);
    } else {
      for (let i = 0; i < 4; i++) {
        await page.wheel(c.x, c.y, -240);
        await sleep(60);
      }
    }
  });
  add("zoom-out", async ({ page }) => {
    const c = await canvasCenter(page);
    if (touch) {
      for (let i = 0; i < 3; i++) {
        await page.pinch(c.x, c.y, 240, 60);
        await sleep(250);
      }
    } else {
      for (let i = 0; i < 10; i++) {
        await page.wheel(c.x, c.y, 240);
        await sleep(60);
      }
    }
  });
  // The user guide (src/guide/): opened as a person would (F1 on a desktop, More > User guide on a tablet, the More
  // tab's Help section on a phone), then a search, a result with its anchor, and Close.
  add("guide-open", async ({ page }) => {
    await escape(page);
    if (d.kind === "phone") {
      await moreSheet(page);
      await openSection(page, "Help");
      await pressFind(page, ".phone-more button, .phone-more [role=menuitem]", "User guide");
    } else if (d.kind === "tablet") {
      await pressFind(page, "button", "More");
      await sleep(350);
      await pressFind(page, "[role=menuitem], .menu-item", "User guide");
    } else {
      const c = await canvasCenter(page);
      await page.mouse("mouseMoved", c.x, c.y, { button: "none" });
      await page.key("F1", { code: "F1" });
    }
    if (!(await page.waitFor(".guide-md h1", { timeout: 8000 }))) throw new Error("the guide did not open");
  });
  add("guide-search", async ({ page }) => {
    const f = await find(page, ".guide-search input");
    if (!f) throw new Error("no guide search field");
    await page.press(f.x, f.y);
    await page.type("energy density");
    await sleep(300);
    await page.evaluate(() => document.activeElement?.blur?.());
    if (!(await page.waitFor(".guide-hit", { timeout: 3000 }))) throw new Error("no search results");
  });
  add("guide-page", async ({ page }) => {
    await pressFind(page, ".guide-hit", "Energy density");
    if (!(await page.waitFor(".guide-table", { timeout: 4000 }))) throw new Error("the example page did not show");
    await sleep(300);
  });
  add("guide-contents", async ({ page }) => {
    await pressFind(page, ".guide-bar button", "Contents");
    if (!(await page.waitFor(".guide-toc", { timeout: 3000 }))) throw new Error("no contents");
  });
  add("guide-close", async ({ page }) => {
    await pressFind(page, ".guide-bar button", "Close the guide");
    await sleep(400);
    if (await page.evaluate(() => !!document.querySelector(".guide"))) throw new Error("the guide is still open");
  });
  add("keyboard-input", async ({ page, ctx }) => {
    // A focused numeric field with the on-screen keyboard up (the visual viewport shrinks by the keyboard height).
    const f = await find(page, "button", "Fit field");
    if (f) await page.press(f.x, f.y);
    await sleep(300);
    if (!(await selectionCount(page))) ctx.sel = await selectSomething(page);
    if (d.kind === "phone") {
      await phoneTab(page, "Canvas");
      if (!(await sheetOpen(page))) await phoneTab(page, "Canvas");
      await sheetTo(page, "half");
    } else if (d.kind === "tablet") {
      const pill = await find(page, ".properties-pill");
      if (pill) await page.press(pill.x, pill.y);
      await sleep(400);
    }
    const inp = await find(page, ".bottom-sheet input[type=text], .bottom-sheet input:not([type]), .bottom-sheet input[inputmode], .dock-overlay input, input[inputmode=decimal], input.num, input[type=number], input[type=text]");
    if (!inp) throw new Error("no text input to focus");
    await page.press(inp.x, inp.y);
    await sleep(200);
    await page.setViewportHeight(d.height - (d.keyboard ?? 300));
    // The keyboard covers the home indicator: no bottom safe-area band while it is up.
    if (d.safe) {
      ctx.safeNow = { ...d.safe, bottom: 0 };
      await applySafeArea(page, ctx.safeNow);
    }
    await sleep(600);
  }, ["phone", "tablet"]);
  return S;
}

async function runDevice(browser, d, theme, ctxBase) {
  const dir = join(ctxBase.out, `${d.id}-${theme}`);
  mkdirSync(dir, { recursive: true });
  const page = await browser.newPage();
  const results = [];
  const ctx = { ...ctxBase };
  try {
    await page.setDevice(d, { theme });
    await api(ctx.url, "/api/machine/frame/stop", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ origin: "ui" }) });
    await execute(ctx.url, "doc.new");
    await page.goto(ctx.url + "/", { waitFor: ".btn-stop", timeout: 20000 });
    await sleep(1500);
    if (d.safe && Object.values(d.safe).some((v) => v > 0)) await applySafeArea(page, d.safe);
    const check = async (name, extra = {}) => {
      const shot = join(dir, `${String(results.length).padStart(2, "0")}-${name}.png`);
      await page.settle(250);
      await page.screenshot(shot);
      const c = await runCheck(page, { touch: d.touch, safe: ctx.safeNow ?? d.safe }).catch((e) => ({ error: String(e), problems: [] }));
      results.push({ step: name, shot, ...extra, layout: c.layout, counts: c.counts, problems: c.problems, error: extra.error ?? c.error });
    };
    await check("first-visit");
    // The "Use as an app" offer on the first touch visit.
    const close = await find(page, ".modal-foot button, [role=dialog] button", "Close");
    if (close) {
      await page.press(close.x, close.y);
      await sleep(400);
    }
    const steps = stepsFor(d).filter((s) => !ctx.steps || ctx.steps.some((f) => s.name.includes(f)) || s.name === "heavy-fit" || s.name === "empty");
    for (const s of steps) {
      let error;
      try {
        await s.fn({ page, ctx, d });
      } catch (e) {
        error = String(e.message ?? e);
      }
      await sleep(250);
      await check(s.name, { error });
      // A page zoom left by a step (reported by the check) would spoil the next steps: undo it.
      if (await page.evaluate(() => Math.abs((window.visualViewport?.scale ?? 1) - 1) > 0.01)) {
        await page.send("Emulation.setPageScaleFactor", { pageScaleFactor: 1 });
        await sleep(300);
      }
      if (s.name === "keyboard-input") {
        await page.setViewportHeight(d.height);
        await page.evaluate(() => document.activeElement?.blur?.());
        if (ctx.safeNow) {
          ctx.safeNow = undefined;
          await applySafeArea(page, d.safe);
        }
      }
    }
    if (page.errors.length) results.push({ step: "console-errors", errors: page.errors.slice(0, 20), problems: [] });
  } finally {
    await page.close();
  }
  return { device: d.id, name: d.name, theme, dir, offField: ctx.offField ?? null, chromePinchScale: ctx.chromePinchScale ?? null, results };
}

function markdown(runs, out) {
  const lines = ["# Device harness report", ""];
  for (const r of runs) {
    lines.push(`## ${r.name} (${r.device}, ${r.theme})`, "");
    if (r.offField) lines.push(`Off-field warning: before "${r.offField.before ?? "-"}", after "${r.offField.after ?? "-"}"`, "");
    for (const s of r.results) {
      const probs = (s.problems ?? []).filter((p) => p.type !== "small");
      const small = (s.problems ?? []).find((p) => p.type === "small");
      lines.push(`- **${s.step}** ${s.shot ? `\`${s.shot}\`` : ""}${s.error ? ` ERROR: ${s.error}` : ""}${small ? ` (${small.count} small targets)` : ""}`);
      for (const p of probs) lines.push(`  - ${p.severity} ${p.type}: ${p.detail}`);
      if (s.errors) for (const e of s.errors) lines.push(`  - console: ${String(e).slice(0, 200)}`);
    }
    lines.push("");
  }
  return lines.join("\n");
}

async function main() {
  const a = args(process.argv.slice(2));
  if (a.help || !a.url) {
    console.log(readFileSync(join(here, "README.md"), "utf8"));
    if (!a.url) process.exitCode = a.help ? 0 : 2;
    return;
  }
  const url = a.url.replace(/\/$/, "");
  const run = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const out = resolve(a.out ?? join(here, "..", "..", "target", "ux-evidence", `devices-${run}`));
  mkdirSync(out, { recursive: true });
  const devices = pickDevices(a.devices);
  const browser = await launch({ port: a.port ?? 9333, headless: !a.headful });
  const runs = [];
  try {
    for (const d of devices) {
      const themes = a.theme === "all" ? ["dark", "light"] : a.theme === "device" ? d.themes : [a.theme];
      for (const theme of themes) {
        const t0 = Date.now();
        const r = await runDevice(browser, d, theme, { url, out, doc: a.doc, steps: a.steps?.split(",") });
        runs.push(r);
        const n = r.results.reduce((k, s) => k + (s.problems ?? []).filter((p) => p.type !== "small").length, 0);
        const errs = r.results.filter((s) => s.error).length;
        console.log(`${d.id} ${theme}: ${r.results.length} steps, ${n} problems, ${errs} step errors (${((Date.now() - t0) / 1000).toFixed(0)} s)`);
        writeFileSync(join(out, "report.json"), JSON.stringify(runs, null, 1));
      }
    }
  } finally {
    await browser.close();
  }
  writeFileSync(join(out, "report.md"), markdown(runs, out));
  console.log(`report: ${join(out, "report.md")}`);
}

// Run only as a script: journeys.mjs imports the helpers above.
if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url)))
  main().catch((e) => {
    console.error(e);
    process.exit(1);
  });
