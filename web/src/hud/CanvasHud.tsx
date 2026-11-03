// The canvas HUD: one shared layout for every control that floats over the canvas (hud/hudLayout.ts places them).
//
// - `<CanvasHud/>` is mounted once in `.center-stage`, over the canvas. Its box, minus the safe-area bands, is the
//   HUD's area.
// - `<HudItem id slot rank>` puts its children into a named slot (portalled into the HUD): the view buttons, the
//   cursor / field readout, the off-field warning, a tool's option bar on the phone, the preview player row, the touch
//   modifier bar, the selection action bar (anchored above the selection), the phone's undo / redo / tools buttons,
//   the tablet's Properties pill, the empty-document card, the snap bubble.
// - `useHudFixed(spec)` places an element that lives elsewhere (`position: fixed`, its own layer) in a slot, after the
//   controls: the toasts, which so never cover a control.
// - Elements with `data-hud-obstacle` are kept clear of without being moved: the rulers and their corner, the phone's
//   bottom sheet at its current height, a dock overlay.
//
// Layout runs after every change (an item rendered or resized, an obstacle resized, the HUD resized) before the next
// paint, by writing each item's left / top / width straight to its element.

import { useLayoutEffect, useRef, useSyncExternalStore, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { subscribeHud } from "../touch/hudState.ts";
import { layoutHud, safeArea, type Box, type HudItemSpec, type HudPlacement, type HudSlot } from "./hudLayout.ts";
import { safeInsets } from "./safeInsets.ts";
import "./hud.css";

/** A client-px rectangle (getBoundingClientRect's shape). */
export interface ClientRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface HudItemOptions {
  id: string;
  slot: HudSlot;
  rank: number;
  /** May shrink to this width (the item wraps). */
  minW?: number;
  /** Cap on the natural width. */
  maxW?: number;
  minH?: number;
  optional?: boolean;
  /** A message (toasts, warnings): placed after the required controls, before the optional ones. */
  late?: boolean;
  /** A stack (the toasts): when the whole stack has no room it may show only its newest (last) children. */
  stack?: boolean;
  /** Anchored next to this client rect instead of the slot's edge (the slot is the fallback). */
  anchor?: { rect: ClientRect; side: "above" | "below"; gap?: number } | null;
}

interface Reg {
  el: HTMLElement;
  opts: () => HudItemOptions;
  fixed: boolean;
}

const regs = new Map<string, Reg>();
let root: HTMLElement | null = null;
const listeners = new Set<() => void>();
let ro: ResizeObserver | null = null;
const observed = new Set<Element>();
let queued = false;
/** The last layout, for tests and the e2e harness (window.__hud). */
let last: Record<string, HudPlacement> = {};

function emit(): void {
  for (const l of listeners) l();
}
function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}
const getRoot = (): HTMLElement | null => root;

/** The width of the HUD's area (the canvas minus the safe-area bands), for items that pick their content to fit it. */
let areaWidth = 0;
const areaListeners = new Set<() => void>();
function subscribeArea(fn: () => void): () => void {
  areaListeners.add(fn);
  return () => areaListeners.delete(fn);
}

/** The HUD area's width in px (0 before the first layout); re-renders when it changes. */
export function useHudAreaWidth(): number {
  return useSyncExternalStore(subscribeArea, () => areaWidth, () => 0);
}

/** Lay the HUD out again before the next paint (coalesced). */
export function invalidateHud(): void {
  if (queued) return;
  queued = true;
  queueMicrotask(() => {
    queued = false;
    relayout();
  });
}

function observe(el: Element): void {
  if (!ro || observed.has(el)) return;
  observed.add(el);
  ro.observe(el);
}

function clearFixed(el: HTMLElement): void {
  el.style.left = "";
  el.style.top = "";
  el.style.width = "";
  el.style.maxWidth = "";
  el.style.bottom = "";
  el.style.transform = "";
  el.style.maxHeight = "";
  el.style.overflow = "";
  el.style.justifyContent = "";
  delete el.dataset.hudPlaced;
}

function relayout(): void {
  // Forget elements that left the page.
  for (const el of observed)
    if (!el.isConnected) {
      ro?.unobserve(el);
      observed.delete(el);
    }
  if (!root || !root.isConnected) {
    for (const r of regs.values()) if (r.fixed) clearFixed(r.el);
    return;
  }
  const rr = root.getBoundingClientRect();
  if (rr.width < 1 || rr.height < 1) return;
  const safe = safeInsets();
  const area = safeArea({ x: rr.left, y: rr.top, w: rr.width, h: rr.height }, { w: window.innerWidth, h: window.innerHeight }, safe);
  if (Math.abs(area.w - areaWidth) > 0.5) {
    areaWidth = area.w;
    for (const l of areaListeners) l();
  }
  const local = (r: ClientRect): Box => ({ x: r.left - rr.left, y: r.top - rr.top, w: r.right - r.left, h: r.bottom - r.top });

  const obstacles: Box[] = [];
  for (const el of document.querySelectorAll<HTMLElement>("[data-hud-obstacle]")) {
    observe(el);
    const b = el.getBoundingClientRect();
    if (b.width < 1 || b.height < 1) continue;
    if (getComputedStyle(el).visibility === "hidden") continue;
    obstacles.push(local(b));
  }

  // Natural sizes.
  const entries: { reg: Reg; o: HudItemOptions; spec: HudItemSpec; natW: number }[] = [];
  for (const reg of regs.values()) {
    const el = reg.el;
    if (!el.isConnected) continue;
    observe(el);
    const o = reg.opts();
    el.style.width = "";
    el.style.maxHeight = "";
    if (reg.fixed) el.style.maxWidth = "";
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    if (w < 1 || h < 1) {
      delete el.dataset.hudHidden;
      continue;
    }
    const natW = Math.min(w, o.maxW ?? Infinity);
    const spec: HudItemSpec = { id: o.id, slot: o.slot, rank: o.rank, w: natW, h, minW: o.minW, minH: o.minH, optional: o.optional, late: o.late };
    if (o.anchor) spec.anchor = { box: local(o.anchor.rect), side: o.anchor.side, gap: o.anchor.gap };
    if (o.stack && el.children.length > 1) {
      const b = el.getBoundingClientRect().bottom;
      const kids = [...el.children];
      spec.shorter = kids.slice(1).map((k) => b - k.getBoundingClientRect().top);
    }
    entries.push({ reg, o, spec, natW });
  }

  const specs = entries.map((e) => e.spec);
  // Held to its maxW (narrower than its content): it gets that width, and wraps.
  const capped = (e: (typeof entries)[number]) => e.o.maxW !== undefined && e.natW >= e.o.maxW - 0.5;
  let out = layoutHud(area, obstacles, specs);
  // An item given less than its natural width wraps and grows taller: measure it there and lay out again.
  for (let pass = 0; pass < 3; pass++) {
    let changed = false;
    for (const e of entries) {
      const p = out[e.spec.id];
      if (!p || p.hidden) continue;
      if (p.w < e.natW - 0.5 || capped(e)) {
        e.reg.el.style.width = `${p.w}px`;
        const h = e.reg.el.offsetHeight;
        if (Math.abs(h - e.spec.h) > 0.5) {
          e.spec.h = h;
          changed = true;
        }
      }
    }
    if (!changed) break;
    out = layoutHud(area, obstacles, specs);
  }

  for (const e of entries) {
    const p = out[e.spec.id];
    const el = e.reg.el;
    if (!p) continue;
    el.style.width = p.w < e.natW - 0.5 || capped(e) ? `${p.w}px` : "";
    el.style.maxHeight = p.maxH !== undefined ? `${p.maxH}px` : "";
    if (p.hidden) el.dataset.hudHidden = "";
    else delete el.dataset.hudHidden;
    if (p.overlaps) el.dataset.hudOverlaps = "";
    else delete el.dataset.hudOverlaps;
    if (e.reg.fixed) {
      // A stack held shorter shows its newest (last) children.
      el.style.overflow = p.maxH !== undefined ? "hidden" : "";
      el.style.justifyContent = p.maxH !== undefined ? "flex-end" : "";
      el.style.left = `${rr.left + p.x}px`;
      el.style.top = `${rr.top + p.y}px`;
      el.style.bottom = "auto";
      el.style.transform = "none";
    } else {
      el.style.left = `${p.x}px`;
      el.style.top = `${p.y}px`;
    }
    el.dataset.hudPlaced = "";
  }
  last = out;
  (window as unknown as { __hud?: unknown }).__hud = { area, obstacles, items: specs, out };
}

/** The last layout (for tests). */
export function lastHudLayout(): Record<string, HudPlacement> {
  return last;
}

/** The HUD layer: mount once in `.center-stage`, after the canvas. */
export function CanvasHud() {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    root = ref.current;
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(() => invalidateHud());
      if (root) observe(root);
    }
    emit();
    const onResize = () => invalidateHud();
    window.addEventListener("resize", onResize);
    window.visualViewport?.addEventListener("resize", onResize);
    // The sheet's height and the selection's place come through the touch HUD's store.
    const off = subscribeHud(onResize);
    invalidateHud();
    return () => {
      window.removeEventListener("resize", onResize);
      window.visualViewport?.removeEventListener("resize", onResize);
      off();
      ro?.disconnect();
      ro = null;
      observed.clear();
      if (root === ref.current) root = null;
      emit();
      invalidateHud();
    };
  }, []);
  return (
    <div ref={ref} className="canvas-hud" />
  );
}

function useRegister(o: HudItemOptions, fixed: boolean, el: () => HTMLElement | null, dep: unknown) {
  const opts = useRef(o);
  opts.current = o;
  useLayoutEffect(() => {
    const e = el();
    if (!e) return;
    const reg: Reg = { el: e, opts: () => opts.current, fixed };
    regs.set(o.id, reg);
    invalidateHud();
    return () => {
      if (regs.get(o.id) === reg) regs.delete(o.id);
      if (fixed) clearFixed(e);
      invalidateHud();
    };
  }, [o.id, dep]);
  // Any render may change the item's content or anchor.
  useLayoutEffect(() => invalidateHud());
}

/** A control in a HUD slot (portalled into the HUD layer while one is mounted). */
export function HudItem(p: HudItemOptions & { className?: string; passThrough?: boolean; children: ReactNode }) {
  const host = useSyncExternalStore(subscribe, getRoot, () => null);
  const ref = useRef<HTMLDivElement>(null);
  const { className, passThrough, children, ...o } = p;
  useRegister(o, false, () => ref.current, host);
  const node = (
    <div ref={ref} className={`hud-item${passThrough ? " hud-pass" : ""}${className ? ` ${className}` : ""}`} data-hud-id={o.id} data-hud-slot={o.slot}>
      {children}
    </div>
  );
  return host ? createPortal(node, host) : node;
}

/** Place an element that lives outside the HUD (fixed, its own layer: the toasts) in a slot, after the controls. */
export function useHudFixed<T extends HTMLElement>(o: HudItemOptions) {
  const ref = useRef<T>(null);
  const host = useSyncExternalStore(subscribe, getRoot, () => null);
  useRegister(o, true, () => ref.current, host);
  return ref;
}
