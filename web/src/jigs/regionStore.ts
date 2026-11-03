// The re-mark region shared by the canvas tool (jigs/useJigsUi.tsx) and every Start (laser/startFlow.ts reads it in
// `startCtxOf`): while a region is set, Start runs only the planned toolpath inside it ("Start · region"). Pure apart
// from `useSyncExternalStore`; node tests import it.

import { useSyncExternalStore } from "react";
import type { Vec2 } from "../api/types.ts";

let region: Vec2[] | null = null;
const listeners = new Set<() => void>();

export function getRemarkRegion(): Vec2[] | null {
  return region;
}

export function setRemarkRegion(r: Vec2[] | null): void {
  region = r && r.length >= 3 ? r : null;
  listeners.forEach((f) => f());
}

export function useRemarkRegion(): Vec2[] | null {
  return useSyncExternalStore(
    (f) => {
      listeners.add(f);
      return () => listeners.delete(f);
    },
    () => region,
    () => region,
  );
}
