// Starting values of the Grid Array, Circular Array, Nest and Offset dialogs, as LightBurn keeps its dialogs' last
// settings in its preferences (GridArray_*, RadialArray_*, NestMultisheet, OffsetDistance, ...). Filled by the LightBurn
// preferences import (files/LightBurnPrefsImport.tsx); kept in this browser (every access in try/catch). The values
// are the engine commands' parameter names (`art.array`, `art.circular_array`, `art.nest`, `path.offset`), read with
// the typed getters below so a stored value of the wrong type is ignored. Pure apart from localStorage; tested.

export type DialogName = "grid_array" | "circular_array" | "nest" | "offset";
export type DialogDefaults = Partial<Record<DialogName, Record<string, unknown>>>;

export const DIALOG_DEFAULTS_KEY = "openetch.dialogDefaults";
const NAMES: DialogName[] = ["grid_array", "circular_array", "nest", "offset"];

let memory: DialogDefaults = {};

/** Only the known dialogs, each an object. */
export function sanitizeDialogDefaults(raw: unknown): DialogDefaults {
  const out: DialogDefaults = {};
  if (!raw || typeof raw !== "object") return out;
  for (const n of NAMES) {
    const v = (raw as Record<string, unknown>)[n];
    if (v && typeof v === "object" && !Array.isArray(v)) out[n] = { ...(v as Record<string, unknown>) };
  }
  return out;
}

export function getDialogDefaults(): DialogDefaults {
  try {
    const raw = typeof localStorage === "undefined" ? null : localStorage.getItem(DIALOG_DEFAULTS_KEY);
    if (raw) return sanitizeDialogDefaults(JSON.parse(raw));
  } catch {
    // Storage unavailable: what this visit set.
  }
  return memory;
}

/** Merge `d` into the stored defaults (per dialog, key by key). */
export function setDialogDefaults(d: DialogDefaults): void {
  const cur = getDialogDefaults();
  const next: DialogDefaults = { ...cur };
  for (const [n, v] of Object.entries(sanitizeDialogDefaults(d)) as [DialogName, Record<string, unknown>][]) next[n] = { ...(cur[n] ?? {}), ...v };
  memory = next;
  try {
    if (typeof localStorage !== "undefined") localStorage.setItem(DIALOG_DEFAULTS_KEY, JSON.stringify(next));
  } catch {
    // Storage unavailable: kept for this visit.
  }
}

/** A stored number of dialog `n`, else `d`. */
export function dNum(n: DialogName, key: string, d: number): number {
  const v = getDialogDefaults()[n]?.[key];
  return typeof v === "number" && Number.isFinite(v) ? v : d;
}

/** A stored boolean of dialog `n`, else `d`. */
export function dBool(n: DialogName, key: string, d: boolean): boolean {
  const v = getDialogDefaults()[n]?.[key];
  return typeof v === "boolean" ? v : d;
}

/** A stored string of dialog `n`, else `d`. */
export function dStr(n: DialogName, key: string, d: string): string {
  const v = getDialogDefaults()[n]?.[key];
  return typeof v === "string" ? v : d;
}
