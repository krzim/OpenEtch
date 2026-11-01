// React bindings for the preferences store: `usePrefs()` re-renders on change, `useT()` translates in the chosen
// language, `useResolvedTheme()` follows the system theme when "Match system" is picked, and `useApplyWorkspace()`
// (once, in App) puts the theme and UI scale on <html>.

import { useCallback, useEffect, useMemo, useSyncExternalStore } from "react";
import { translate, type MsgKey, type T, type Vars } from "./i18n.ts";
import { getPrefs, reloadPrefs, subscribePrefs, PREFS_KEY, type Prefs } from "./prefs.ts";
import { resolveTheme, type Theme } from "./theme.ts";
import { unitConversion, type UnitConversion } from "./units.ts";

export function usePrefs(): Prefs {
  return useSyncExternalStore(subscribePrefs, getPrefs, getPrefs);
}

export function useT(): T {
  const lang = usePrefs().language;
  return useCallback((key: MsgKey, vars?: Vars) => translate(lang, key, vars), [lang]);
}

const DARK_QUERY = "(prefers-color-scheme: dark)";

function systemDark(): boolean {
  try {
    return window.matchMedia(DARK_QUERY).matches;
  } catch {
    return false;
  }
}

function subscribeSystem(fn: () => void): () => void {
  try {
    const mq = window.matchMedia(DARK_QUERY);
    mq.addEventListener("change", fn);
    return () => mq.removeEventListener("change", fn);
  } catch {
    return () => {};
  }
}

export function useResolvedTheme(): Theme {
  const pref = usePrefs().theme;
  const dark = useSyncExternalStore(subscribeSystem, systemDark, () => false);
  return resolveTheme(pref, dark);
}

/** Theme, UI scale and language on the document root; preferences changed in another tab are picked up. */
export function useApplyWorkspace(): void {
  const theme = useResolvedTheme();
  const { uiScale, language } = usePrefs();
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = theme;
    root.style.colorScheme = theme;
  }, [theme]);
  useEffect(() => {
    document.documentElement.style.setProperty("--ui-scale", String(uiScale));
  }, [uiScale]);
  useEffect(() => {
    document.documentElement.lang = language;
  }, [language]);
  useEffect(() => {
    const onStorage = (e: StorageEvent) => e.key === PREFS_KEY && reloadPrefs();
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
}

/** Display conversion for a field labelled with engine unit `unit` ("mm", "mm/s"; others unchanged). */
export function useUnitConversion(unit: string | undefined): UnitConversion {
  const { units, speedUnit } = usePrefs();
  return useMemo(() => unitConversion(unit, { units, speedUnit }), [unit, units, speedUnit]);
}
