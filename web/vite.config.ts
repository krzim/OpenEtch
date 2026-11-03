// Vite config for the OpenEtch web UI. In development the engine runs separately, on 127.0.0.1:7878 unless
// OPENETCH_ENGINE names another one (OPENETCH_ENGINE=http://127.0.0.1:7981 npm run dev); /api (HTTP and the
// /api/events WebSocket) and /mcp are proxied to it. `npm run build` writes web/dist, which the engine serves, with a
// .br and a .gz copy next to every text file worth compressing (the engine sends them to browsers that accept them,
// crates/engine/src/api.rs `serve_web`). public/ holds the manifest and the icons (copied as they are).
//
// OpenEtch Design (docs/design-only/plan.md): `npm run build:design` (`vite build --mode design`, or VITE_DESIGN_ONLY=1)
// writes web/dist-design, the browser-only editor for GitHub Pages: src/mode.ts gets DESIGN_ONLY true (so the machine
// UI drops out of the bundle), the base is relative (VITE_BASE overrides it, e.g. /OpenEtch/), there are no source
// maps (they would publish the source) and no .br/.gz copies (Pages compresses by itself). `--mode design-engine`
// (dist-design-engine, base /) is the same UI talking to a simulator engine that serves it, for the device harness.
import { defineConfig, type Plugin, type UserConfig } from "vite";
import react from "@vitejs/plugin-react";
import pkg from "./package.json";
import { designGuideSource, designHiddenPages } from "./src/guide/designFilter.ts";

// `process` read through globalThis: the web tsconfig has no Node types (no @types/node dependency).
const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};
const ENGINE = env.OPENETCH_ENGINE ?? "http://127.0.0.1:7878";

/** The parts of Node's zlib the precompress step uses (typed here: no @types/node). */
interface Zlib {
  gzipSync(data: Uint8Array, opts?: { level?: number }): Uint8Array;
  brotliCompressSync(data: Uint8Array, opts?: { params?: Record<number, number> }): Uint8Array;
  constants: { BROTLI_PARAM_QUALITY: number; BROTLI_PARAM_SIZE_HINT: number; BROTLI_MAX_QUALITY: number };
}

/** Files worth a compressed copy: text the browser loads (never source maps, images or the copies themselves). */
const COMPRESSIBLE = /\.(js|mjs|css|html|svg|json|webmanifest|txt|wasm)$/;
/** Below this, compression saves less than a packet. */
const MIN_BYTES = 1024;

/**
 * Write `<file>.br` and `<file>.gz` next to every compressible output file at build time, with Node's own zlib (no
 * dependency), so the engine on the Pi serves compressed bundles without spending CPU on it. A copy is kept only when
 * it is smaller than the file.
 */
function precompress(): Plugin {
  return {
    name: "openetch-precompress",
    apply: "build",
    enforce: "post",
    // `order: "post"`: after Vite's own generateBundle hooks, which still rewrite the chunks (the dynamic-import
    // preload helper's `__VITE_PRELOAD__` marker): a copy made earlier would hold the unfinished code.
    generateBundle: {
      order: "post",
      async handler(_options, bundle) {
      // A computed specifier keeps TypeScript from looking for Node's types; Vite runs this config in Node.
      const zlib = (await import(/* @vite-ignore */ ["node", "zlib"].join(":"))) as Zlib;
      for (const out of Object.values(bundle)) {
        if (!COMPRESSIBLE.test(out.fileName)) continue;
        const raw = out.type === "chunk" ? new TextEncoder().encode(out.code) : typeof out.source === "string" ? new TextEncoder().encode(out.source) : out.source;
        if (raw.byteLength < MIN_BYTES) continue;
        const { BROTLI_PARAM_QUALITY, BROTLI_PARAM_SIZE_HINT, BROTLI_MAX_QUALITY } = zlib.constants;
        const br = zlib.brotliCompressSync(raw, { params: { [BROTLI_PARAM_QUALITY]: BROTLI_MAX_QUALITY, [BROTLI_PARAM_SIZE_HINT]: raw.byteLength } });
        const gz = zlib.gzipSync(raw, { level: 9 });
        if (br.byteLength < raw.byteLength) this.emitFile({ type: "asset", fileName: `${out.fileName}.br`, source: br });
        if (gz.byteLength < raw.byteLength) this.emitFile({ type: "asset", fileName: `${out.fileName}.gz`, source: gz });
      }
      },
    },
  };
}

/** The parts of Node's fs the design steps use. */
interface Fs {
  readFileSync(path: string | URL, enc: "utf8"): string;
  writeFileSync(path: string, data: string): void;
  readdirSync(path: URL): string[];
}

/** OpenEtch Design's hidden guide pages (src/guide/flags.ts), read from docs/guide's front matter. */
async function hiddenGuidePages(): Promise<string[]> {
  const fs = (await import(/* @vite-ignore */ ["node", "fs"].join(":"))) as Fs;
  const dir = new URL("../docs/guide/", import.meta.url);
  const pages = fs
    .readdirSync(dir)
    .filter((f) => f.endsWith(".md") && f !== "README.md")
    .map((f) => [f.slice(0, -3), fs.readFileSync(new URL(f, dir), "utf8")] as [string, string]);
  return designHiddenPages(pages);
}

/**
 * OpenEtch Design's web app manifest: the site lives under a path (github.io/<repo>/), so every URL is relative, and
 * it has no production cockpit shortcut. Written over public/manifest.webmanifest's copy after the build.
 */
function designManifest(outDir: string): Plugin {
  return {
    name: "openetch-design-manifest",
    apply: "build",
    async writeBundle(options) {
      const fs = (await import(/* @vite-ignore */ ["node", "fs"].join(":"))) as Fs;
      const dir = options.dir ?? outDir;
      const m = JSON.parse(fs.readFileSync(`${dir}/manifest.webmanifest`, "utf8")) as Record<string, unknown> & { icons?: { src: string }[] };
      const design = {
        ...m,
        id: "./",
        name: "OpenEtch Design",
        short_name: "OpenEtch",
        description: "Laser design in your browser: nothing is uploaded.",
        start_url: "./",
        scope: "./",
        icons: (m.icons ?? []).map((i) => ({ ...i, src: i.src.replace(/^\//, "") })),
      };
      delete (design as { shortcuts?: unknown }).shortcuts;
      fs.writeFileSync(`${dir}/manifest.webmanifest`, `${JSON.stringify(design, null, 2)}
`);
    },
  };
}

/**
 * OpenEtch Design's guide (src/guide/designFilter.ts): the bundled pages leave out the machine pages' text and the
 * lines about what the design build does not have, so none of it ships in the design bundle.
 */
function designGuide(): Plugin {
  const PAGE = /[\\/]docs[\\/]guide[\\/]([^\\/]+)\.md\?raw$/;
  return {
    name: "openetch-design-guide",
    enforce: "pre",
    async load(id) {
      const m = PAGE.exec(id);
      if (!m) return null;
      const fs = (await import(/* @vite-ignore */ ["node", "fs"].join(":"))) as Fs;
      const src = fs.readFileSync(id.slice(0, id.indexOf("?")), "utf8");
      return `export default ${JSON.stringify(designGuideSource(m[1], src))};`;
    },
  };
}

/** The build's mode, from `--mode` or VITE_DESIGN_ONLY / VITE_DESIGN_BACKEND (src/mode.ts reads the result). */
function designMode(mode: string): { design: boolean; backend: string; outDir: string } {
  const design = mode === "design" || mode === "design-engine" || env.VITE_DESIGN_ONLY === "1";
  const backend = mode === "design-engine" ? "engine" : (env.VITE_DESIGN_BACKEND ?? "");
  return { design, backend, outDir: !design ? "dist" : backend === "engine" ? "dist-design-engine" : "dist-design" };
}

export default defineConfig(async ({ mode }): Promise<UserConfig> => {
  const dm = designMode(mode);
  const hiddenGuide = dm.design ? await hiddenGuidePages() : [];
  return {
  plugins: dm.design ? [react(), designGuide(), designManifest(dm.outDir)] : [react(), precompress()],
  base: env.VITE_BASE ?? (dm.design && dm.backend !== "engine" ? "./" : "/"),
  // The browser's WebAssembly core (crates/webcore), built into src/wasm/pkg by scripts/wasm.mjs (`npm run wasm`),
  // which `npm run build` and `npm run dev` run first. Its wasm file is an asset with a content hash.
  resolve: { alias: { "webcore-pkg": "/src/wasm/pkg/webcore.js" } },
  // The preview worker (src/wasm/previewWorker.ts) is an ES module: it imports the wasm-bindgen glue.
  worker: { format: "es" },
  // The web UI's version for Help > About (src/workspace/env.d.ts).
  define: {
    __OPENETCH_VERSION__: JSON.stringify(pkg.version),
    __OPENETCH_DESIGN_ONLY__: JSON.stringify(dm.design),
    __OPENETCH_DESIGN_BACKEND__: JSON.stringify(dm.backend),
    __OPENETCH_REPO_URL__: JSON.stringify(env.VITE_REPO_URL ?? ""),
    __OPENETCH_FULL_APP_URL__: JSON.stringify(env.VITE_FULL_APP_URL ?? ""),
    __OPENETCH_GUIDE_HIDDEN__: JSON.stringify(hiddenGuide),
  },
  server: {
    host: true,
    // The user guide's pages and pictures live in ../docs/guide (src/guide/content.ts imports them): the dev server
    // may read that folder as well as the web root.
    fs: { allow: [".", "../docs/guide"] },
    // OpenEtch Design talks to no engine (except the harness's design-engine mode).
    proxy:
      dm.design && dm.backend !== "engine"
        ? undefined
        : {
            "/api": { target: ENGINE, changeOrigin: true, ws: true },
            "/mcp": { target: ENGINE, changeOrigin: true, ws: true },
          },
  },
  build: {
    outDir: dm.outDir,
    emptyOutDir: true,
    // Source maps stay (bug reports), but are neither precompressed nor cached for ever by the engine. Never in
    // OpenEtch Design: a public site would publish the source with them.
    sourcemap: !dm.design,
  },
  };
});
