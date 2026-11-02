// A GRBL gantry's Move window, Console window and Save G-code (LightBurn's Move and Console windows, File > Save
// GCode): the engine's GET /api/machine/grbl, POST /api/machine/jog, /home, /unlock, /console and GET
// /api/export/gcode, read and built here. None of this can fire the laser: jogs, homing and unlocking never turn it on,
// and the engine refuses console lines (macros included) that could (M3/M4, S > 0, realtime characters, start-up
// blocks that do either). Shown only while the active device is a GRBL gantry. Pure, so `npm test` covers it.

import type { MachineStatus } from "./types.ts";

/** The device's origin corner (a GRBL gantry never has the centre). */
export type GantryOrigin = "bottom_left" | "top_left" | "bottom_right" | "top_right";

/** The parts of `machine::grbl::GrblProfile` the windows use. */
export interface GrblProfileView {
  work_area_mm: [number, number];
  origin: GantryOrigin;
  homing: boolean;
  frame_feed_mm_min: number;
  max_feed_mm_min: number;
  dynamic_power: boolean;
}

/** GET /api/machine/grbl, read defensively. */
export interface GrblStatusView {
  profile: GrblProfileView;
  /** The board's state from its last status report ("Idle", "Run", "Jog", "Alarm", ...), or null. */
  state: string | null;
  /** Head position from the last status report: machine mm from the origin corner, and the canvas (field) frame. */
  machine: [number, number] | null;
  field: [number, number] | null;
  alarm: number | null;
  version: string | null;
  /** Start-up blocks that could turn the laser on (motion is refused while there are any). */
  unsafeStartup: string[];
  running: boolean;
  framing: boolean;
}

/** The active machine is a GRBL gantry (GET /api/machine `driver`): the Move and Console windows, Save G-code and the
 * GRBL layer options are offered only then. */
export function isGrblMachine(m: MachineStatus | null | undefined): boolean {
  return m?.driver === "grbl";
}

function obj(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

function pair(v: unknown): [number, number] | null {
  if (!Array.isArray(v) || v.length < 2) return null;
  const a = num(v[0]);
  const b = num(v[1]);
  return a === null || b === null ? null : [a, b];
}

const ORIGINS: readonly GantryOrigin[] = ["bottom_left", "top_left", "bottom_right", "top_right"];

/** The profile of GET /api/machine/grbl (or GET /api/machine `grbl.profile`), or null when it is not usable. */
export function parseGrblProfile(raw: unknown): GrblProfileView | null {
  const p = obj(raw);
  const area = pair(p?.work_area_mm);
  if (!p || !area || !(area[0] > 0 && area[1] > 0)) return null;
  const origin = ORIGINS.includes(p.origin as GantryOrigin) ? (p.origin as GantryOrigin) : "bottom_left";
  const max = num(p.max_feed_mm_min) ?? 6000;
  return {
    work_area_mm: area,
    origin,
    homing: p.homing === true,
    frame_feed_mm_min: Math.min(num(p.frame_feed_mm_min) ?? 3000, max),
    max_feed_mm_min: max,
    dynamic_power: p.dynamic_power !== false,
  };
}

/** For the GRBL layer options: null unless the active machine is a GRBL gantry, else its default power mode. */
export function gantryOf(m: MachineStatus | null | undefined): { dynamic: boolean } | null {
  if (!isGrblMachine(m)) return null;
  return { dynamic: parseGrblProfile(obj(m?.grbl)?.profile)?.dynamic_power ?? true };
}

export function parseGrblStatus(raw: unknown): GrblStatusView | null {
  const v = obj(raw);
  const profile = parseGrblProfile(v?.profile);
  if (!v || !profile) return null;
  const board = obj(v.board);
  const status = obj(board?.status);
  const pos = obj(v.position);
  return {
    profile,
    state: typeof status?.state === "string" ? status.state : null,
    machine: pair(pos?.machine),
    field: pair(pos?.field),
    alarm: num(board?.alarm),
    version: typeof board?.version === "string" ? board.version : null,
    unsafeStartup: Array.isArray(board?.unsafe_startup) ? board.unsafe_startup.filter((x): x is string => typeof x === "string") : [],
    running: v.running === true,
    framing: v.framing === true,
  };
}

/** Which axes point from the right or rear edge (the engine's `GrblProfile::mirror`). */
function mirror(o: GantryOrigin): [boolean, boolean] {
  return [o === "bottom_right" || o === "top_right", o === "top_left" || o === "top_right"];
}

/** Machine mm (from the origin corner, positive into the bed) to the canvas frame (bed centre, +Y to the rear). */
export function machineToField(p: GrblProfileView, m: [number, number]): [number, number] {
  const [w, h] = p.work_area_mm;
  const [mx, my] = mirror(p.origin);
  return [(mx ? -1 : 1) * (m[0] - w / 2), (my ? -1 : 1) * (m[1] - h / 2)];
}

/** The canvas frame to machine mm (the inverse of `machineToField`). */
export function fieldToMachine(p: GrblProfileView, f: [number, number]): [number, number] {
  const [w, h] = p.work_area_mm;
  const [mx, my] = mirror(p.origin);
  return [(mx ? -f[0] : f[0]) + w / 2, (my ? -f[1] : f[1]) + h / 2];
}

/** Why the machine position `m` cannot be jogged to (off the bed), or null. */
export function positionProblem(p: GrblProfileView, m: [number, number]): string | null {
  const [w, h] = p.work_area_mm;
  if (!m.every(Number.isFinite)) return "Enter both X and Y.";
  if (m[0] < 0 || m[0] > w || m[1] < 0 || m[1] > h) return `The bed is 0…${w} × 0…${h} mm from the origin corner.`;
  return null;
}

// ---- jogging --------------------------------------------------------------------------------------------------------

/** A jog direction on screen: up is to the rear of the bed (+Y on the canvas), right is +X. */
export type JogDir = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

/** LightBurn-like distance presets, mm. */
export const JOG_STEPS = [0.1, 1, 5, 10, 50, 100];

const DIRS: Record<JogDir, [number, number]> = {
  n: [0, 1],
  s: [0, -1],
  e: [1, 0],
  w: [-1, 0],
  ne: [1, 1],
  nw: [-1, 1],
  se: [1, -1],
  sw: [-1, -1],
};

/** The `{dx, dy}` of POST /api/machine/jog (canvas directions; the engine mirrors them for the origin corner). */
export function jogDelta(dir: JogDir, step: number): { dx: number; dy: number } {
  const [x, y] = DIRS[dir];
  // `+ 0` turns -0 into 0, so the body never carries "-0".
  return { dx: x * step + 0, dy: y * step + 0 };
}

/**
 * The jog a key asks for while the Move window has the focus (arrow keys; Shift ×10, Alt ÷10), or null. One jog per key
 * press: a held key's auto-repeat is ignored, since each repeat would queue another step in GRBL's planner and the head
 * would keep going after the key is released.
 */
export function jogKey(
  e: { key: string; shiftKey?: boolean; altKey?: boolean; ctrlKey?: boolean; metaKey?: boolean; repeat?: boolean },
  step: number,
): { dir: JogDir; step: number } | null {
  if (e.ctrlKey || e.metaKey || e.repeat) return null;
  const dir = ({ ArrowUp: "n", ArrowDown: "s", ArrowLeft: "w", ArrowRight: "e" } as Record<string, JogDir>)[e.key];
  if (!dir) return null;
  return { dir, step: e.shiftKey ? step * 10 : e.altKey ? step / 10 : step };
}

/**
 * Whether the Move window keeps a key to itself (never passed on to the canvas, where arrows nudge the selection):
 * the arrow keys it jogs with, also while a jog is in flight or the key auto-repeats. Escape stops a jog and still
 * goes on to the app (which, among other things, turns the position laser off).
 */
export function moveWindowOwnsKey(e: { key: string; ctrlKey?: boolean; metaKey?: boolean }): boolean {
  return !e.ctrlKey && !e.metaKey && ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(e.key);
}

/** Why a jog step or feed cannot be used, or null. */
export function jogProblem(p: GrblProfileView, step: number, feed: number): string | null {
  const [w, h] = p.work_area_mm;
  if (!(Number.isFinite(step) && step > 0 && step <= Math.max(w, h))) return `The distance must be more than 0 and at most ${Math.max(w, h)} mm.`;
  if (!(Number.isFinite(feed) && feed > 0 && feed <= p.max_feed_mm_min)) return `The speed must be more than 0 and at most ${p.max_feed_mm_min} mm/min.`;
  return null;
}

// ---- console --------------------------------------------------------------------------------------------------------

/** One console log line: what was sent, what the board answered, refusals and notes. */
export interface ConsoleEntry {
  kind: "sent" | "reply" | "ok" | "error" | "info";
  text: string;
}

/** Most log lines kept, and most history lines. */
export const MAX_LOG = 500;
export const MAX_HISTORY = 100;

export function appendLog(log: readonly ConsoleEntry[], more: readonly ConsoleEntry[], max = MAX_LOG): ConsoleEntry[] {
  const next = [...log, ...more];
  return next.length > max ? next.slice(next.length - max) : next;
}

/** POST /api/machine/console's answer `{line, lines, ok, error}` as log lines. */
export function replyEntries(raw: unknown): ConsoleEntry[] {
  const r = obj(raw) ?? {};
  const lines = Array.isArray(r.lines) ? r.lines.filter((x): x is string => typeof x === "string") : [];
  const out: ConsoleEntry[] = lines.map((text) => ({ kind: "reply", text }));
  if (r.ok === true) out.push({ kind: "ok", text: "ok" });
  else out.push({ kind: "error", text: typeof r.error === "string" ? r.error : "error" });
  return out;
}

/** The history with `line` added last (a repeat of the last line is not added twice). */
export function historyPush(hist: readonly string[], line: string, max = MAX_HISTORY): string[] {
  const t = line.trim();
  if (!t || hist[hist.length - 1] === t) return [...hist];
  const next = [...hist, t];
  return next.length > max ? next.slice(next.length - max) : next;
}

/**
 * Up / Down through the history: `index` is the position shown (`hist.length` = the empty new line). Returns the new
 * index and the text to show.
 */
export function historyStep(hist: readonly string[], index: number, dir: -1 | 1): { index: number; text: string } {
  const i = Math.max(0, Math.min(hist.length, index + dir));
  return { index: i, text: i === hist.length ? "" : hist[i] };
}

// ---- macros ---------------------------------------------------------------------------------------------------------

/** A console macro as the engine stores it with the device (`console_macros` of GET /api/prefs `device_prefs`). */
export interface Macro {
  name: string;
  lines: string[];
}

export const MAX_MACROS = 32;
export const MAX_MACRO_NAME = 40;
export const MAX_MACRO_LINES = 50;
export const MAX_CONSOLE_LINE = 120;

/** The device's macros from GET /api/prefs. */
export function parseMacros(prefs: unknown): Macro[] {
  const d = obj(obj(prefs)?.device_prefs);
  const list = Array.isArray(d?.console_macros) ? d.console_macros : [];
  return list.flatMap((m) => {
    const o = obj(m);
    if (!o || typeof o.name !== "string" || !Array.isArray(o.lines)) return [];
    return [{ name: o.name, lines: o.lines.filter((x): x is string => typeof x === "string") }];
  });
}

/** A macro's text (one console line per line) as its lines: trimmed, blank and comment-only lines dropped. */
export function macroLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l && !/^;/.test(l) && !/^\([^()]*\)$/.test(l));
}

/**
 * Whether a line looks like it turns the laser on (M3 / M4, or S above 0), so the editor can say the engine will refuse
 * it. Only a hint: the engine's console policy decides, for macros as for typed lines.
 */
export function looksLaserOn(line: string): boolean {
  const s = line.replace(/\([^)]*\)/g, "").replace(/;.*$/, "").replace(/\s+/g, "").toUpperCase();
  if (/M\+?0*[34](\.0*)?(?![0-9.])/.test(s)) return true;
  const m = s.match(/S\+?([0-9]*\.?[0-9]+(?:E[+-]?[0-9]+)?)/g) ?? [];
  return m.some((w) => Number(w.slice(1).replace(/^\+/, "")) > 0);
}

/** Why the macros cannot be saved (the engine's rules, checked first for a clear message), or null. */
export function macrosProblem(list: readonly Macro[]): string | null {
  if (list.length > MAX_MACROS) return `At most ${MAX_MACROS} macros.`;
  for (const m of list) {
    const name = m.name.trim();
    if (!name || name.length > MAX_MACRO_NAME) return `A macro name must be 1 to ${MAX_MACRO_NAME} characters.`;
    if (m.lines.length === 0) return `Macro "${name}" has no lines.`;
    if (m.lines.length > MAX_MACRO_LINES) return `Macro "${name}" has more than ${MAX_MACRO_LINES} lines.`;
    for (const l of m.lines) {
      if (l.length > MAX_CONSOLE_LINE) return `Macro "${name}": a line is longer than ${MAX_CONSOLE_LINE} characters.`;
      if (/[?!~]/.test(l)) return `Macro "${name}": ? ! ~ are realtime commands and cannot be sent as lines.`;
      if (looksLaserOn(l)) return `Macro "${name}": "${l}" would turn the laser on; only an approved run can.`;
    }
  }
  return null;
}

/** The PUT /api/prefs body that stores `list` as the device's macros. */
export function macrosPatch(list: readonly Macro[]): { console_macros: Macro[] } {
  return { console_macros: list.map((m) => ({ name: m.name.trim(), lines: [...m.lines] })) };
}

// ---- Save G-code ----------------------------------------------------------------------------------------------------

/** The file name of a saved G-code job: the engine's name, else the document's, else "job". */
export function gcodeFileName(fromEngine: string | null, docName: string | null): string {
  if (fromEngine && /\.(gcode|nc|gc)$/i.test(fromEngine)) return fromEngine;
  const base = (docName ?? "").replace(/\.[A-Za-z0-9]{1,8}$/, "").replace(/[\\/:*?"<>|]+/g, "_").trim();
  return `${base || "job"}.gcode`;
}
