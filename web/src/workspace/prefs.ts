// User preferences of the web UI (LightBurn's File > Settings, display side only): theme, display units, grid and
// snapping, nudge steps, mouse wheel, UI scale, crosshair cursor, language and hotkey overrides. They live in this
// browser's localStorage (every access in try/catch: private mode or blocked storage just means defaults for the
// visit). The engine never sees them and always works in millimetres.
//
// A tiny external store so non-React code (the keyboard handler, the canvas wheel listener) reads the current values
// with `getPrefs()` and components re-render through `usePrefs()` (usePrefs.ts). No React import here, so
// `npm test` covers it.

export type ThemePref = "light" | "dark" | "system";
export type LengthUnit = "mm" | "in";
export type SpeedUnit = "mm/s" | "mm/min" | "in/s";
/** Snap kinds the user can switch off one by one (snapping.ts `SnapKind`, without the grid, which has its own toggle). */
export type SnapToggle = "endpoint" | "intersection" | "center" | "quadrant" | "midpoint" | "nearest" | "guide";
export const SNAP_TOGGLES: SnapToggle[] = ["endpoint", "intersection", "center", "quadrant", "midpoint", "nearest", "guide"];

/** Arrow-key nudge distances in mm (LightBurn: Shape move distance; Shift large, Ctrl small). */
export interface NudgeSteps {
  normal: number;
  shift: number;
  ctrl: number;
  ctrlShift: number;
}

export interface Prefs {
  theme: ThemePref;
  /** Display and entry unit for lengths; the engine stays in mm. */
  units: LengthUnit;
  speedUnit: SpeedUnit;
  /** Draw the canvas grid. */
  gridVisible: boolean;
  /** Visual grid spacing in mm; 0 picks one from the zoom (the snap grid is separate, in the toolbar). */
  gridMm: number;
  /** Object snap distance in screen pixels. */
  snapTolPx: number;
  snapKinds: Record<SnapToggle, boolean>;
  nudge: NudgeSteps;
  invertWheel: boolean;
  /** Font scale of the whole UI, 0.8 to 1.5. */
  uiScale: number;
  /** Full-window crosshair lines at the pointer over the canvas. */
  crosshair: boolean;
  /** UI language code (i18n.ts). */
  language: string;
  /** Hotkey overrides: action id -> chords (hotkeys.ts); an action not listed keeps its defaults. */
  hotkeys: Record<string, string[]>;
  /** Cuts/Layers and Laser docks collapsed. */
  docks: Record<string, boolean>;
}

export const DEFAULT_NUDGE: NudgeSteps = { normal: 5, shift: 20, ctrl: 1, ctrlShift: 0.1 };

export const DEFAULT_PREFS: Prefs = {
  theme: "system",
  units: "mm",
  speedUnit: "mm/s",
  gridVisible: true,
  gridMm: 0,
  snapTolPx: 8,
  snapKinds: { endpoint: true, intersection: true, center: true, quadrant: true, midpoint: true, nearest: true, guide: true },
  nudge: DEFAULT_NUDGE,
  invertWheel: false,
  uiScale: 1,
  crosshair: false,
  language: "en",
  hotkeys: {},
  docks: {},
};

export const PREFS_KEY = "openetch.prefs";

const num = (v: unknown, lo: number, hi: number, dflt: number): number =>
  typeof v === "number" && Number.isFinite(v) && v >= lo && v <= hi ? v : dflt;
const bool = (v: unknown, dflt: boolean): boolean => (typeof v === "boolean" ? v : dflt);
const oneOf = <T extends string>(v: unknown, all: readonly T[], dflt: T): T => (all.includes(v as T) ? (v as T) : dflt);

/** Valid preferences from anything (stored JSON from an older or newer build, or garbage): unknown keys dropped. */
export function sanitizePrefs(raw: unknown): Prefs {
  const r = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const d = DEFAULT_PREFS;
  const sk = r.snapKinds && typeof r.snapKinds === "object" ? (r.snapKinds as Record<string, unknown>) : {};
  const nu = r.nudge && typeof r.nudge === "object" ? (r.nudge as Record<string, unknown>) : {};
  const hk: Record<string, string[]> = {};
  if (r.hotkeys && typeof r.hotkeys === "object") {
    for (const [k, v] of Object.entries(r.hotkeys as Record<string, unknown>)) {
      if (Array.isArray(v) && v.every((c) => typeof c === "string") && v.length <= 4) hk[k] = v as string[];
    }
  }
  const docks: Record<string, boolean> = {};
  if (r.docks && typeof r.docks === "object") {
    for (const [k, v] of Object.entries(r.docks as Record<string, unknown>)) if (typeof v === "boolean") docks[k] = v;
  }
  return {
    theme: oneOf(r.theme, ["light", "dark", "system"] as const, d.theme),
    units: oneOf(r.units, ["mm", "in"] as const, d.units),
    speedUnit: oneOf(r.speedUnit, ["mm/s", "mm/min", "in/s"] as const, d.speedUnit),
    gridVisible: bool(r.gridVisible, d.gridVisible),
    gridMm: num(r.gridMm, 0, 1000, d.gridMm),
    snapTolPx: num(r.snapTolPx, 1, 40, d.snapTolPx),
    snapKinds: Object.fromEntries(SNAP_TOGGLES.map((k) => [k, bool(sk[k], true)])) as Record<SnapToggle, boolean>,
    nudge: {
      normal: num(nu.normal, 0.001, 1000, d.nudge.normal),
      shift: num(nu.shift, 0.001, 1000, d.nudge.shift),
      ctrl: num(nu.ctrl, 0.001, 1000, d.nudge.ctrl),
      ctrlShift: num(nu.ctrlShift, 0.001, 1000, d.nudge.ctrlShift),
    },
    invertWheel: bool(r.invertWheel, d.invertWheel),
    uiScale: num(r.uiScale, 0.8, 1.5, d.uiScale),
    crosshair: bool(r.crosshair, d.crosshair),
    language: typeof r.language === "string" && /^[a-z]{2}(-[A-Z]{2})?$/.test(r.language) ? r.language : d.language,
    hotkeys: hk,
    docks,
  };
}

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadPrefs(): Prefs {
  try {
    const raw = storage()?.getItem(PREFS_KEY);
    return sanitizePrefs(raw ? JSON.parse(raw) : null);
  } catch {
    return DEFAULT_PREFS;
  }
}

function savePrefs(p: Prefs): void {
  try {
    storage()?.setItem(PREFS_KEY, JSON.stringify(p));
  } catch {
    // Storage unavailable or full: the change lasts for this visit.
  }
}

let current: Prefs = loadPrefs();
const listeners = new Set<() => void>();

export function getPrefs(): Prefs {
  return current;
}

/** Change some preferences (validated), persist and notify subscribers. */
export function updatePrefs(patch: Partial<Prefs> | ((p: Prefs) => Partial<Prefs>)): void {
  const delta = typeof patch === "function" ? patch(current) : patch;
  current = sanitizePrefs({ ...current, ...delta });
  savePrefs(current);
  for (const l of listeners) l();
}

/** Back to the defaults (hotkeys too). */
export function resetPrefs(keep?: (keyof Prefs)[]): void {
  const kept = Object.fromEntries((keep ?? []).map((k) => [k, current[k]]));
  updatePrefs({ ...DEFAULT_PREFS, ...kept });
}

export function subscribePrefs(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Another tab changed the stored preferences: adopt them. */
export function reloadPrefs(): void {
  current = loadPrefs();
  for (const l of listeners) l();
}
