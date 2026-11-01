// LightBurn preferences import, the browser side (UX-067): POST /api/prefs/import_lightburn maps a LightBurn .lbprefs
// backup or prefs.ini (crates/engine/src/lbprefs.rs reads only the keys OpenEtch has, typed) and applies the engine's
// part itself (autosave, the active device's Optimization defaults); its `browser` answer is applied here to this
// browser's preferences: display prefs, canvas snapping and view, import options, framing options, the system
// clipboard choice and the dialogs' starting values. Every part is re-checked before use. `planLightBurnPrefs` is pure.

import type { FrameOptions } from "../api/job.ts";
import type { DialogDefaults } from "../workspace/dialogDefaults.ts";
import type { Prefs } from "../workspace/prefs.ts";
import type { ImportPrefs } from "./fileFormats.ts";

/** What the import changes in this browser, each part only when the file had it. */
export interface LightBurnBrowserPlan {
  prefs: Partial<Prefs>;
  /** Canvas snapping and view style (App.tsx's view preferences). */
  view: { snap?: { objects?: boolean; grid?: boolean; gridMm?: number }; viewStyle?: "filled" | "wireframe" };
  importPrefs: Partial<ImportPrefs>;
  frame: Partial<FrameOptions>;
  systemClipboard: boolean | null;
  dialogs: DialogDefaults;
}

/** The engine's summary of an import (or a preview). */
export interface LightBurnPrefsResult {
  preview: boolean;
  used: string[];
  rejected: string[];
  otherKeys: number;
  autosaveMinutes: number | null;
  optimization: boolean;
  browser: unknown;
}

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const bool = (v: unknown): boolean | undefined => (typeof v === "boolean" ? v : undefined);
const num = (v: unknown, lo: number, hi: number): number | undefined => (typeof v === "number" && Number.isFinite(v) && v >= lo && v <= hi ? v : undefined);

/** Drop undefined fields. */
function defined<T extends Record<string, unknown>>(o: T): Partial<T> {
  return Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined)) as Partial<T>;
}

export function parseLightBurnPrefsResult(raw: unknown): LightBurnPrefsResult {
  const r = obj(raw);
  const strs = (v: unknown) => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
  const engine = obj(r.engine);
  return {
    preview: r.preview === true,
    used: strs(r.used),
    rejected: strs(r.rejected),
    otherKeys: typeof r.other_keys === "number" ? r.other_keys : 0,
    autosaveMinutes: typeof engine.autosave_minutes === "number" ? engine.autosave_minutes : null,
    optimization: !!engine.optimization,
    browser: r.browser,
  };
}

/** The browser-side changes of the engine's `browser` answer, checked field by field. */
export function planLightBurnPrefs(browser: unknown): LightBurnBrowserPlan {
  const b = obj(browser);
  const p = obj(b.prefs);
  const nudge = obj(p.nudge);
  const n = defined({ normal: num(nudge.normal, 0.001, 1000), shift: num(nudge.shift, 0.001, 1000), ctrl: num(nudge.ctrl, 0.001, 1000) });
  const prefs: Partial<Prefs> = defined({
    units: p.units === "mm" || p.units === "in" ? p.units : undefined,
    gridMm: num(p.gridMm, 0, 1000),
    snapTolPx: num(p.snapTolPx, 1, 40),
    invertWheel: bool(p.invertWheel),
    crosshair: bool(p.crosshair),
    language: typeof p.language === "string" && /^[a-z]{2}$/.test(p.language) ? p.language : undefined,
  }) as Partial<Prefs>;
  // Nudge steps merge into the current ones in `applyLightBurnPrefs` (Ctrl+Shift has no LightBurn key).
  if (Object.keys(n).length) (prefs as Record<string, unknown>).nudge = n;
  const v = obj(b.view);
  const s = obj(v.snap);
  const snap = defined({ objects: bool(s.objects), grid: bool(s.grid), gridMm: num(s.gridMm, 0.001, 1000) });
  const view: LightBurnBrowserPlan["view"] = defined({
    snap: Object.keys(snap).length ? snap : undefined,
    viewStyle: v.viewStyle === "filled" || v.viewStyle === "wireframe" ? v.viewStyle : undefined,
  });
  const imp = obj(obj(b.files).importPrefs);
  const importPrefs = defined({ group: bool(imp.group), select: bool(imp.select), center: bool(imp.center), hiddenLayers: bool(imp.hiddenLayers) });
  const f = obj(obj(b.laser).frame);
  const frame = defined({
    style: f.style === "bounds" || f.style === "hull" || f.style === "outline" ? f.style : undefined,
    individually: bool(f.individually),
    outsideOnly: bool(f.outsideOnly),
    toolLayersOnly: bool(f.toolLayersOnly),
  }) as Partial<FrameOptions>;
  const d = obj(b.dialogs);
  const dialogs: DialogDefaults = {};
  for (const k of ["grid_array", "circular_array", "nest", "offset"] as const) {
    const x = obj(d[k]);
    if (Object.keys(x).length) dialogs[k] = x;
  }
  return { prefs, view, importPrefs, frame, systemClipboard: bool(b.system_clipboard) ?? null, dialogs };
}

/** How many settings the plan changes in this browser. */
export function planSize(p: LightBurnBrowserPlan): number {
  const nudge = (p.prefs as { nudge?: object }).nudge;
  return (
    Object.keys(p.prefs).length -
    (nudge ? 1 : 0) +
    (nudge ? Object.keys(nudge).length : 0) +
    Object.keys(p.view.snap ?? {}).length +
    (p.view.viewStyle ? 1 : 0) +
    Object.keys(p.importPrefs).length +
    Object.keys(p.frame).length +
    (p.systemClipboard === null ? 0 : 1) +
    Object.values(p.dialogs).reduce((a, x) => a + Object.keys(x ?? {}).length, 0)
  );
}
