// Runtime registries behind the dockable windows (Dock.tsx, DockHost.tsx): which windows are mounted now (their id and
// title, for the Window menu and the tab strips), and the slot element each host offers a window to render into (a
// Dock portals its frame into its slot, so moving a window between the side panels, the bottom strip and a floating
// frame needs no change in the component that renders it). Small external stores read with useSyncExternalStore.

import { useSyncExternalStore } from "react";
import { getLayout, subscribeLayout, type Layout } from "./layout.ts";

function store<T>(initial: T) {
  let value = initial;
  const listeners = new Set<() => void>();
  return {
    get: () => value,
    set: (v: T) => {
      if (v === value) return;
      value = v;
      for (const l of listeners) l();
    },
    subscribe: (fn: () => void) => {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
  };
}

/** Mounted windows: id -> title, in mount order. */
const mounted = store<ReadonlyMap<string, string>>(new Map());
/** Host slots: window id -> element. */
const slots = store<ReadonlyMap<string, HTMLElement>>(new Map());
/** How many hosts are mounted (0: Docks render where they are, as before the layout existed). */
const hosts = store(0);

export function registerWindow(id: string, title: string): void {
  const cur = mounted.get();
  if (cur.get(id) === title) return;
  const next = new Map(cur);
  next.set(id, title);
  mounted.set(next);
}

export function unregisterWindow(id: string): void {
  const cur = mounted.get();
  if (!cur.has(id)) return;
  const next = new Map(cur);
  next.delete(id);
  mounted.set(next);
}

export function useMountedWindows(): ReadonlyMap<string, string> {
  return useSyncExternalStore(mounted.subscribe, mounted.get, mounted.get);
}

export function mountedWindows(): ReadonlyMap<string, string> {
  return mounted.get();
}

const slotRefs = new Map<string, (el: HTMLElement | null) => void>();

/** A callback ref for window `id`'s slot in a host (the same function for an id, so React calls it only on (un)mount). */
export function slotRef(id: string): (el: HTMLElement | null) => void {
  const have = slotRefs.get(id);
  if (have) return have;
  const fn = (el: HTMLElement | null) => {
    const cur = slots.get();
    if (el ? cur.get(id) === el : !cur.has(id)) return;
    // React detaches the old element's ref (null) before it attaches a new one, so a moved slot ends up set.
    if (!el) {
      const next = new Map(cur);
      next.delete(id);
      slots.set(next);
      return;
    }
    const next = new Map(cur);
    next.set(id, el);
    slots.set(next);
  };
  slotRefs.set(id, fn);
  return fn;
}

export function useSlot(id: string): HTMLElement | null {
  const m = useSyncExternalStore(slots.subscribe, slots.get, slots.get);
  return m.get(id) ?? null;
}

export function hostMounted(delta: 1 | -1): void {
  hosts.set(Math.max(0, hosts.get() + delta));
}

export function useHostsActive(): boolean {
  return useSyncExternalStore(hosts.subscribe, () => hosts.get() > 0, () => false);
}

export function useLayout(): Layout {
  return useSyncExternalStore(subscribeLayout, getLayout, getLayout);
}
