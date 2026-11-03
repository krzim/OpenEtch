// Vite config for the OpenEtch web UI. In development the engine runs separately, on 127.0.0.1:7878 unless
// OPENETCH_ENGINE names another one (OPENETCH_ENGINE=http://127.0.0.1:7981 npm run dev); /api (HTTP and the
// /api/events WebSocket) and /mcp are proxied to it. `npm run build` writes web/dist, which the engine serves, with a
// .br and a .gz copy next to every text file worth compressing (the engine sends them to browsers that accept them,
// crates/engine/src/api.rs `serve_web`). public/ holds the manifest and the icons (copied as they are).
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import pkg from "./package.json";

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

export default defineConfig({
  plugins: [react(), precompress()],
  // The browser's WebAssembly core (crates/webcore), built into src/wasm/pkg by scripts/wasm.mjs (`npm run wasm`),
  // which `npm run build` and `npm run dev` run first. Its wasm file is an asset with a content hash.
  resolve: { alias: { "webcore-pkg": "/src/wasm/pkg/webcore.js" } },
  // The preview worker (src/wasm/previewWorker.ts) is an ES module: it imports the wasm-bindgen glue.
  worker: { format: "es" },
  // The web UI's version for Help > About (src/workspace/env.d.ts).
  define: { __OPENETCH_VERSION__: JSON.stringify(pkg.version) },
  server: {
    host: true,
    // The user guide's pages and pictures live in ../docs/guide (src/guide/content.ts imports them): the dev server
    // may read that folder as well as the web root.
    fs: { allow: [".", "../docs/guide"] },
    proxy: {
      "/api": { target: ENGINE, changeOrigin: true, ws: true },
      "/mcp": { target: ENGINE, changeOrigin: true, ws: true },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // Source maps stay (bug reports), but are neither precompressed nor cached for ever by the engine.
    sourcemap: true,
  },
});
