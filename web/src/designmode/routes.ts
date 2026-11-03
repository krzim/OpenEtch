// What OpenEtch Design's backend answers for the machine side of the API (docs/design-only/plan.md 1.2 "stubs"): GET
// /api/machine says there is no machine, GET /api/job is idle, every other machine, run, camera, production and
// diagnostics route is 404 `{error, code: "design_only"}`. The design UI hides what would call them; the stubs are a
// safety net. One list for every design backend (the in-browser core, the stand-ins in this folder), so they agree.
// Pure: `npm test` covers it.

/** GET /api/machine in OpenEtch Design. */
export const NO_MACHINE = { connected: false, design_only: true, sim: false, busy: false, runs: [], pending_runs: [] } as const;

/** GET /api/job in OpenEtch Design: idle, nothing ever runs. */
export const IDLE_JOB = {
  active: false,
  state: "idle",
  completed: 0,
  count: 0,
  elapsed_s: 0,
  estimated_time_s: 0,
  progress: 0,
  remaining_s: 0,
  run_id: null,
  paused: false,
} as const;

/** The 404 body of a machine route. */
export const DESIGN_ONLY_ERROR = { error: "not available in the design-only build", code: "design_only" } as const;

/** Route prefixes (after /api) that need a machine, an engine computer or the engine's own files. */
const MACHINE_PREFIXES = [
  "/machine",
  "/runs",
  "/job",
  "/camera",
  "/calibration",
  "/production",
  "/notifications",
  "/diagnostics",
  "/updates",
  "/sim",
  "/devices/scan",
  "/devices/wizard",
  "/devices/grbl",
  "/devices/ruida",
  "/devices/import/markcfg",
  "/export/rd",
  "/job-log",
  "/job-ledger",
];

/** Whether `path` (no query) is a machine route in OpenEtch Design. (MCP is absent altogether: nothing calls it.) */
export function isMachineRoute(path: string): boolean {
  if (!path.startsWith("/api/")) return false;
  const rest = path.slice(4);
  return MACHINE_PREFIXES.some((p) => rest === p || rest.startsWith(`${p}/`) || rest.startsWith(`${p}?`));
}

/** The design build's answer to a machine route, or null for every other route (the document backend's). */
export function machineStub(method: string, path: string): { status: number; body: unknown } | null {
  const m = method.toUpperCase();
  if (m === "GET" && path === "/api/machine") return { status: 200, body: NO_MACHINE };
  if (m === "GET" && path === "/api/job") return { status: 200, body: IDLE_JOB };
  if (isMachineRoute(path)) return { status: 404, body: DESIGN_ONLY_ERROR };
  return null;
}

/** A JSON Response. */
export function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}
