#!/usr/bin/env node
// The design-only worker shell (src/standalone/) in real browsers: builds the check page (e2e/standalone/, the real
// worker with the mock webcore), serves it on 127.0.0.1 and runs one scenario in two tabs of one browser profile:
// IndexedDB storage, save / list / thumbnail, the BroadcastChannel refresh, the two-tab 409, a reload bringing the
// unsaved design back, a webcore panic restarting the worker, a 4 MB project, a font, the download fallback.
//
//   node e2e/standalone.mjs [--out dir] [--port 7993] [--cdp-port 9341]          headless Edge (or Chrome)
//   node e2e/standalone.mjs --safari <ssh host> [--out dir] [--port 7993]         Safari on a Mac, over ssh
//
// Safari: the page is copied to a temporary folder on the Mac and served there by `python3 -m http.server` on the
// Mac's 127.0.0.1; safaridriver (Safari > Develop > Allow Remote Automation) is reached through an ssh tunnel.
// Nothing is left on the Mac afterwards. The output folder (default: the system temp folder) gets a screenshot and
// report.json; nothing is written into the repository.

import { spawn, execFileSync } from "node:child_process";
import { createReadStream, existsSync, mkdirSync, statSync, writeFileSync, readdirSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, Page, sleep } from "./cdp.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));
const web = resolve(here, "..");

function args(argv) {
  const a = { port: 7993, cdpPort: 9341 };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const v = () => argv[++i];
    if (k === "--out") a.out = v();
    else if (k === "--port") a.port = Number(v());
    else if (k === "--cdp-port") a.cdpPort = Number(v());
    else if (k === "--safari") a.safari = v();
  }
  a.out ??= join(tmpdir(), `oe-ds-shell-${Date.now()}`);
  return a;
}

async function buildPage(outDir) {
  process.env.VITE_STANDALONE_MOCK = "1";
  const { build } = await import("vite");
  const root = join(here, "standalone");
  await build({
    root,
    base: "./",
    configFile: false,
    logLevel: "warn",
    resolve: { alias: { "webcore-pkg": join(root, "no-webcore.js") } },
    worker: { format: "es" },
    build: { outDir, emptyOutDir: true, sourcemap: false, target: "es2022" },
  });
}

const MIME = { ".html": "text/html; charset=utf-8", ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".json": "application/json" };

function serve(dir, port) {
  const server = createServer((req, res) => {
    const p = decodeURIComponent(new URL(req.url, "http://x").pathname);
    const file = join(dir, p === "/" ? "index.html" : p);
    if (!file.startsWith(dir) || !existsSync(file) || !statSync(file).isFile()) {
      res.writeHead(404).end();
      return;
    }
    res.writeHead(200, { "content-type": MIME[extname(file)] ?? "application/octet-stream", "cache-control": "no-store" });
    createReadStream(file).pipe(res);
  });
  return new Promise((r) => server.listen(port, "127.0.0.1", () => r(server)));
}

// --- The scenario, for any driver: open() gives a tab { ev(fn, ...args), goto(url), reload(), shot(path) } --------

async function scenario(open, url, out, browser) {
  const results = [];
  const check = (name, ok, detail) => {
    results.push({ name, ok: !!ok, detail });
    console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail !== undefined ? `  ${typeof detail === "string" ? detail : JSON.stringify(detail)}` : ""}`);
  };
  const ready = async (t) => {
    for (let i = 0; i < 100; i++) {
      if (await t.ev(() => window.__dsReady === true).catch(() => false)) return true;
      await sleep(100);
    }
    return false;
  };
  const f = (t, method, path, body) => t.ev((m, p, b) => window.__ds.fetch(m, p, b ?? undefined), method, path, body ?? null);
  const until = async (fn, ms = 4000) => {
    const t0 = Date.now();
    while (Date.now() - t0 < ms) {
      if (await fn()) return true;
      await sleep(100);
    }
    return false;
  };

  const A = await open();
  await A.goto(url);
  check("page A starts the worker", await ready(A));
  check("storage is IndexedDB", (await A.ev(() => window.__ds.storage())) === "indexeddb");
  check("file pickers (File System Access)", true, (await A.ev(() => window.__ds.canPick())) ? "available" : "not here: downloads");

  await f(A, "POST", "/api/execute", { id: "shape.rect" });
  const save = await f(A, "POST", "/api/projects/save", { name: "Alpha" });
  check("save Alpha", save.status === 200 && save.json?.ok, save.status);
  const list = await f(A, "GET", "/api/projects");
  check("list has Alpha", list.json?.some?.((p) => p.name === "Alpha"));
  const thumb = await f(A, "GET", "/api/projects/Alpha/thumbnail.png?width=128");
  check("thumbnail is a PNG", thumb.status === 200 && thumb.ct === "image/png" && thumb.size > 0, thumb.ct);

  const B = await open();
  await B.goto(url);
  check("page B starts its own worker", await ready(B));
  check("B lists A's project (shared IndexedDB)", (await f(B, "GET", "/api/projects")).json?.some?.((p) => p.name === "Alpha"));
  await B.ev(() => void (window.__ds.events.length = 0));
  await f(A, "POST", "/api/projects/save", { name: "Beta" });
  check(
    "B hears A's save (BroadcastChannel)",
    await until(() => B.ev(() => window.__ds.events.includes("stores_changed"))),
  );

  check("B opens Alpha", (await f(B, "POST", "/api/projects/open", { name: "Alpha" })).status === 200);
  await f(A, "POST", "/api/projects/open", { name: "Alpha" });
  await f(A, "POST", "/api/execute", { id: "shape.rect" });
  check("A saves Alpha", (await f(A, "POST", "/api/projects/save", { name: "Alpha" })).status === 200);
  await f(B, "POST", "/api/execute", { id: "shape.ellipse" });
  const stale = await f(B, "POST", "/api/projects/save", { name: "Alpha" });
  check("B's stale save of Alpha is refused (409 changed_elsewhere)", stale.status === 409 && stale.json?.code === "changed_elsewhere", stale.json?.error);
  check("B saves with overwrite after asking", (await f(B, "POST", "/api/projects/save", { name: "Alpha", overwrite: true })).status === 200);

  await f(A, "POST", "/api/execute", { id: "doc.new" });
  for (let i = 0; i < 3; i++) await f(A, "POST", "/api/execute", { id: "shape.rect" });
  await A.ev(() => window.__ds.flush());
  await sleep(400);
  await A.reload();
  check("A after a reload", await ready(A));
  const rec = await A.ev(() => window.__ds.recovered());
  check("the reload brought the unsaved design back", rec?.source === "recovery", rec);
  check("with 3 objects", (await f(A, "GET", "/api/document")).json?.objects?.length === 3);
  check("and said so", await A.ev(() => window.__ds.notices.some((n) => n.key === "ds.reloadRecovered")));
  const p = (await f(A, "GET", "/api/project")).json;
  check("it is still unsaved", p?.dirty === true, p);

  await A.ev(() => window.__ds.flush());
  await sleep(300);
  const crash = await f(A, "POST", "/api/_mock/panic");
  check("a webcore panic answers 500 editor_crashed", crash.status === 500 && crash.json?.code === "editor_crashed");
  const after = await f(A, "GET", "/api/document");
  check("the restarted worker answers, design intact", after.status === 200 && after.json?.objects?.length === 3, after.status);
  check("and the restart notice", await until(() => A.ev(() => window.__ds.notices.some((n) => n.key === "ds.restartedRecovered"))));

  const big = await A.ev((n) => window.__ds.uploadBig(n), 60000);
  check("a 4 MB project uploads", big.status === 200, `${(big.bytes / 1e6).toFixed(1)} MB`);
  const t0 = Date.now();
  const bigSave = await f(A, "POST", "/api/projects/save", { name: "Big" });
  check("and saves to IndexedDB", bigSave.status === 200, `${Date.now() - t0} ms`);
  const t1 = Date.now();
  const bigOpen = await f(B, "POST", "/api/projects/open", { name: "Big" });
  check("B opens it", bigOpen.status === 200, `${Date.now() - t1} ms`);
  check("with all objects", (await f(B, "GET", "/api/document")).json?.objects?.length === 60000);

  const ttf = await A.ev(async () => {
    const b = new Uint8Array(64);
    b.set([0, 1, 0, 0]);
    const r = await window.__ds.fetch("POST", "/api/fonts/user?name=Check.ttf", new TextDecoder("latin1").decode(b));
    return r.status;
  });
  check("a font is stored", ttf === 200, ttf);
  const st = await f(A, "GET", "/api/storage");
  check("storage estimate", st.status === 200, st.json);
  check("a download link starts without an error", await A.ev(() => window.__ds.download()));

  await A.ev((rs) => {
    for (const r of rs) window.__ds.note(`${r.name}${r.detail !== undefined ? ` (${typeof r.detail === "string" ? r.detail : JSON.stringify(r.detail)})` : ""}`, r.ok);
  }, results);
  await A.shot(join(out, `${browser}.png`));
  return results;
}

// --- Edge / Chrome over CDP ---------------------------------------------------------------------------------------

async function cdpTab(conn, contextId) {
  const { targetId } = await conn.send("Target.createTarget", { url: "about:blank", browserContextId: contextId });
  const { sessionId } = await conn.send("Target.attachToTarget", { targetId, flatten: true });
  const p = new Page(conn, sessionId, targetId, contextId);
  await p.send("Page.enable");
  await p.send("Runtime.enable");
  return p;
}

async function runEdge(a, url) {
  const b = await launch({ port: a.cdpPort });
  try {
    const { browserContextId } = await b.conn.send("Target.createBrowserContext", { disposeOnDetach: true });
    const open = async () => {
      const p = await cdpTab(b.conn, browserContextId);
      await p.send("Emulation.setDeviceMetricsOverride", { width: 900, height: 1300, deviceScaleFactor: 1, mobile: false });
      return {
        ev: (fn, ...xs) => p.evaluate(fn, ...xs),
        goto: (u) => p.goto(u),
        reload: async () => {
          await p.send("Page.reload");
          await sleep(500);
        },
        shot: (path) => p.screenshot(path),
      };
    };
    return await scenario(open, url, a.out, "edge");
  } finally {
    await b.close();
  }
}

// --- Safari through safaridriver on a Mac -------------------------------------------------------------------------

function ssh(host, cmd, opts = {}) {
  return execFileSync("ssh", ["-o", "BatchMode=yes", "-o", "ConnectTimeout=15", host, cmd], { encoding: "utf8", ...opts });
}

async function wd(base, method, path, body) {
  const r = await fetch(`${base}${path}`, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const j = await r.json();
  if (!r.ok) throw new Error(`${method} ${path}: ${j.value?.message ?? r.status}`);
  return j.value;
}

async function runSafari(a, dist) {
  const host = a.safari;
  const remote = ssh(host, "mktemp -d /tmp/oe-ds-shell.XXXXXX").trim();
  const port = a.port;
  const wdPort = a.port + 1;
  const procs = [];
  try {
    execFileSync("scp", ["-q", "-o", "BatchMode=yes", "-r", ...readdirSync(dist).map((f) => join(dist, f)), `${host}:${remote}/`]);
    // The page server and safaridriver on the Mac, both on its 127.0.0.1; the tunnel brings safaridriver here.
    procs.push(spawn("ssh", ["-o", "BatchMode=yes", host, `cd ${remote} && exec python3 -m http.server ${port} --bind 127.0.0.1`], { stdio: "ignore" }));
    procs.push(spawn("ssh", ["-o", "BatchMode=yes", "-L", `${wdPort}:127.0.0.1:${wdPort}`, host, `exec safaridriver -p ${wdPort}`], { stdio: "ignore" }));
    const base = `http://127.0.0.1:${wdPort}`;
    let session = null;
    for (let i = 0; i < 60 && !session; i++) {
      await sleep(500);
      session = await wd(base, "POST", "/session", { capabilities: { alwaysMatch: { browserName: "safari" } } }).catch((e) => (i > 50 ? Promise.reject(e) : null));
    }
    const sid = session.sessionId;
    const version = session.capabilities?.browserVersion;
    console.log(`Safari ${version} on ${host}`);
    const S = (m, p, b) => wd(base, m, `/session/${sid}${p}`, b);
    let first = true;
    let current = null;
    const open = async () => {
      let handle;
      if (first) {
        first = false;
        handle = await S("GET", "/window");
      } else {
        handle = (await S("POST", "/window/new", { type: "tab" })).handle;
      }
      const use = async () => {
        if (current !== handle) {
          await S("POST", "/window", { handle });
          current = handle;
        }
      };
      return {
        ev: async (fn, ...xs) => {
          await use();
          const script = `const done = arguments[arguments.length - 1]; Promise.resolve((${fn})(...${JSON.stringify(xs)})).then((v) => done({ ok: true, v }), (e) => done({ ok: false, e: String(e && e.stack || e) }));`;
          const r = await S("POST", "/execute/async", { script, args: [] });
          if (!r?.ok) throw new Error(r?.e ?? "script failed");
          return r.v;
        },
        goto: async (u) => {
          await use();
          await S("POST", "/url", { url: u });
        },
        reload: async () => {
          await use();
          await S("POST", "/refresh", {});
          await sleep(500);
        },
        shot: async (path) => {
          await use();
          writeFileSync(path, Buffer.from(await S("GET", "/screenshot"), "base64"));
        },
      };
    };
    await S("POST", "/timeouts", { script: 60000 });
    try {
      return { version, results: await scenario(open, `http://127.0.0.1:${port}/`, a.out, "safari") };
    } finally {
      await S("DELETE", "").catch(() => undefined);
    }
  } finally {
    for (const p of procs) p.kill();
    try {
      ssh(host, `pkill -f "http.server ${port}" ; pkill -f "safaridriver -p ${wdPort}" ; rm -rf ${remote}`);
    } catch {
      /* already gone */
    }
  }
}

async function main() {
  const a = args(process.argv.slice(2));
  mkdirSync(a.out, { recursive: true });
  const dist = join(a.out, "page");
  await buildPage(dist);
  let report;
  if (a.safari) {
    report = await runSafari(a, dist);
  } else {
    const server = await serve(dist, a.port);
    try {
      report = { results: await runEdge(a, `http://127.0.0.1:${a.port}/`) };
    } finally {
      server.close();
    }
  }
  writeFileSync(join(a.out, "report.json"), JSON.stringify(report, null, 2));
  const failed = report.results.filter((r) => !r.ok);
  console.log(`${report.results.length - failed.length}/${report.results.length} passed; report and screenshot in ${a.out}`);
  process.exit(failed.length ? 1 : 0);
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
