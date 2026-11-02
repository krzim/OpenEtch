// The gesture arbiter (docs/ux/ui-plan-2026-11.md 1.2): one finger is the left mouse button, two fingers move the
// view. A pure state machine fed with pointer events and clock ticks; it answers with effects the Viewport carries out.
// - Mouse and pen presses start at once (exactly as before for the mouse).
// - A first finger opens a *pending* press: nothing happens on down. Moving past the touch slop starts the press (at
//   the down position, then a drag); lifting is a tap (press and release together); holding still for the long-press
//   time is a long-press (the touch right-click). A second finger while pending drops the press with no side effects
//   and starts pan/pinch, so a pinch never leaves a Line vertex, a measure point or a trim behind (T4).
// - A second finger during a drag aborts the drag (the touch Esc) and starts pan/pinch.
// - When every finger of a short, still two- or three-finger touch lifts, that is a two- or three-finger tap (undo /
//   redo).
// - Palm rejection (T15): while a pen is down, touches are ignored; for a second after the pen's last event, touches
//   only navigate (one finger pans).
// No DOM; `npm test` covers it (gesture.test.ts).

import type { Vec2 } from "../api/types.ts";
import { TOL, type PointerClass } from "../pointer.ts";

export type ArbiterInput =
  | { kind: "down"; id: number; cls: PointerClass; p: Vec2; t: number }
  | { kind: "move"; id: number; p: Vec2; t: number }
  | { kind: "up"; id: number; p: Vec2; t: number }
  | { kind: "cancel"; id: number; t: number }
  /** The clock (the Viewport arms a timer for `nextDeadline()`). */
  | { kind: "tick"; t: number }
  /** The long-press was not wanted (no context menu here): start the press after all. */
  | { kind: "promote"; t: number };

export type Effect =
  /** Start a one-pointer gesture at `p` (the down position): the old pointerdown. */
  | { kind: "press"; id: number; cls: PointerClass; p: Vec2; longPress?: boolean }
  /** The pressing pointer moved. */
  | { kind: "drag"; id: number; p: Vec2 }
  /** The press ended normally (the old pointerup). */
  | { kind: "release"; id: number; p: Vec2 }
  /** The press was cancelled: roll back whatever it did (the old pointercancel / second finger). */
  | { kind: "abort" }
  /** View navigation starts (or restarts, with another finger) with these points: one finger pans, two pinch. */
  | { kind: "navStart"; pts: Vec2[] }
  | { kind: "nav"; pts: Vec2[] }
  | { kind: "navEnd" }
  /** A long-press at `p` (the touch right-click). */
  | { kind: "longPress"; id: number; p: Vec2 }
  /** A quick, still tap with 2 or 3 fingers. */
  | { kind: "multiTap"; fingers: number };

export interface ArbiterOptions {
  /** Slop per class, px (default TOL). */
  slop: (cls: PointerClass) => number;
  longPressMs: number;
  /** After a pen event, touches only navigate for this long (ms). */
  palmMs: number;
  /** A multi-finger tap lasts at most this long (ms). */
  multiTapMs: number;
}

export const DEFAULT_ARBITER: ArbiterOptions = {
  slop: (cls) => TOL[cls].slop,
  longPressMs: TOL.touch.longPressMs,
  palmMs: 1000,
  multiTapMs: 350,
};

type State =
  | { k: "idle" }
  | { k: "pending"; id: number; start: Vec2; t0: number }
  /** A long-press fired; waits for the lift (or a promote). */
  | { k: "held"; id: number; start: Vec2 }
  | { k: "pressed"; id: number; cls: PointerClass }
  | { k: "nav"; ids: number[]; t0: number; moved: boolean; fingers: number }
  /** Waiting for the remaining pointers to lift; a multi-finger tap may still complete. */
  | { k: "dead"; tap: { t0: number; fingers: number } | null };

interface Ptr {
  cls: PointerClass;
  p: Vec2;
  start: Vec2;
}

const dist = (a: Vec2, b: Vec2) => Math.hypot(a[0] - b[0], a[1] - b[1]);

export class GestureArbiter {
  private s: State = { k: "idle" };
  private ptrs = new Map<number, Ptr>();
  /** Pen pointers down now. */
  private pens = new Set<number>();
  /** Time of the last pen event. */
  private penT = -Infinity;
  private o: ArbiterOptions;

  constructor(o: Partial<ArbiterOptions> = {}) {
    this.o = { ...DEFAULT_ARBITER, ...o };
  }

  /** The state's name (tests, and the Viewport's "is a gesture under way"). */
  get state(): State["k"] {
    return this.s.k;
  }

  /** Whether `id` is a pointer the arbiter tracks (a press or navigation finger). */
  tracks(id: number): boolean {
    return this.ptrs.has(id);
  }

  /** Whether view navigation (pan/pinch by fingers) is under way. */
  get navigating(): boolean {
    return this.s.k === "nav";
  }

  /** When the next `tick` is due (ms), or null. */
  nextDeadline(): number | null {
    return this.s.k === "pending" ? this.s.t0 + this.o.longPressMs : null;
  }

  /** Forget everything (the canvas lost its document, a reset). */
  reset(): Effect[] {
    const out: Effect[] = this.s.k === "pressed" ? [{ kind: "abort" }] : this.s.k === "nav" ? [{ kind: "navEnd" }] : [];
    this.s = { k: "idle" };
    this.ptrs.clear();
    this.pens.clear();
    return out;
  }

  feed(ev: ArbiterInput): Effect[] {
    switch (ev.kind) {
      case "down":
        return this.down(ev.id, ev.cls, ev.p, ev.t);
      case "move":
        return this.move(ev.id, ev.p, ev.t);
      case "up":
        return this.end(ev.id, ev.p, ev.t, false);
      case "cancel":
        return this.end(ev.id, this.ptrs.get(ev.id)?.p ?? [0, 0], ev.t, true);
      case "tick":
        return this.tick(ev.t);
      case "promote": {
        const s = this.s;
        if (s.k !== "held") return [];
        const cls = this.ptrs.get(s.id)?.cls ?? "touch";
        this.s = { k: "pressed", id: s.id, cls };
        return [{ kind: "press", id: s.id, cls, p: s.start, longPress: true }];
      }
    }
  }

  private touchPts(): Vec2[] {
    return [...this.ptrs.values()].filter((q) => q.cls === "touch").map((q) => q.p);
  }

  private navPts(ids: readonly number[]): Vec2[] {
    return ids.map((id) => this.ptrs.get(id)?.p).filter((p): p is Vec2 => !!p);
  }

  private startNav(t: number): Effect[] {
    const ids = [...this.ptrs.entries()].filter(([, q]) => q.cls === "touch").map(([id]) => id);
    const prev = this.s.k === "nav" ? this.s : null;
    this.s = { k: "nav", ids, t0: prev?.t0 ?? t, moved: prev?.moved ?? false, fingers: Math.max(prev?.fingers ?? 0, ids.length) };
    return [{ kind: "navStart", pts: this.navPts(ids) }];
  }

  private down(id: number, cls: PointerClass, p: Vec2, t: number): Effect[] {
    if (cls === "pen") {
      this.pens.add(id);
      this.penT = t;
    }
    // Palm rejection: a resting hand while the pen is down does nothing at all.
    if (cls === "touch" && this.pens.size > 0) return [];
    const navOnly = cls === "touch" && t - this.penT < this.o.palmMs;
    this.ptrs.set(id, { cls, p, start: p });
    const s = this.s;
    switch (s.k) {
      case "idle":
        if (cls !== "touch") {
          this.s = { k: "pressed", id, cls };
          return [{ kind: "press", id, cls, p }];
        }
        if (navOnly) return this.startNav(t);
        this.s = { k: "pending", id, start: p, t0: t };
        return [];
      case "pending":
      case "held":
        // A second finger before the press started: pan/pinch, nothing else happened.
        return cls === "touch" ? this.startNav(t) : this.ignore(id);
      case "pressed": {
        if (cls !== "touch" || this.ptrs.get(s.id)?.cls !== "touch") return this.ignore(id);
        // A second finger during a drag cancels the drag (the touch Esc) and navigates.
        const out: Effect[] = [{ kind: "abort" }];
        return [...out, ...this.startNav(t)];
      }
      case "nav":
        return cls === "touch" ? this.startNav(t) : this.ignore(id);
      case "dead":
        // After a pinch a finger stayed down: a new finger starts a new pinch.
        if (cls === "touch" && this.touchPts().length >= 2) {
          this.s = { k: "idle" };
          return this.startNav(t);
        }
        return [];
    }
  }

  /** A pointer that plays no part (a mouse during a touch gesture, ...): not tracked. */
  private ignore(id: number): Effect[] {
    this.ptrs.delete(id);
    return [];
  }

  private move(id: number, p: Vec2, t: number): Effect[] {
    const q = this.ptrs.get(id);
    if (!q) return [];
    if (q.cls === "pen") this.penT = t;
    q.p = p;
    const s = this.s;
    switch (s.k) {
      case "pending":
        if (s.id !== id || dist(p, s.start) <= this.o.slop(q.cls)) return [];
        this.s = { k: "pressed", id, cls: q.cls };
        return [
          { kind: "press", id, cls: q.cls, p: s.start },
          { kind: "drag", id, p },
        ];
      case "pressed":
        return s.id === id ? [{ kind: "drag", id, p }] : [];
      case "nav":
        if (!s.ids.includes(id)) return [];
        if (dist(p, q.start) > this.o.slop("touch")) s.moved = true;
        return [{ kind: "nav", pts: this.navPts(s.ids) }];
      default:
        return [];
    }
  }

  private end(id: number, p: Vec2, t: number, cancelled: boolean): Effect[] {
    const q = this.ptrs.get(id);
    if (this.pens.delete(id)) this.penT = t;
    if (!q) return [];
    q.p = p;
    this.ptrs.delete(id);
    const s = this.s;
    const left = this.ptrs.size;
    switch (s.k) {
      case "pending":
        if (s.id !== id) return [];
        this.s = left ? { k: "dead", tap: null } : { k: "idle" };
        // A tap: the press and its release together (nothing when cancelled).
        return cancelled
          ? []
          : [
              { kind: "press", id, cls: q.cls, p: s.start },
              { kind: "release", id, p },
            ];
      case "held":
        if (s.id !== id) return [];
        this.s = left ? { k: "dead", tap: null } : { k: "idle" };
        return [];
      case "pressed":
        if (s.id !== id) return [];
        this.s = left ? { k: "dead", tap: null } : { k: "idle" };
        return [cancelled ? { kind: "abort" } : { kind: "release", id, p }];
      case "nav": {
        const ids = s.ids.filter((x) => x !== id);
        const tap = !cancelled && !s.moved && s.fingers >= 2 ? { t0: s.t0, fingers: s.fingers } : null;
        if (ids.length >= 2) {
          // Still pinching with the others: restart from where they are.
          this.s = { ...s, ids };
          return [{ kind: "navStart", pts: this.navPts(ids) }];
        }
        this.s = left ? { k: "dead", tap } : { k: "idle" };
        const out: Effect[] = [{ kind: "navEnd" }];
        if (!left && tap && t - tap.t0 <= this.o.multiTapMs) out.push({ kind: "multiTap", fingers: tap.fingers });
        return out;
      }
      case "dead": {
        if (left) return [];
        this.s = { k: "idle" };
        if (!cancelled && s.tap && t - s.tap.t0 <= this.o.multiTapMs) return [{ kind: "multiTap", fingers: s.tap.fingers }];
        return [];
      }
      default:
        return [];
    }
  }

  private tick(t: number): Effect[] {
    const s = this.s;
    if (s.k !== "pending" || t - s.t0 < this.o.longPressMs) return [];
    this.s = { k: "held", id: s.id, start: s.start };
    return [{ kind: "longPress", id: s.id, p: this.ptrs.get(s.id)?.p ?? s.start }];
  }
}
