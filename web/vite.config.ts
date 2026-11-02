// Vite config for the OpenEtch web UI. In development the engine runs separately, on 127.0.0.1:7878 unless
// OPENETCH_ENGINE names another one (OPENETCH_ENGINE=http://127.0.0.1:7981 npm run dev); /api (HTTP and the
// /api/events WebSocket) and /mcp are proxied to it. `npm run build` writes web/dist, which the engine serves.
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import pkg from "./package.json";

// `process` read through globalThis: the web tsconfig has no Node types (no @types/node dependency).
const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};
const ENGINE = env.OPENETCH_ENGINE ?? "http://127.0.0.1:7878";

export default defineConfig({
  plugins: [react()],
  // The web UI's version for Help > About (src/workspace/env.d.ts).
  define: { __OPENETCH_VERSION__: JSON.stringify(pkg.version) },
  server: {
    host: true,
    proxy: {
      "/api": { target: ENGINE, changeOrigin: true, ws: true },
      "/mcp": { target: ENGINE, changeOrigin: true, ws: true },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: true,
  },
});
