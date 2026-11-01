// Per-browser settings of the files UI: LightBurn's import options (group, select, centre, hidden AI layers) and the
// Recent Templates list. localStorage, every access in try/catch (private mode or blocked storage: defaults for the
// visit). A tiny external store so components re-render through `useFilesStore`.

import { useSyncExternalStore } from "react";
import { DEFAULT_IMPORT_PREFS, importPrefsFrom, pushRecent, type ImportPrefs } from "./fileFormats.ts";

const KEY = "openetch.files";

interface State {
  importPrefs: ImportPrefs;
  recentTemplates: string[];
}

function load(): State {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<State> | null;
    return {
      importPrefs: importPrefsFrom(raw?.importPrefs),
      recentTemplates: Array.isArray(raw?.recentTemplates) ? raw.recentTemplates.filter((x): x is string => typeof x === "string").slice(0, 8) : [],
    };
  } catch {
    return { importPrefs: DEFAULT_IMPORT_PREFS, recentTemplates: [] };
  }
}

let state: State = load();
const listeners = new Set<() => void>();

function set(next: State): void {
  state = next;
  try {
    localStorage.setItem(KEY, JSON.stringify(state));
  } catch {
    // Storage unavailable: kept for this visit.
  }
  for (const l of listeners) l();
}

export function filesState(): State {
  return state;
}

export function setImportPrefs(p: Partial<ImportPrefs>): void {
  set({ ...state, importPrefs: { ...state.importPrefs, ...p } });
}

export function rememberTemplate(name: string): void {
  set({ ...state, recentTemplates: pushRecent(state.recentTemplates, name) });
}

export function useFilesStore(): State {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => state,
  );
}
