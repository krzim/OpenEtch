// Vite config for the galvo-studio web UI. In development the engine runs separately on 127.0.0.1:7878; /api (HTTP and
// the /api/events WebSocket) and /mcp are proxied to it. `npm run build` writes web/dist, which the engine serves.
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const ENGINE = "http://127.0.0.1:7878";

export default defineConfig({
  plugins: [react()],
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
