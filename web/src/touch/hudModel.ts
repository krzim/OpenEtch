// The touch HUD's model (docs/ux/ui-plan-2026-11.md 1.2): which modifier toggles each tool offers, which actions the
// selection bar shows, and where the bar goes. Pure, so `npm test` covers it (hudModel.test.ts); TouchHud.tsx draws it.

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
    { id: "delete", label: "touch.delete", icon: "trash" },
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

export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * Where the action bar goes (client px, its top-left): centred above the selection with a gap, below it when there is
 * no room above, and kept inside the canvas with a margin; with no selection rect, at the canvas's bottom centre.
 */
export function placeActionBar(sel: Rect | null, canvas: Rect, w: number, h: number, gap = 12, margin = 8): { left: number; top: number } {
  const minL = canvas.left + margin;
  const maxL = Math.max(minL, canvas.right - margin - w);
  const minT = canvas.top + margin;
  const maxT = Math.max(minT, canvas.bottom - margin - h);
  if (!sel) return { left: clamp((canvas.left + canvas.right - w) / 2, minL, maxL), top: maxT };
  const left = clamp((sel.left + sel.right - w) / 2, minL, maxL);
  const above = sel.top - gap - h;
  if (above >= minT) return { left, top: Math.min(above, maxT) };
  const below = sel.bottom + gap;
  if (below <= maxT) return { left, top: below };
  // The selection fills the canvas: the top edge.
  return { left, top: minT };
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}
