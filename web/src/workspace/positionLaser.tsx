// LightBurn's Position Laser (Alt+L): the next click on the canvas points the laser there. On a galvo that is the red
// pointer held at the clicked field point (POST /api/machine/pointer, light only, nothing fires); Esc or a second
// Alt+L ends it without a click. A canvas-tool wrapper (canvasTool.ts) takes the click while the mode is on.

import { useSyncExternalStore } from "react";
import type { CanvasTool } from "../canvasTool.ts";
import type { Vec2 } from "../api/types.ts";

let on = false;
const listeners = new Set<() => void>();
function set(v: boolean): void {
  on = v;
  for (const l of listeners) l();
}
const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
};

export function togglePositionLaser(v?: boolean): void {
  set(v ?? !on);
}

export function usePositionLaser(): boolean {
  return useSyncExternalStore(subscribe, () => on, () => on);
}

/** `inner` with the click taken while Position Laser is on: `point` gets the field point, then the mode ends. */
export function withPositionLaser(inner: CanvasTool, active: boolean, point: (p: Vec2) => void, bar: (end: () => void) => JSX.Element): CanvasTool {
  if (!active) return inner;
  return {
    ...inner,
    down: (f) => {
      set(false);
      point(f);
      return true;
    },
    drag: () => undefined,
    up: () => undefined,
    hover: (f, _m, ctx) => {
      ctx.readout(`→ ${f[0].toFixed(2)}, ${f[1].toFixed(2)}`);
      return true;
    },
    overlay: (r) => (
      <>
        {inner.overlay(r)}
        {bar(() => set(false))}
      </>
    ),
  };
}
