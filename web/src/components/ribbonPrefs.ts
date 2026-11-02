// The ribbon's own option (owner decision 6 of docs/ux/ui-plan-2026-11.md): LightBurn-style creation tools on a
// vertical rail at the canvas's left edge on desktop. Off by default. Kept in this browser's localStorage until
// Preferences (workspace/prefs.ts, P1) takes it over; toggled from the ribbon's right-click menu and command search.

import { useSyncExternalStore } from "react";

const KEY = "openetch.ribbon.creationRail";

function read(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

let rail = read();
const subs = new Set<() => void>();

export function getCreationRail(): boolean {
  return rail;
}

export function setCreationRail(on: boolean): void {
  rail = on;
  try {
    if (on) localStorage.setItem(KEY, "1");
    else localStorage.removeItem(KEY);
  } catch {
    // Storage blocked (private window): the choice lasts for this page only.
  }
  for (const f of subs) f();
}

export function useCreationRail(): boolean {
  return useSyncExternalStore(
    (fn) => {
      subs.add(fn);
      return () => {
        subs.delete(fn);
      };
    },
    getCreationRail,
    getCreationRail,
  );
}
