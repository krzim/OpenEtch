// The touch HUD's model (docs/ux/ui-plan-2026-11.md 1.2): which modifier toggles each tool offers, which actions the
// selection bar shows, and when a sheet hides it (the canvas HUD, hud/hudLayout.ts, places the bar). Pure, so
// `npm test` covers it (hud.test.ts); TouchHud.tsx draws it.

import type { MsgKey } from "../workspace/i18n.ts";
import type { VirtualMod } from "./virtualMods.ts";

export interface ModToggle {
  mod: VirtualMod;
  /** Label, and an optional second line (what the same key does while dragging). */
  label: MsgKey;
  sub?: MsgKey;
  tip: MsgKey;
}

/**
 * The modifier toggles for a tool. They stand for LightBurn's keys, so each lights the same way the key would act:
 * in Select, Shift adds (and constrains a drag), Ctrl toggles (and turns snapping off while dragging), Alt snaps to
 * guides only; rotate handles take 15° (Shift) and 5° (Ctrl) steps; rectangles and ellipses get Square and From
 * centre; lines, pens and measuring 15° steps; Edit Nodes adds to the node selection.
 */
export function modToggles(tool: string, rotateMode: boolean): ModToggle[] {
  switch (tool) {
    case "select":
      if (rotateMode)
        return [
          { mod: "shift", label: "touch.mod15", tip: "touch.tip15" },
          { mod: "ctrl", label: "touch.mod5", tip: "touch.tip5" },
        ];
      return [
        { mod: "shift", label: "touch.modAdd", sub: "touch.modAddSub", tip: "touch.tipShift" },
        { mod: "ctrl", label: "touch.modToggle", sub: "touch.modToggleSub", tip: "touch.tipCtrl" },
        { mod: "alt", label: "touch.modGuides", tip: "touch.tipAlt" },
      ];
    case "rect":
    case "ellipse":
      return [
        { mod: "shift", label: "touch.modSquare", tip: "touch.tipSquare" },
        { mod: "ctrl", label: "touch.modFromCentre", tip: "touch.tipFromCentre" },
      ];
    case "circle":
      return [{ mod: "ctrl", label: "touch.modNoSnap", tip: "touch.tipNoSnap" }];
    case "line":
    case "pen":
    case "measure":
    case "pick":
      return [
        { mod: "shift", label: "touch.mod15", tip: "touch.tip15" },
        { mod: "ctrl", label: "touch.modNoSnap", tip: "touch.tipNoSnap" },
      ];
    case "polygon":
      return [{ mod: "shift", label: "touch.mod15", tip: "touch.tip15" }];
    case "node":
      return [{ mod: "shift", label: "touch.modAddNode", tip: "touch.tipAddNode" }];
    default:
      return [];
  }
}

export interface HudAction {
  /** TouchHud action id (touch/actions.ts runTouchAction). */
  id: string;
  label: MsgKey;
  icon: string;
}

/** The selection action bar: Copy, Paste, Duplicate, Delete, Group or Ungroup, Rotate 90, Flip H/V and the full menu. */
export function barActions(selectionCount: number, grouped: boolean): HudAction[] {
  if (selectionCount < 1) return [];
  const out: HudAction[] = [
    { id: "copy", label: "touch.copy", icon: "copy" },
    { id: "paste", label: "touch.paste", icon: "paste" },
    { id: "duplicate", label: "touch.duplicate", icon: "duplicate" },
    { id: "delete", label: "touch.delete", icon: "delete" },
  ];
  if (grouped) out.push({ id: "ungroup", label: "touch.ungroup", icon: "ungroup" });
  else if (selectionCount > 1) out.push({ id: "group", label: "touch.group", icon: "group" });
  out.push(
    { id: "rotate90", label: "touch.rotate90", icon: "rotate" },
    { id: "flipH", label: "touch.flipH", icon: "flipH" },
    { id: "flipV", label: "touch.flipV", icon: "flipV" },
    { id: "menu", label: "touch.more", icon: "more" },
  );
  return out;
}

/** One action bar button and the gap after it, px (touch.css: 44 px buttons, 2 px apart). */
const BAR_SLOT = 46;
/** The bar's padding and border, both sides, px. */
const BAR_CHROME = 10;

/** How many action bar buttons fit in `widthPx` (at least one, the ⋯). */
export function barCapacity(widthPx: number): number {
  return Math.max(1, Math.floor((widthPx - BAR_CHROME + 2) / BAR_SLOT));
}

/**
 * The action bar's buttons for `capacity` slots, so it never scrolls sideways: all of them when they fit, else the
 * first ones and a last ⋯ whose menu lists the rest (the full context menu after them).
 */
export function fitBarActions(actions: readonly HudAction[], capacity: number): { shown: HudAction[]; overflow: HudAction[] } {
  if (actions.length <= capacity) return { shown: [...actions], overflow: [] };
  const more = actions.find((a) => a.id === "menu") ?? { id: "menu", label: "touch.more", icon: "more" };
  const rest = actions.filter((a) => a.id !== "menu");
  const n = Math.max(0, capacity - 1);
  return { shown: [...rest.slice(0, n), more], overflow: rest.slice(n) };
}

export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * Whether a sheet whose top edge is at client y `coverTop` hides the selection: less than half of it (and less than
 * 16 px) is left above the sheet. A selection fully above the sheet is not covered.
 */
export function selectionCovered(sel: Rect, coverTop: number | null): boolean {
  if (coverTop === null) return false;
  const visible = Math.min(sel.bottom, coverTop) - sel.top;
  return visible < Math.max(16, (sel.bottom - sel.top) / 2);
}

