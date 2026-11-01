// LightBurn's "Flash layer" (Cuts / Layers right-click menu, or Shift+right-click on a layer): the shapes on a layer
// blink on the canvas for a moment so you can see which they are. A tiny store that the Cuts / Layers window sets and
// a canvas-tool wrapper (canvasTool.ts) that draws the blinking outlines above the objects.

import { useSyncExternalStore } from "react";
import type { CanvasTool } from "../canvasTool.ts";
import type { GDocument } from "../api/types.ts";
import { geometryCmds, toPathData } from "../geometry.ts";
import "./workspace.css";

/** How long the layer blinks, ms. */
export const FLASH_MS = 1800;

interface FlashState {
  layer: string | null;
  /** Bumped on every flash so the CSS animation restarts. */
  seq: number;
}

let state: FlashState = { layer: null, seq: 0 };
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setTimeout> | null = null;

function set(next: FlashState): void {
  state = next;
  for (const l of listeners) l();
}

/** Blink the shapes on `layer` for FLASH_MS. */
export function flashLayer(layer: string): void {
  if (timer !== null) clearTimeout(timer);
  set({ layer, seq: state.seq + 1 });
  timer = setTimeout(() => {
    timer = null;
    set({ layer: null, seq: state.seq });
  }, FLASH_MS);
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function useFlash(): FlashState {
  return useSyncExternalStore(subscribe, () => state, () => state);
}

/** `inner` plus the flashing layer's outlines drawn above the objects. */
export function withLayerFlash(inner: CanvasTool, doc: GDocument | null, flash: FlashState): CanvasTool {
  if (!flash.layer || !doc) return inner;
  const objs = doc.objects.filter((o) => o.layer === flash.layer);
  if (objs.length === 0) return inner;
  const d = objs.map((o) => toPathData(geometryCmds(o.geometry), o.transform)).join(" ");
  return {
    ...inner,
    render: (r) => (
      <>
        {inner.render(r)}
        <path key={flash.seq} className="layer-flash" d={d} strokeWidth={r.px(3)} pointerEvents="none" />
      </>
    ),
  };
}
