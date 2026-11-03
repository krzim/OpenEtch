// Where the toasts go so that none covers an open dialog or sheet: the open dialog on top (components/Dialog.tsx's
// `.oe-dialog` box), else the phone's bottom sheet when it is pulled up full (it leaves the canvas HUD no room), else
// null (the canvas HUD places them, clear of a lower sheet). App portals the toasts into that box as a strip in its own
// flow. On the phone a dialog fills the screen, and a toast sat over the Devices window's list.

import { useEffect, useState } from "react";
import { layers } from "./dialogStack.ts";
import { subscribeHud } from "../touch/hudState.ts";

function host(): HTMLElement | null {
  if (typeof document === "undefined") return null;
  if (layers.dialogCount() > 0) {
    // A dialog opened from another one is rendered inside it, so the last box in document order is the top one.
    const all = [...document.querySelectorAll<HTMLElement>(".oe-dialog")].filter((el) => el.isConnected);
    if (all.length) return all[all.length - 1];
  }
  return document.querySelector<HTMLElement>(".bottom-sheet.snap-full");
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
    return () => {
      offLayers();
      offHud();
      cancelAnimationFrame(frame);
    };
  }, []);
  return el?.isConnected ? el : null;
}
