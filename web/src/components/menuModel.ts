// The pure half of the Menu primitive (components/Menu.tsx; docs/ux/ui-plan-2026-11.md P2 "Menu"): keyboard
// navigation (roving focus, submenus, Home/End, type-ahead, disabled items skipped) and placement (flip and clamp into
// the window, with a max-height so a long menu scrolls inside itself). No DOM; tested in menu.test.ts.

/** What navigation needs to know about one row. */
export interface NavItem {
  label: string;
  disabled?: boolean;
  /** A separator (never focused). */
  sep?: boolean;
  /** Opens a submenu. */
  submenu?: boolean;
}

/** What a key press in a menu does. */
export type NavAction =
  | { kind: "move"; index: number }
  | { kind: "openSubmenu"; index: number }
  | { kind: "closeSubmenu" }
  | { kind: "activate"; index: number }
  | { kind: "close" }
  | { kind: "closeAll" }
  | { kind: "none" };

const focusable = (it: NavItem | undefined) => !!it && !it.sep && !it.disabled;

/** The next focusable row after `from` in direction `dir` (wrapping), or -1 when there is none. */
export function stepIndex(items: readonly NavItem[], from: number, dir: 1 | -1): number {
  const n = items.length;
  if (!n) return -1;
  let i = from;
  for (let k = 0; k < n; k++) {
    i = (((i + dir) % n) + n) % n;
    if (focusable(items[i])) return i;
  }
  return -1;
}

/** The first focusable row, or -1. */
export function firstIndex(items: readonly NavItem[]): number {
  return stepIndex(items, -1, 1);
}

/** The last focusable row, or -1. */
export function lastIndex(items: readonly NavItem[]): number {
  return stepIndex(items, items.length, -1);
}

/**
 * Type-ahead: the next focusable row after `from` whose label starts with `typed` (case-insensitive). Repeating one
 * letter ("ss") cycles through the rows starting with it, as native menus do.
 */
export function typeAhead(items: readonly NavItem[], from: number, typed: string): number {
  const q = typed.toLowerCase();
  if (!q) return -1;
  const same = [...q].every((c) => c === q[0]);
  const needle = same ? q[0] : q;
  // A longer query keeps the current row when it still matches; a single letter moves on.
  const start = q.length > 1 && !same ? from - 1 : from;
  const n = items.length;
  for (let k = 1; k <= n; k++) {
    const i = (((start + k) % n) + n) % n;
    const it = items[i];
    if (focusable(it) && it.label.trim().toLowerCase().startsWith(needle)) return i;
  }
  return -1;
}

/**
 * The action for `key` with row `active` focused. `level` is 0 for the menu itself and 1+ inside a submenu (Left
 * closes a submenu, never the menu). `rtl` swaps Left and Right.
 */
export function menuNav(items: readonly NavItem[], active: number, key: string, ctx: { level: number; rtl?: boolean } = { level: 0 }): NavAction {
  const into = ctx.rtl ? "ArrowLeft" : "ArrowRight";
  const out = ctx.rtl ? "ArrowRight" : "ArrowLeft";
  switch (key) {
    case "ArrowDown": {
      const i = active < 0 ? firstIndex(items) : stepIndex(items, active, 1);
      return i < 0 ? { kind: "none" } : { kind: "move", index: i };
    }
    case "ArrowUp": {
      const i = active < 0 ? lastIndex(items) : stepIndex(items, active, -1);
      return i < 0 ? { kind: "none" } : { kind: "move", index: i };
    }
    case "Home": {
      const i = firstIndex(items);
      return i < 0 ? { kind: "none" } : { kind: "move", index: i };
    }
    case "End": {
      const i = lastIndex(items);
      return i < 0 ? { kind: "none" } : { kind: "move", index: i };
    }
    case into:
      return focusable(items[active]) && items[active].submenu ? { kind: "openSubmenu", index: active } : { kind: "none" };
    case out:
      return ctx.level > 0 ? { kind: "closeSubmenu" } : { kind: "none" };
    case "Enter":
    case " ":
      if (!focusable(items[active])) return { kind: "none" };
      return items[active].submenu ? { kind: "openSubmenu", index: active } : { kind: "activate", index: active };
    case "Escape":
      return ctx.level > 0 ? { kind: "closeSubmenu" } : { kind: "close" };
    case "Tab":
      return { kind: "closeAll" };
    default:
      return { kind: "none" };
  }
}

// --- placement ---------------------------------------------------------------------------------------------------

export interface RectLike {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export type Placement = "below" | "above" | "right" | "left" | "point";

/** Below this much room beside its button a dropdown is clamped into the window instead (and scrolls). */
export const MIN_MENU_H = 120;

export interface MenuPos {
  left: number;
  top: number;
  /** Cap on the menu's height; it scrolls inside itself when its content is taller. */
  maxHeight: number;
  /** Which way it finally opened (after flipping). */
  side: Placement;
}

/**
 * Where a menu of `size` goes for `anchor` (a point or a rectangle) inside a `view` of width × height, keeping
 * `margin` from every edge:
 * - point: at the point, flipped up / left when it would cross the bottom / right edge;
 * - below / above: under / over the rectangle, flipped when the other side has more room (`align` "end" lines up
 *   the right edges);
 * - right / left (submenus): beside the rectangle, flipped to the other side when it does not fit.
 * When nothing fits, it is clamped into the window and `maxHeight` makes it scroll.
 */
export function placeMenu(
  anchor: RectLike | { x: number; y: number },
  size: { width: number; height: number },
  view: { width: number; height: number },
  placement: Placement = "point",
  margin = 4,
  align: "start" | "end" = "start",
): MenuPos {
  // A rectangle is told apart by its right edge: a DOMRect also has x and y, and read as a point it opened a flyout on
  // top of its own button instead of beside it.
  const r: RectLike = "right" in anchor ? anchor : { left: anchor.x, top: anchor.y, right: anchor.x, bottom: anchor.y };
  const vw = view.width;
  const vh = view.height;
  const full = Math.max(0, vh - 2 * margin);
  const w = Math.min(size.width, Math.max(0, vw - 2 * margin));
  const h = Math.min(size.height, full);
  const clampX = (x: number) => Math.max(margin, Math.min(x, vw - margin - w));
  const clampY = (y: number) => Math.max(margin, Math.min(y, vh - margin - h));
  const gap = placement === "point" ? 0 : 2;

  if (placement === "below" || placement === "above") {
    const roomBelow = vh - margin - (r.bottom + gap);
    const roomAbove = r.top - gap - margin;
    let side: Placement = placement;
    if (placement === "below" && h > roomBelow && roomAbove > roomBelow) side = "above";
    if (placement === "above" && h > roomAbove && roomBelow > roomAbove) side = "below";
    const room = Math.max(0, side === "below" ? roomBelow : roomAbove);
    // Too little room on either side (an anchor in the middle of a short window): clamp into the window and scroll.
    const x = align === "end" ? r.right - w : r.left;
    if (h > room && room < MIN_MENU_H) return { left: clampX(x), top: clampY(r.top), maxHeight: full, side };
    const hh = Math.min(h, room);
    const top = side === "below" ? r.bottom + gap : r.top - gap - hh;
    return { left: clampX(x), top, maxHeight: Math.min(full, room), side };
  }

  if (placement === "right" || placement === "left") {
    const fitsRight = r.right + w <= vw - margin;
    const fitsLeft = r.left - w >= margin;
    let side: Placement = placement;
    if (placement === "right" && !fitsRight && (fitsLeft || r.left > vw - r.right)) side = "left";
    if (placement === "left" && !fitsLeft && (fitsRight || vw - r.right > r.left)) side = "right";
    const left = side === "right" ? r.right : r.left - w;
    return { left: clampX(left), top: clampY(r.top), maxHeight: full, side };
  }

  // A point (context menus): open down-right, flip up / left when that side has more room.
  let side: Placement = "point";
  let left = r.left;
  if (left + w > vw - margin && r.left - w >= margin) left = r.left - w;
  let top = r.top;
  if (top + h > vh - margin) {
    if (r.top - h >= margin) {
      top = r.top - h;
      side = "above";
    } else top = vh - margin - h;
  }
  top = Math.max(margin, top);
  return { left: clampX(left), top, maxHeight: Math.min(full, vh - margin - top), side };
}
