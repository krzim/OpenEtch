// The normal build's stand-in for designmode/backend.ts (vite.config.ts aliases it): main.tsx only installs a design
// backend in OpenEtch Design, but Rollup would still emit the backend worker and its wasm glue for the dynamic
// import in the dead branch.

export async function installDesignBackend(): Promise<void> {}
