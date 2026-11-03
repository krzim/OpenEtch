// Where the toasts go so that none covers an open dialog, sheet or screen: the open dialog on top (components/Dialog.tsx's
// `.oe-dialog` box), else a full screen's own toast strip (`useToastStrip`: the production cockpit, which has no canvas
// HUD to place them), else the phone's bottom sheet when it is pulled up full (it leaves the canvas HUD no room), else
// null (the canvas HUD places them, clear of a lower sheet). App portals the toasts into that box as a strip in its own
// flow. On the phone a dialog fills the screen, and a toast sat over the Devices window's list; on the cockpit a fixed
// toast sat over the counter's Reset count and the slot buttons.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { layers } from "./dialogStack.ts";
import { subscribeHud } from "../touch/hudState.ts";

/** The mounted toast strips (the last mounted first), and the hosts to tell when one comes or goes. */
const strips: HTMLElement[] = [];
const stripListeners = new Set<() => void>();

function host(): HTMLElement | null {
  if (typeof document === "undefined") return null;
  if (layers.dialogCount() > 0) {
    // A dialog opened from another one is rendered inside it, so the last box in document order is the top one.
    const all = [...document.querySelectorAll<HTMLElement>(".oe-dialog")].filter((el) => el.isConnected);
    if (all.length) return all[all.length - 1];
  }
  const strip = strips.find((el) => el.isConnected);
  if (strip) return strip;
  return document.querySelector<HTMLElement>(".bottom-sheet.snap-full");
}

/**
 * A full screen's own toast strip: put the ref on an element in the screen's flow (under its scrolling content), and
 * while it is mounted the toasts are a strip inside it, taking room of their own instead of lying over the controls.
 */
export function useToastStrip<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    strips.unshift(el);
    for (const l of stripListeners) l();
    return () => {
      const i = strips.indexOf(el);
      if (i >= 0) strips.splice(i, 1);
      for (const l of stripListeners) l();
    };
  }, []);
  return ref;
}

export function useToastHost(): HTMLElement | null {
  const [el, setEl] = useState<HTMLElement | null>(null);
  useEffect(() => {
    let frame = 0;
    const find = () => {
      setEl(host());
      // A closing dialog leaves the stack in its effect cleanup, a sheet snaps after its transition: look again once
      // the page has settled.
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setEl(host()));
    };
    find();
    const offLayers = layers.subscribe(find);
    // The sheet's height comes through the touch HUD's store.
    const offHud = subscribeHud(find);
    stripListeners.add(find);
    return () => {
      offLayers();
      offHud();
      stripListeners.delete(find);
      cancelAnimationFrame(frame);
    };
  }, []);
  return el?.isConnected ? el : null;
}
