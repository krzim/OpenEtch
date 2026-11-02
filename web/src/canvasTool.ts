// The contract between the Viewport and the editing tools that live outside it (components/EditTools.tsx): node
// editing, line/pen/polygon drawing, measure, trim, point picks for dialogs, rulers and manual guides. The Viewport
// keeps pan/zoom, selection and the rectangle/ellipse/circle tools; for everything else it hands primary-pointer
// presses to the canvas tool first, with a context that converts coordinates, snaps (snapping.ts, including the
// guides' snap parts) and shows the snap glyph and readout. Types only.

import type { ReactNode } from "react";
import type { GDocument, Vec2 } from "./api/types.ts";
import type { ConstraintLine, Mods } from "./editing.ts";
import type { PointerClass, Tolerances } from "./pointer.ts";
import type { SnapHit, SnapPart, SnapPoint } from "./snapping.ts";

/** Tools implemented by the canvas tool (the Viewport's own are select, rect, ellipse and circle). */
export type CanvasToolId = "node" | "line" | "pen" | "polygon" | "measure" | "trim" | "fill" | "pick" | "text" | "barcode";

/**
 * "text" and "barcode" are handled by text/useTextTools.tsx and "fill" (Fill Region) by fill/useFillRegionTool.tsx,
 * which wrap EditTools' canvas tool.
 */
export const CANVAS_TOOLS: readonly CanvasToolId[] = ["node", "line", "pen", "polygon", "measure", "trim", "fill", "pick", "text", "barcode"];

export function isCanvasTool(t: string): t is CanvasToolId {
  return (CANVAS_TOOLS as readonly string[]).includes(t);
}

/** What a canvas tool can ask the Viewport during a pointer event. */
export interface CanvasCtx {
  doc: GDocument;
  /** Screen px per mm. */
  scale: number;
  /** Canvas size in px. */
  size: Vec2;
  toField: (s: Vec2) => Vec2;
  toScreen: (p: Vec2) => Vec2;
  /** Snap a field point (Ctrl toggles snapping, Alt snaps to guides only), skipping objects `exclude`. */
  snap: (p: Vec2, m: Mods, o?: { exclude?: readonly string[]; extra?: SnapPoint[] }) => { p: Vec2; hit: SnapHit | null };
  /** Show (or clear) the snap glyph and label. Also clears the Shift constraint guide. */
  showSnap: (hit: SnapHit | null) => void;
  /** Show (or clear) the guide line of a Shift-constrained drag (editing.ts constraintLine), until the next showSnap. */
  showConstraint: (line: ConstraintLine | null) => void;
  /** Text after the cursor position in the bottom-left readout, or null. */
  readout: (text: string | null) => void;
  /** The object under a field point (click tolerance), or null. */
  hitObject: (p: Vec2) => string | null;
  /** The pointer class of the gesture (mouse when absent): touch commits point tools on lift. */
  pointerClass?: PointerClass;
  /** Screen-px tolerances for that pointer class (pointer.ts TOL; the mouse row when absent). */
  tol?: Tolerances;
}

/** What a canvas tool needs to draw. */
export interface RenderCtx {
  scale: number;
  size: Vec2;
  /** Screen px to mm (stroke widths stay constant on screen). */
  px: (n: number) => number;
  toField: (s: Vec2) => Vec2;
  toScreen: (p: Vec2) => Vec2;
  /** Snap against the document as rendered now (rulers have no pointer-event context of their own). */
  snap?: CanvasCtx["snap"];
}

export interface PointerInfo {
  target: Element | null;
  pointerType: string;
  /** Event time stamp, ms. */
  time: number;
  /** Screen position in the canvas, px. */
  screen: Vec2;
  /** The press came from a touch long-press that nothing else wanted (it grabs a guide on touch). */
  longPress?: boolean;
}

export interface CanvasTool {
  /** A primary press (or touch). Return true when the tool takes the gesture; the Viewport then forwards drag/up. */
  down: (f: Vec2, m: Mods, info: PointerInfo, ctx: CanvasCtx) => boolean;
  drag: (f: Vec2, m: Mods, ctx: CanvasCtx) => void;
  up: (f: Vec2, m: Mods, ctx: CanvasCtx) => void;
  /** The gesture was cancelled (pointercancel, a second finger): roll back whatever the press added. */
  cancel: () => void;
  /** Hover without a press. Return true to suppress the Viewport's own hover snap glyph. */
  hover: (f: Vec2, m: Mods, ctx: CanvasCtx) => boolean;
  /** Drawn inside the field-frame group (mm, +Y up), above the objects. */
  render: (r: RenderCtx) => ReactNode;
  /** Drawn in the field frame under the images and objects (above the field and grid): the camera overlay. */
  underlay?: (r: RenderCtx) => ReactNode;
  /** Drawn over the canvas in screen space (HTML): rulers, the tool options bar. */
  overlay: (r: RenderCtx) => ReactNode;
  /** Extra snap targets (manual guides). */
  snapParts: readonly SnapPart[];
}
