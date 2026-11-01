// "Keep circles round": a per-browser preference shared by the Properties panel (its lock toggle next to Diameter) and
// the canvas (scale handles on a lone circle scale it uniformly while the lock is on). On by default, like Fusion's
// circle, whose only size is its diameter. Stored in localStorage when available; every access is guarded.

import { useSyncExternalStore } from "react";

const KEY = "openetch.circleLock";
const listeners = new Set<() => void>();

let value = (() => {
  try {
    return localStorage.getItem(KEY) !== "0";
  } catch {
    return true;
  }
})();

export function circleLocked(): boolean {
  return value;
}

export function setCircleLocked(v: boolean): void {
  value = v;
  try {
    localStorage.setItem(KEY, v ? "1" : "0");
  } catch {
    // Private mode or blocked storage: the setting lasts for this page only.
  }
  for (const l of listeners) l();
}

/** React hook: the current lock and a setter. */
export function useCircleLock(): [boolean, (v: boolean) => void] {
  const v = useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    circleLocked,
    circleLocked,
  );
  return [v, setCircleLocked];
}
