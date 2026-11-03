// The build's mode, read once (docs/design-only/plan.md "Mode switch"). `npm run build:design` (vite build --mode
// design, or VITE_DESIGN_ONLY=1 vite build) makes OpenEtch Design: the editor alone, running in the browser with no
// engine and no laser. vite.config.ts turns the switch into the constants below (`define`), so `if (!DESIGN_ONLY)` is
// a literal test and Rollup drops the machine UI from the design bundle. Under the Node test runner nothing is
// defined: the full app.

declare const __OPENETCH_DESIGN_ONLY__: boolean | undefined;
declare const __OPENETCH_DESIGN_BACKEND__: string | undefined;
declare const __OPENETCH_REPO_URL__: string | undefined;
declare const __OPENETCH_FULL_APP_URL__: string | undefined;

/** OpenEtch Design: the browser-only build. */
export const DESIGN_ONLY: boolean = typeof __OPENETCH_DESIGN_ONLY__ !== "undefined" && __OPENETCH_DESIGN_ONLY__ === true;

/** The project's source and releases (VITE_REPO_URL). */
export const REPO_URL: string = (typeof __OPENETCH_REPO_URL__ !== "undefined" && __OPENETCH_REPO_URL__) || "https://github.com/krzim/OpenEtch";

/** "Get the full app": where the full app (engine and laser control) is described (VITE_FULL_APP_URL). */
export const FULL_APP_URL: string = (typeof __OPENETCH_FULL_APP_URL__ !== "undefined" && __OPENETCH_FULL_APP_URL__) || `${REPO_URL}#readme`;

/**
 * The design build's backend (designmode/backend.ts): "" the in-browser one; "engine" a simulator engine serving
 * the page (`vite build --mode design-engine`, for the device harness only; never published).
 */
export const DESIGN_BACKEND: string = (typeof __OPENETCH_DESIGN_BACKEND__ !== "undefined" && __OPENETCH_DESIGN_BACKEND__) || "";
