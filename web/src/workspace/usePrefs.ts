// React bindings for the preferences store: `usePrefs()` re-renders on change, `useT()` translates in the chosen
// language, `useResolvedTheme()` follows the system theme when "Match system" is picked, and `useApplyWorkspace()`
// (once, in App) puts the theme, UI scale and layout mode (layoutMode.ts) on <html>.

import { useCallback, useEffect, useMemo, useSyncExternalStore } from "react";
import { translate, translateDynamic, type MsgKey, type T, type Vars } from "./i18n.ts";
import { useLayoutMode } from "./layoutMode.ts";
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

/** Translate a key built at runtime ("shape.Circle"), with the English `fallback` when there is no such message. */
export function useTD(): (key: string, fallback: string, vars?: Vars) => string {
  const lang = usePrefs().language;
  return useCallback((key: string, fallback: string, vars?: Vars) => translateDynamic(lang, key, fallback, vars), [lang]);
}

/** The display name of a shape kind from snapping.ts `shapeKindName` ("Circle", "Path", ...), translated. */
export function shapeKindText(td: ReturnType<typeof useTD>, englishName: string): string {
  return td(`shape.${englishName}`, englishName);
}

/** A pass label from api/passes.ts ("Main", "Sub-layer 2" or the sub-layer's own name), translated. */
export function passLabelText(t: T, label: string): string {
  if (label === "Main") return t("layers.main");
  if (label === "Cleanup") return t("layers.cleanupPass");
  if (label.startsWith("Main · ")) return `${t("layers.main")} · ${label.slice(7)}`;
  const m = /^Sub-layer (\d+)$/.exec(label);
  return m ? t("layers.subN", { n: Number(m[1]) }) : label;
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

/** Theme, UI scale, language and the layout attributes on the document root; preferences changed in another tab are
 * picked up. */
export function useApplyWorkspace(): void {
  const theme = useResolvedTheme();
  // Subscribing keeps data-layout / data-pointer / data-density on <html> current.
  useLayoutMode();
  const { uiScale, iconScale, handleScale, language } = usePrefs();
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = theme;
    root.style.colorScheme = theme;
  }, [theme]);
  useEffect(() => {
    document.documentElement.style.setProperty("--ui-scale", String(uiScale));
  }, [uiScale]);
  useEffect(() => {
    document.documentElement.style.setProperty("--icon-scale", String(iconScale));
  }, [iconScale]);
  useEffect(() => {
    document.documentElement.style.setProperty("--handle-scale", String(handleScale));
  }, [handleScale]);
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
