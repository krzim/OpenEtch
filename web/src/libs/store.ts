// Shared state of the libs UI: a tiny bus that opens its windows from anywhere (menus, the layer menu, the Properties
// panel), the active device's preferences (GET /api/prefs, refetched on the `prefs` and device events), the active
// device's profile (IO ports, axis), the Library window's view, the camera overlay settings and a per-browser mute.

import { useCallback, useEffect, useSyncExternalStore } from "react";
import { onEngineEvent } from "../api/events.ts";
import { libsApi } from "./api.ts";
import { NO_PREFS, parseLibraryView, parsePrefs, type DevicePrefs, type LibraryView } from "./model.ts";

export type LibsWindow =
  | "library"
  | "materialTest"
  | "intervalTest"
  | "focusTest"
  | "printCut"
  | "camera"
  | "findLaser"
  | "jobLog"
  | "batch"
  | "repeatTable"
  | "updates"
  | "logs";

type Open = { kind: LibsWindow; layer?: string | null } | null;

let open: Open = null;
const openSubs = new Set<() => void>();

/** Open one of the libs windows (the Library window docks; the others are dialogs). */
export function openLibsWindow(kind: LibsWindow, opts: { layer?: string | null } = {}): void {
  open = { kind, layer: opts.layer ?? null };
  for (const f of [...openSubs]) f();
}

export function closeLibsWindow(): void {
  open = null;
  for (const f of [...openSubs]) f();
}

export function useLibsWindow(): Open {
  return useSyncExternalStore(
    (f) => {
      openSubs.add(f);
      return () => openSubs.delete(f);
    },
    () => open,
  );
}

// ---- small observable values ----------------------------------------------------------------------------------------

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

// ---- preferences of the active device -------------------------------------------------------------------------------

const prefs = observable<{ prefs: DevicePrefs; supported: boolean | null }>({ prefs: NO_PREFS, supported: null });
let prefsLoading: Promise<void> | null = null;

export function refreshPrefs(): Promise<void> {
  prefsLoading ??= libsApi
    .prefs()
    .then((r) => prefs.set({ prefs: parsePrefs(r), supported: true }))
    .catch(() => prefs.set({ prefs: NO_PREFS, supported: false }))
    .finally(() => {
      prefsLoading = null;
    });
  return prefsLoading;
}

/** The active device's preferences; `supported` is false on engines without GET /api/prefs. */
export function useDevicePrefs(): { prefs: DevicePrefs; supported: boolean | null } {
  const v = prefs.use();
  useEffect(() => {
    if (v.supported === null) void refreshPrefs();
  }, [v.supported]);
  return v;
}

/** PUT /api/prefs with the given keys; the answer is the new preferences. */
export async function editPrefs(patch: Record<string, unknown>): Promise<DevicePrefs> {
  const r = await libsApi.prefsEdit(patch);
  const p = parsePrefs(r);
  prefs.set({ prefs: p, supported: true });
  return p;
}

// ---- the active device's profile ------------------------------------------------------------------------------------

const device = observable<{ id: string; profile: Record<string, unknown> | null } | null>(null);
let deviceLoaded = false;

export function refreshActiveDevice(): Promise<void> {
  deviceLoaded = true;
  return libsApi
    .activeDevice()
    .then((d) => device.set(d))
    .catch(() => device.set(null));
}

export function useActiveDevice(): { id: string; profile: Record<string, unknown> | null } | null {
  const v = device.use();
  useEffect(() => {
    if (!deviceLoaded) void refreshActiveDevice();
  }, []);
  return v;
}

// ---- the Library window's view --------------------------------------------------------------------------------------

const library = observable<{ view: LibraryView | null; supported: boolean | null }>({ view: null, supported: null });

export function refreshLibrary(): Promise<void> {
  return libsApi
    .library()
    .then((r) => library.set({ view: parseLibraryView(r), supported: true }))
    .catch(() => library.set({ view: null, supported: false }));
}

/** Use an answer that carries the library view (most library routes return it). */
export function takeLibraryView(r: unknown): void {
  if (r && typeof r === "object" && Array.isArray((r as { libraries?: unknown }).libraries)) library.set({ view: parseLibraryView(r), supported: true });
  else void refreshLibrary();
}

export function useLibrary(): { view: LibraryView | null; supported: boolean | null; refresh: () => Promise<void> } {
  const v = library.use();
  useEffect(() => {
    if (v.supported === null) void refreshLibrary();
  }, [v.supported]);
  return { ...v, refresh: useCallback(() => refreshLibrary(), []) };
}

// ---- camera overlay on the canvas -----------------------------------------------------------------------------------

export interface CameraOverlay {
  /** Object URL of the overlay PNG (field square, +Y up), or null. */
  url: string | null;
  fieldMm: number;
  opacity: number;
  shown: boolean;
}

export const cameraOverlay = observable<CameraOverlay>({ url: null, fieldMm: 0, opacity: 0.6, shown: false });

// ---- start point tool -----------------------------------------------------------------------------------------------

/** The Set Start Point tool is active (LightBurn's Start Point tool: click a node; Shift reverses; Ctrl clears). */
export const startPointTool = observable<boolean>(false);

// ---- per-browser mute of the job-finished sound ---------------------------------------------------------------------

const MUTE_KEY = "openetch.jobSoundMuted";

function readMute(): boolean {
  try {
    return localStorage.getItem(MUTE_KEY) === "1";
  } catch {
    return false;
  }
}

export const soundMuted = observable<boolean>(readMute());

export function setSoundMuted(v: boolean): void {
  soundMuted.set(v);
  try {
    localStorage.setItem(MUTE_KEY, v ? "1" : "0");
  } catch {
    // Storage unavailable: the choice lasts for this visit.
  }
}

// ---- the app's toast, for components rendered where no notify is passed down ----

let toast: (msg: string, kind?: "info" | "error") => void = () => undefined;

export function setLibsToast(fn: (msg: string, kind?: "info" | "error") => void): void {
  toast = fn;
}

export function libsToast(msg: string, kind?: "info" | "error"): void {
  toast(msg, kind);
}

// ---- engine events --------------------------------------------------------------------------------------------------

onEngineEvent((ev) => {
  if (ev.type === "machine" && ev.event === "prefs") void refreshPrefs();
  if (ev.type === "device_changed" || ev.type === "devices_changed") {
    void refreshPrefs();
    void refreshActiveDevice();
    void refreshLibrary();
  }
});
