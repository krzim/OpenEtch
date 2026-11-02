// Pure rules of the application shell (docs/ux/ui-plan-2026-11.md 1.1, P1): the chrome budget per layout mode (the
// same numbers as tokens.css, which a test keeps in step), how the timeline and the status bar share the bottom, and
// the window title. No React and no DOM here, so `npm test` covers it.

import type { LayoutMode } from "./layoutMode.ts";

/** Heights (CSS px) of the rows around the canvas, and the widths of the rails beside it. */
export interface ChromeBudget {
  /** Top bar, without the top safe area. */
  top: number;
  /** Horizontal ribbon (0 when the tools are a vertical rail or a strip). */
  tool: number;
  /** Context strip (selection fields, tool options, hint). */
  context: number;
  /** Colour palette. */
  palette: number;
  /** Timeline (the whole merged row in compact and tablet). */
  timeline: number;
  /** Status bar (0 when merged into the timeline row, or on phones). */
  status: number;
  /** Phone bottom tabs, without the bottom safe area. */
  nav: number;
  /** Everything under the canvas: what toasts and the warnings panel sit above. */
  bottom: number;
  /** Vertical tool rail left of the canvas (tablet). */
  toolRail: number;
}

/** How the timeline and the status bar are laid out at the bottom. */
export type BottomRow = "split" | "merged" | "none";

/** Wide: timeline and status bar as two rows; compact and tablet: one merged row (pinned compact: split); phone: none. */
export function bottomRow(mode: LayoutMode, timelinePinned = false): BottomRow {
  if (mode === "phone") return "none";
  if (mode === "wide") return "split";
  if (mode === "compact" && timelinePinned) return "split";
  return "merged";
}

/** The chrome budget of a layout mode (tokens.css holds the same numbers as CSS variables). */
export function chromeBudget(mode: LayoutMode, timelinePinned = false): ChromeBudget {
  const row = bottomRow(mode, timelinePinned);
  let b: ChromeBudget;
  switch (mode) {
    case "wide":
      b = { top: 44, tool: 64, context: 32, palette: 24, timeline: 52, status: 24, nav: 0, bottom: 0, toolRail: 0 };
      break;
    case "compact":
      b =
        row === "split"
          ? { top: 40, tool: 40, context: 32, palette: 24, timeline: 52, status: 24, nav: 0, bottom: 0, toolRail: 0 }
          : { top: 40, tool: 40, context: 32, palette: 24, timeline: 28, status: 0, nav: 0, bottom: 0, toolRail: 0 };
      break;
    case "tablet":
      b = { top: 48, tool: 0, context: 44, palette: 36, timeline: 44, status: 0, nav: 0, bottom: 0, toolRail: 52 };
      break;
    case "phone":
      b = { top: 48, tool: 0, context: 0, palette: 0, timeline: 0, status: 0, nav: 56, bottom: 0, toolRail: 0 };
      break;
  }
  b.bottom = mode === "phone" ? b.nav : b.timeline + b.status;
  return b;
}

/** Rows above and below the canvas (`.center-stage`), as the acceptance criteria count them. */
export function chromeHeight(b: ChromeBudget): number {
  return b.top + b.tool + b.context + b.palette + b.timeline + b.status + b.nav;
}

/** Where toasts sit: above everything under the canvas. */
export function toastBottom(b: ChromeBudget): number {
  return b.bottom + 18;
}

/** The window / tab title: "<document> — OpenEtch", with "•" while there are unsaved changes. */
export function windowTitle(docName: string | null | undefined, dirty: boolean | null | undefined): string {
  const name = (docName ?? "").trim();
  if (!name) return dirty ? "• — OpenEtch" : "OpenEtch";
  return `${name}${dirty ? " •" : ""} — OpenEtch`;
}
