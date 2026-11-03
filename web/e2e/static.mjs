// A static file server for a built site under a base path, as GitHub Pages serves it (`/OpenEtch/` for a project
// site): index.html for the base itself, 404 for anything outside it, `application/wasm` for the core. 127.0.0.1
// only. Every request is logged, so a harness can check that the site loads nothing but its own files.
//
//   import { serveSite } from "./static.mjs";
//   const site = await serveSite({ dir: "web/dist-design", port: 7976, base: "/OpenEtch/" });
//   ... site.log (["GET /OpenEtch/assets/...", ...]) ... await site.close();
//
//   node e2e/static.mjs --dir dist-design --port 7976 --base /OpenEtch/     (serve until Ctrl+C)

import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".css": "text/css",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".webmanifest": "application/manifest+json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
  ".txt": "text/plain; charset=utf-8",
};

export function serveSite({ dir, port, base = "/", host = "127.0.0.1" }) {
  const root = resolve(dir);
  const prefix = base.endsWith("/") ? base : `${base}/`;
  const log = [];
  const server = createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://x");
    log.push(`${req.method} ${url.pathname}`);
    if (!url.pathname.startsWith(prefix) && url.pathname !== prefix.slice(0, -1)) {
      res.writeHead(404).end();
      return;
    }
    let rel = decodeURIComponent(url.pathname.slice(prefix.length));
    if (rel === "" || rel.endsWith("/")) rel += "index.html";
    const file = resolve(join(root, rel));
    if (!(file === root || file.startsWith(root + sep)) || !existsSync(file) || !statSync(file).isFile()) {
      res.writeHead(404).end();
      return;
    }
    // Pages sends max-age=600; no-store here so a rebuilt site is never served stale to a harness run.
    res.writeHead(200, { "content-type": MIME[extname(file)] ?? "application/octet-stream", "cache-control": "no-store" });
    createReadStream(file).pipe(res);
  });
  return new Promise((ok, fail) => {
    server.once("error", fail);
    server.listen(port, host, () =>
      ok({
        url: `http://${host}:${port}${prefix}`,
        log,
        close: () => new Promise((r) => server.close(() => r())),
      }),
    );
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const a = process.argv.slice(2);
  const get = (k, d) => {
    const i = a.indexOf(k);
    return i >= 0 ? a[i + 1] : d;
  };
  const site = await serveSite({ dir: get("--dir", "dist-design"), port: Number(get("--port", "7976")), base: get("--base", "/") });
  console.log(`serving ${get("--dir", "dist-design")} at ${site.url}`);
}
