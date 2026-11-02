// Where a floating popover goes (owner: P5; a local copy of the idea behind P2's `placeMenu`, plus the rule P5 needs):
// below its anchor, else above, else beside the panel it lives in (left first, towards the canvas), always inside the
// window with a max-height (it scrolls inside), and never over the rectangles it must not cover (Start and STOP, V13).

export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export type PopSide = "below" | "above" | "left" | "right";

export interface Placement {
  left: number;
  top: number;
  /** The popover's max-height (it scrolls inside when its content is taller). */
  maxHeight: number;
  side: PopSide;
}

export interface PlaceOptions {
  /** Gap to the window edges. */
  margin?: number;
  /** Gap to the anchor. */
  gap?: number;
  /** Shortest useful height before another side is tried. */
  minHeight?: number;
  /** For the left and right sides: the box to sit beside (the panel), default the anchor. */
  sideOf?: Rect;
}

const SIDES: readonly PopSide[] = ["below", "above", "left", "right"];

function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}

export function intersects(a: Rect, b: Rect): boolean {
  return a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
}

function overlapArea(a: Rect, b: Rect): number {
  const w = Math.min(a.right, b.right) - Math.max(a.left, b.left);
  const h = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top);
  return w > 0 && h > 0 ? w * h : 0;
}

/** One side's placement, or null when the popover's width does not fit there. */
function onSide(side: PopSide, anchor: Rect, size: { w: number; h: number }, view: { w: number; h: number }, o: Required<Omit<PlaceOptions, "sideOf">> & { sideOf: Rect }): Placement | null {
  const { margin: m, gap: g } = o;
  const w = Math.min(size.w, view.w - 2 * m);
  const maxTall = Math.max(0, view.h - 2 * m);
  if (side === "below" || side === "above") {
    const left = clamp(anchor.left, m, view.w - m - w);
    if (side === "below") {
      const top = anchor.bottom + g;
      const avail = view.h - m - top;
      return { left, top, maxHeight: Math.max(0, Math.min(size.h, avail)), side };
    }
    const avail = anchor.top - g - m;
    const hh = Math.max(0, Math.min(size.h, avail));
    return { left, top: anchor.top - g - hh, maxHeight: hh, side };
  }
  const box = o.sideOf;
  const left = side === "left" ? box.left - g - w : box.right + g;
  if (left < m || left + w > view.w - m) return null;
  const hh = Math.min(size.h, maxTall);
  const top = clamp(anchor.top, m, view.h - m - hh);
  return { left, top, maxHeight: hh, side };
}

function rectOf(p: Placement, w: number): Rect {
  return { left: p.left, top: p.top, right: p.left + w, bottom: p.top + p.maxHeight };
}

/**
 * Place a popover of natural `size` for `anchor` in a `view`, never over `avoid`. Among the sides that do not touch
 * `avoid`, the first that holds the whole popover wins, else the first with at least `minHeight` (it scrolls inside),
 * else the one with the most height; when every side touches `avoid`, the one that covers the least of it.
 */
export function placePopover(anchor: Rect, size: { w: number; h: number }, view: { w: number; h: number }, avoid: readonly Rect[] = [], opts: PlaceOptions = {}): Placement {
  const o = { margin: opts.margin ?? 8, gap: opts.gap ?? 4, minHeight: opts.minHeight ?? 160, sideOf: opts.sideOf ?? anchor };
  const w = Math.min(size.w, view.w - 2 * o.margin);
  const all = SIDES.map((s) => onSide(s, anchor, size, view, o)).filter((p): p is Placement => p !== null && p.maxHeight > 0);
  const clear = all.filter((p) => !avoid.some((a) => intersects(rectOf(p, w), a)));
  const whole = clear.find((p) => p.maxHeight >= size.h);
  if (whole) return whole;
  const enough = Math.min(size.h, o.minHeight);
  const good = clear.find((p) => p.maxHeight >= enough);
  if (good) return good;
  if (clear.length) return clear.reduce((a, b) => (b.maxHeight > a.maxHeight ? b : a));
  if (all.length) {
    const cost = (p: Placement) => avoid.reduce((s, a) => s + overlapArea(rectOf(p, w), a), 0);
    return all.reduce((a, b) => (cost(b) < cost(a) ? b : a));
  }
  // Nothing fits at all (a tiny window): top-left corner, as tall as the window allows.
  return { left: o.margin, top: o.margin, maxHeight: Math.max(0, view.h - 2 * o.margin), side: "below" };
}
