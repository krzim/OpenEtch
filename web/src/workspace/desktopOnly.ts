// What the phone leaves out (docs/ux/ui-plan-2026-11.md 1.1 "Off the phone"): node editing, pen and polygon, trim and
// weld, Docking, the Art library editor, device editing (with the lens wizard) and the GRBL console, plus main's Add
// Tabs tool and Manage User Tools. On the phone these are not in the tool strip, the dropdowns, the context strip or
// command search; More > Desktop features lists them with a hint and opens the ones that can run on a phone anyway.
// A desktop-only canvas tool that is active (picked from More) stays in the strip, so it can be switched off again.
// Pure, so `npm test` covers it (desktopOnly.test.ts).

import type { MenuItem } from "../components/Menu.tsx";
import type { LayoutMode } from "./layoutMode.ts";

export type DesktopFeature = "pen" | "polygon" | "nodes" | "trim" | "weld" | "addTabs" | "userTools" | "devices" | "docking" | "artLibrary" | "console";

/** In the order More > Desktop features lists them. */
export const DESKTOP_FEATURES: readonly DesktopFeature[] = ["nodes", "pen", "polygon", "trim", "weld", "addTabs", "userTools", "devices", "docking", "artLibrary", "console"];

/** The canvas tools (Viewport `Tool` ids) that are desktop-only. */
export const DESKTOP_TOOLS: ReadonlySet<string> = new Set(["pen", "polygon", "node", "trim"]);

/** Hotkey action ids (workspace/hotkeys.ts) of desktop-only features: left out of phone menus and command search. */
export const DESKTOP_ACTION_IDS: ReadonlySet<string> = new Set(["tool.nodes", "tool.weld", "tool.trim", "window.devices", "window.artLibrary"]);

/** Whether the layout offers desktop-only features at all (every layout but the phone). */
export function desktopFeatures(layout: LayoutMode): boolean {
  return layout !== "phone";
}

/** Whether canvas tool `tool` gets a button: always off the phone; on the phone unless desktop-only and not active. */
export function toolOffered(layout: LayoutMode, tool: string, activeTool: string): boolean {
  return desktopFeatures(layout) || !DESKTOP_TOOLS.has(tool) || tool === activeTool;
}

/** Whether command search offers hotkey action `actionId` in this layout (desktop-only actions not on the phone). */
export function actionOffered(layout: LayoutMode, actionId: string | undefined): boolean {
  return desktopFeatures(layout) || !actionId || !DESKTOP_ACTION_IDS.has(actionId);
}

/** Whether a menu item is desktop-only (flagged, or running a desktop-only action). */
export function isDesktopOnly(item: MenuItem): boolean {
  return !item.sep && (item.desktopOnly === true || (!!item.actionId && DESKTOP_ACTION_IDS.has(item.actionId)));
}

/**
 * `items` for the layout: unchanged off the phone; on the phone without desktop-only items (in submenus too), without
 * submenus left empty, and without separators at either end or next to each other.
 */
export function forLayout(items: readonly MenuItem[], layout: LayoutMode): MenuItem[] {
  if (desktopFeatures(layout)) return [...items];
  const kept: MenuItem[] = [];
  for (const it of items) {
    if (it.sep) {
      kept.push(it);
      continue;
    }
    if (isDesktopOnly(it)) continue;
    if (it.submenu) {
      const sub = forLayout(it.submenu, layout);
      if (!sub.some((x) => !x.sep)) continue;
      kept.push({ ...it, submenu: sub });
    } else kept.push(it);
  }
  const out: MenuItem[] = [];
  for (const it of kept) {
    if (it.sep && (out.length === 0 || out[out.length - 1].sep)) continue;
    out.push(it);
  }
  while (out.length && out[out.length - 1].sep) out.pop();
  return out;
}
