// STAGE A STUB (owner: P3). The touch HUD (docs/ux/ui-plan-2026-11.md 1.2): the modifier bar (bottom-left of the
// canvas, coarse pointers only) and the selection action bar (Copy, Paste, Duplicate, Delete, Group, Rotate 90, Flip,
// ⋯). P1 mounts it in `.center-stage` when the pointer is coarse. The signature below is final; until P3 lands it
// renders nothing.

import type { Tool } from "../components/Viewport.tsx";
import "./touch.css";

export interface TouchHudProps {
  /** The active canvas tool (picks the modifier toggles: Select, Rect/Ellipse, Line/Pen, Node, ...). */
  tool: Tool;
  /** Selected objects (the action bar shows with one or more). */
  selectionCount: number;
  /** An action bar button: "copy", "paste", "duplicate", "delete", "group", "ungroup", "rotate90", "flipH", "flipV",
   * "menu" (P3 owns the ids). */
  onAction: (id: string) => void;
}

export function TouchHud(_p: TouchHudProps) {
  return null;
}
