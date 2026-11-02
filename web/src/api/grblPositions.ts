// A GRBL gantry's head on the canvas and its saved Move positions (pure parts and a tiny store; the UI is in
// components/GrblMove.tsx and components/HeadMarker.tsx).
//
// - Show Last Position (LightBurn's Laser window button): a crosshair on the canvas at the head's last known position,
//   the field position of the last status report the Move or Console window got (GET /api/machine/grbl?refresh=true).
//   It is only a drawing: it never asks the board for anything by itself and never moves the head.
// - Saved positions: named machine positions stored with the device in the engine's preferences (PUT /api/prefs
//   `move_positions`, UI only: agents cannot save them). Going to one is an ordinary laser-off jog (POST /api/machine/jog).

import type { GrblProfileView } from "./grbl.ts";

/** A saved Move position: machine mm from the origin corner (`engine::prefs::MovePosition`). */
export interface MovePosition {
  name: string;
  x_mm: number;
  y_mm: number;
}

export const MAX_POSITIONS = 32;
export const MAX_POSITION_NAME = 40;

function obj(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

/** The device's saved positions from GET /api/prefs (`device_prefs.move_positions`), defensively. */
export function parsePositions(prefs: unknown): MovePosition[] {
  const d = obj(obj(prefs)?.device_prefs);
  const list = Array.isArray(d?.move_positions) ? d.move_positions : [];
  return list.flatMap((m) => {
    const o = obj(m);
    const ok = o && typeof o.name === "string" && Number.isFinite(o.x_mm) && Number.isFinite(o.y_mm);
    return ok ? [{ name: o.name as string, x_mm: o.x_mm as number, y_mm: o.y_mm as number }] : [];
  });
}

/** Why `list` cannot be saved on the bed of `p` (the engine's rules, checked first for a clear message), or null. */
export function positionsProblem(p: GrblProfileView, list: readonly MovePosition[]): string | null {
  if (list.length > MAX_POSITIONS) return `At most ${MAX_POSITIONS} saved positions.`;
  const [w, h] = p.work_area_mm;
  for (const m of list) {
    const name = m.name.trim();
    if (!name || name.length > MAX_POSITION_NAME) return `A position name must be 1 to ${MAX_POSITION_NAME} characters.`;
    if (!(Number.isFinite(m.x_mm) && Number.isFinite(m.y_mm)) || m.x_mm < 0 || m.x_mm > w || m.y_mm < 0 || m.y_mm > h) {
      return `"${name}" is off the bed (0…${w} × 0…${h} mm).`;
    }
  }
  return null;
}

/** `list` with `m` added, or replacing the position of the same name (names are compared trimmed, case-insensitive). */
export function withPosition(list: readonly MovePosition[], m: MovePosition): MovePosition[] {
  const key = m.name.trim().toLowerCase();
  const round = (v: number) => Math.round(v * 1000) / 1000;
  const next = { name: m.name.trim(), x_mm: round(m.x_mm), y_mm: round(m.y_mm) };
  const i = list.findIndex((q) => q.name.trim().toLowerCase() === key);
  return i < 0 ? [...list, next] : list.map((q, k) => (k === i ? next : q));
}

/** The PUT /api/prefs body that stores `list` as the device's saved positions. */
export function positionsPatch(list: readonly MovePosition[]): { move_positions: MovePosition[] } {
  return { move_positions: list.map((m) => ({ name: m.name.trim(), x_mm: m.x_mm, y_mm: m.y_mm })) };
}

/** A name for a new position: "Position n" with the first n not taken. */
export function nextPositionName(list: readonly MovePosition[], base = "Position"): string {
  const taken = new Set(list.map((m) => m.name.trim().toLowerCase()));
  for (let n = 1; ; n++) {
    const name = `${base} ${n}`;
    if (!taken.has(name.toLowerCase())) return name;
  }
}

// ---- the head's last known position (Show Last Position) --------------------------------------------------------

export interface HeadState {
  /** The head's last known position in the canvas (field) frame, mm, or null when unknown. */
  field: [number, number] | null;
  /** Show Last Position is on: the canvas draws the crosshair. */
  show: boolean;
}

const SHOW_KEY = "openetch.grbl.showLastPosition";

function readShow(): boolean {
  try {
    return globalThis.localStorage?.getItem(SHOW_KEY) === "1";
  } catch {
    return false;
  }
}

let head: HeadState = { field: null, show: readShow() };
const listeners = new Set<() => void>();

function set(next: HeadState) {
  if (next.field === head.field && next.show === head.show) return;
  head = next;
  for (const l of listeners) l();
}

export function headState(): HeadState {
  return head;
}

export function subscribeHead(l: () => void): () => void {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

/** A status report's field position (null: none, e.g. no GRBL device); an unchanged position keeps the same state. */
export function noteHeadPosition(field: [number, number] | null) {
  const same = field && head.field && field[0] === head.field[0] && field[1] === head.field[1];
  if (!same && !(field === null && head.field === null)) set({ ...head, field });
}

/** Turn Show Last Position on or off (remembered in this browser). */
export function setShowLastPosition(show: boolean) {
  try {
    globalThis.localStorage?.setItem(SHOW_KEY, show ? "1" : "0");
  } catch {
    // Private mode or blocked storage: the setting lasts for this page only.
  }
  set({ ...head, show });
}
