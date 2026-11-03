// Shared state of the MOPA materials UI: which of its windows is open (from the Library dock, menus, command search,
// the canvas HUD), the Result colours view toggle (per browser), the active device's hue step model, and the result
// photos as object URLs (fetched with the Authorization header, cached by entry and photo time).

import { useEffect, useSyncExternalStore } from "react";
import { onEngineEvent } from "../api/events.ts";
import { refreshLibrary } from "../libs/store.ts";
import { materialsApi } from "./api.ts";
import { parseHueModel, XTOOL_MODEL, type HueModel } from "./model.ts";

function observable<T>(initial: T) {
  let v = initial;
  const subs = new Set<() => void>();
  return {
    get: () => v,
    set: (x: T) => {
      v = x;
      for (const f of [...subs]) f();
    },
    use: (): T =>
      useSyncExternalStore(
        (f) => {
          subs.add(f);
          return () => subs.delete(f);
        },
        () => v,
      ),
  };
}

export type MaterialsWindow =
  | { kind: "finish"; library: string; id: string; photo?: boolean }
  | { kind: "bracket"; library: string; id: string }
  | { kind: "separate"; image: string }
  | { kind: "hueModel" };

export const materialsWindow = observable<MaterialsWindow | null>(null);

export function openMaterials(w: MaterialsWindow): void {
  materialsWindow.set(w);
}

export function closeMaterials(): void {
  materialsWindow.set(null);
}

/** The preset picked in the Material Library dock (the menus' Bracket test uses it). */
export const pickedEntry = observable<{ library: string; id: string } | null>(null);

// ---- Result colours view ----------------------------------------------------------------------------------------

const RESULT_KEY = "openetch.resultColors";

function readResult(): boolean {
  try {
    return localStorage.getItem(RESULT_KEY) === "1";
  } catch {
    return false;
  }
}

export const resultColors = observable<boolean>(readResult());

export function setResultColors(on: boolean): void {
  resultColors.set(on);
  try {
    localStorage.setItem(RESULT_KEY, on ? "1" : "0");
  } catch {
    // Storage unavailable: the view lasts for this visit.
  }
}

// ---- hue step model of the active device --------------------------------------------------------------------------

export const hueModel = observable<{ model: HueModel; calibrated: boolean; loaded: boolean }>({ model: XTOOL_MODEL, calibrated: false, loaded: false });

export function refreshHueModel(): Promise<void> {
  return materialsApi
    .hueModel()
    .then((r) => {
      const o = (r ?? {}) as { model?: unknown; calibrated?: unknown };
      hueModel.set({ model: parseHueModel(o.model), calibrated: o.calibrated === true, loaded: true });
    })
    .catch(() => hueModel.set({ ...hueModel.get(), loaded: true }));
}

export function useHueModel() {
  const v = hueModel.use();
  useEffect(() => {
    if (!v.loaded) void refreshHueModel();
  }, [v.loaded]);
  return v;
}

// ---- result photos ------------------------------------------------------------------------------------------------

const photos = new Map<string, Promise<string>>();

/** An object URL of an entry's photo (`taken` keys the cache, so a new photo is fetched again). */
export function photoUrl(library: string, id: string, taken: number): Promise<string> {
  const key = `${library}\u0000${id}\u0000${taken}`;
  let p = photos.get(key);
  if (!p) {
    p = materialsApi.photo(library, id).then((d) => URL.createObjectURL(d.blob));
    p.catch(() => photos.delete(key));
    photos.set(key, p);
  }
  return p;
}

// ---- engine events ------------------------------------------------------------------------------------------------

onEngineEvent((ev) => {
  // A library changed elsewhere (another browser, an agent): refetch it.
  if (ev.type === "machine" && (ev as { event?: unknown }).event === "library") void refreshLibrary();
  if (ev.type === "device_changed" || ev.type === "devices_changed") void refreshHueModel();
});
