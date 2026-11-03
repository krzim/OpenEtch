// Starts one preview worker (previewWorker.ts, crates/webcore's small preview wasm) for previewClient.ts. OpenEtch
// Design swaps this module for standalone/previewPort.ts at build time (vite.config.ts), whose "worker" is a port
// onto the backend worker that already holds the document and the design wasm: one wasm download, one copy of the
// document.

export function spawnPreviewWorker(name: string): Worker {
  return new Worker(new URL("./previewWorker.ts", import.meta.url), { type: "module", name: `openetch-${name}` });
}
