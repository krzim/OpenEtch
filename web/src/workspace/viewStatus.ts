// What the canvas reports to the status bar (pointer position, the snapped point and its label, zoom), and the view
// commands the hotkeys send to the canvas (zoom in/out, fit). A small store and an event target, so pointer moves
// re-render only the status bar and not the whole app.

import type { Vec2 } from "../api/types.ts";

export interface ViewStatus {
  /** Field point under the pointer, or null off the canvas. */
  cursor: Vec2 | null;
  /** Where a click or drag would land when a snap applies, with its label ("Center · Circle 1"; "" for the grid). */
  snap: { p: Vec2; label: string; kind: string } | null;
  /** Screen px per mm. */
  scale: number;
}

let status: ViewStatus = { cursor: null, snap: null, scale: 3 };
const listeners = new Set<() => void>();

export function getViewStatus(): ViewStatus {
  return status;
}

export function publishViewStatus(s: ViewStatus): void {
  const same =
    s.scale === status.scale &&
    s.cursor?.[0] === status.cursor?.[0] &&
    s.cursor?.[1] === status.cursor?.[1] &&
    s.snap?.p[0] === status.snap?.p[0] &&
    s.snap?.p[1] === status.snap?.p[1] &&
    s.snap?.label === status.snap?.label;
  if (same) return;
  status = s;
  for (const l of listeners) l();
}

export function subscribeViewStatus(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export type ViewCommand = "zoomIn" | "zoomOut" | "fit" | "selection";

const target: EventTarget | null = typeof EventTarget === "undefined" ? null : new EventTarget();

/** Ask the canvas to zoom or fit. */
export function sendViewCommand(cmd: ViewCommand): void {
  target?.dispatchEvent(new CustomEvent<ViewCommand>("view", { detail: cmd }));
}

export function onViewCommand(fn: (cmd: ViewCommand) => void): () => void {
  if (!target) return () => {};
  const h = (e: Event) => fn((e as CustomEvent<ViewCommand>).detail);
  target.addEventListener("view", h);
  return () => target.removeEventListener("view", h);
}

/**
 * How far (screen px, up) to move the view so a selection spanning `top`..`bottom` (canvas px) ends above `limit`, at
 * least `margin` clear of it, without pushing its top above `margin`. 0 when it already shows. Pure (tests).
 */
export function revealShift(top: number, bottom: number, limit: number, margin = 12): number {
  const over = bottom - (limit - margin);
  if (over <= 0) return 0;
  return Math.max(0, Math.min(over, top - margin));
}

/** Ask the canvas to keep the selection above `fraction` of its height (the phone's peek sheet covers the rest). */
export function sendRevealSelection(fraction: number): void {
  target?.dispatchEvent(new CustomEvent<number>("reveal", { detail: fraction }));
}

export function onRevealSelection(fn: (fraction: number) => void): () => void {
  if (!target) return () => {};
  const h = (e: Event) => fn((e as CustomEvent<number>).detail);
  target.addEventListener("reveal", h);
  return () => target.removeEventListener("reveal", h);
}
