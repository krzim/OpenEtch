// The key map: every bindable action with LightBurn's default keys (docs.lightburnsoftware.com "Hotkeys"), the chord
// notation ("Ctrl+Shift+H"), user overrides from Preferences, and conflict checks. Pure; useKeymap.ts applies it to
// the window's keyboard events.
//
// Two kinds of action:
// - "app" actions are implemented by the keyboard handlers in App.tsx and components/EditTools.tsx, which listen for
//   their own `native` chords (keep these lists in step with them). A rebound key (or an extra LightBurn default such
//   as Ctrl+R next to R) is translated into the native chord; a native chord the user moved elsewhere is swallowed when
//   nothing else uses it.
// - "ws" actions are handled by the workspace layer itself (cheat sheet, zoom, preview, framing, draw order, ...).
//
// Browsers keep a few chords for themselves whatever a page does (Ctrl+N, Ctrl+W, Ctrl+T, Ctrl+Tab): LightBurn's
// Weld (Ctrl+W) and Text (Ctrl+T) defaults therefore cannot apply in a browser; `RESERVED` lists them and the editor
// refuses them.

export type HotkeyGroup = "file" | "edit" | "tools" | "arrange" | "view" | "laser" | "help";
export const HOTKEY_GROUPS: HotkeyGroup[] = ["file", "edit", "tools", "arrange", "view", "laser", "help"];

export interface HotkeyAction {
  id: string;
  group: HotkeyGroup;
  /** English label (translated through the "hk.<id>" message when there is one). */
  label: string;
  /** Default chords (LightBurn's where it has one). */
  defaults: string[];
  /** For "app" actions: the chords App.tsx's own handler acts on; the first is what a rebound key is turned into. */
  native?: string[];
  /** Fixed keys are listed in the cheat sheet but cannot be rebound (they carry modifiers of their own). */
  fixed?: boolean;
}

const app = (id: string, group: HotkeyGroup, label: string, native: string[], extra: string[] = []): HotkeyAction => ({
  id,
  group,
  label,
  defaults: [...native, ...extra],
  native,
});
const ws = (id: string, group: HotkeyGroup, label: string, defaults: string[]): HotkeyAction => ({ id, group, label, defaults });
const fixed = (id: string, group: HotkeyGroup, label: string, keys: string[]): HotkeyAction => ({ id, group, label, defaults: keys, fixed: true });

export const ACTIONS: HotkeyAction[] = [
  app("file.new", "file", "New", ["Ctrl+N"]),
  app("file.open", "file", "Open", ["Ctrl+O"]),
  app("file.import", "file", "Import", ["Ctrl+I"]),
  app("file.save", "file", "Save", ["Ctrl+S"]),
  app("file.saveAs", "file", "Save as", ["Ctrl+Shift+S"]),
  app("window.devices", "file", "Devices", ["Alt+Shift+M"]),
  ws("app.preferences", "file", "Preferences", []),
  // Files and image tools (src/files/useFilesUi.tsx): LightBurn's Export (Alt+X), Notes (Ctrl+Alt+N), Trace Image
  // (Alt+T); Adjust Image gets Alt+I here.
  ws("file.export", "file", "Export", ["Alt+X"]),
  ws("file.print", "file", "Print", []),
  ws("window.notes", "view", "Notes window", ["Ctrl+Alt+N"]),
  ws("image.trace", "tools", "Trace image", ["Alt+T"]),
  ws("image.adjust", "tools", "Adjust image", ["Alt+I"]),
  ws("image.toBitmap", "tools", "Convert to bitmap", []),

  app("edit.undo", "edit", "Undo", ["Ctrl+Z"]),
  app("edit.redo", "edit", "Redo", ["Ctrl+Shift+Z", "Ctrl+Y"]),
  app("edit.selectAll", "edit", "Select all", ["Ctrl+A"]),
  app("edit.invert", "edit", "Invert selection", ["Ctrl+Shift+I"]),
  app("edit.cut", "edit", "Cut", ["Ctrl+X"]),
  app("edit.copy", "edit", "Copy", ["Ctrl+C"]),
  app("edit.paste", "edit", "Paste", ["Ctrl+V"]),
  app("edit.pasteInPlace", "edit", "Paste in place", ["Alt+V"]),
  app("edit.duplicate", "edit", "Duplicate", ["Ctrl+D"]),
  app("edit.delete", "edit", "Delete", ["Del", "Backspace"]),
  app("edit.convertToPath", "edit", "Convert to path", ["Ctrl+Shift+C"]),
  fixed("edit.cycle", "edit", "Select next / previous object", ["Tab", "Shift+Tab"]),
  fixed("edit.escape", "edit", "Select tool, then clear selection", ["Esc"]),

  app("tool.rect", "tools", "Rectangle", ["R"], ["Ctrl+R"]),
  app("tool.ellipse", "tools", "Ellipse", ["E"], ["Ctrl+E"]),
  app("tool.circle", "tools", "Circle from centre", ["C"]),
  // LightBurn's Text is Ctrl+T, which browser tabs keep (an installed app window may pass it); T is the web default.
  // With two or more shapes selected T aligns their tops instead (the handler declines).
  ws("tool.text", "tools", "Text", ["T", "Ctrl+T"]),
  ws("tool.barcode", "tools", "Barcode", []),
  ws("window.variableText", "view", "Variable Text window", []),
  app("tool.line", "tools", "Line / pen", ["Ctrl+L"]),
  app("tool.nodes", "tools", "Edit nodes", ["Ctrl+`"]),
  app("tool.weld", "tools", "Weld", ["Ctrl+W"]),
  app("tool.union", "tools", "Boolean union", ["Alt+Shift++", "Alt++"]),
  app("tool.subtract", "tools", "Boolean subtract", ["Alt+-"]),
  app("tool.intersect", "tools", "Boolean intersection", ["Alt+Shift+*", "Alt+*"]),
  app("tool.cutShapes", "tools", "Cut shapes", ["Alt+C"]),
  app("tool.offset", "tools", "Offset shapes", ["Alt+O"]),
  app("tool.breakApart", "tools", "Break apart", ["Alt+B"]),
  app("tool.autoJoin", "tools", "Auto-join", ["Alt+J"]),
  app("tool.deleteDuplicates", "tools", "Delete duplicates", ["Alt+D"]),
  app("tool.optimize", "tools", "Optimize shapes", ["Alt+Shift+O"]),
  app("tool.rotate2", "tools", "Two-point rotate", ["Ctrl+2"]),

  app("arrange.group", "arrange", "Group", ["Ctrl+G"]),
  app("arrange.ungroup", "arrange", "Ungroup", ["Ctrl+U"]),
  app("arrange.flipH", "arrange", "Flip horizontal", ["Ctrl+Shift+H"]),
  app("arrange.flipV", "arrange", "Flip vertical", ["Ctrl+Shift+V"]),
  app("arrange.alignLeft", "arrange", "Align left", ["Alt+Left"]),
  app("arrange.alignRight", "arrange", "Align right", ["Alt+Right"]),
  app("arrange.alignTop", "arrange", "Align top", ["Alt+Up"]),
  app("arrange.alignBottom", "arrange", "Align bottom", ["Alt+Down"]),
  app("arrange.alignHCenter", "arrange", "Align centres horizontally", ["Alt+PgUp"]),
  app("arrange.alignVCenter", "arrange", "Align centres vertically", ["Alt+PgDn"]),
  app("arrange.centerField", "arrange", "Move selection to field centre", ["P"]),
  app("arrange.forward", "arrange", "Push forward in draw order", ["PgUp"]),
  app("arrange.backward", "arrange", "Push backward in draw order", ["PgDn"]),
  app("arrange.front", "arrange", "Bring to front", ["Ctrl+PgUp"]),
  app("arrange.back", "arrange", "Send to back", ["Ctrl+PgDn"]),
  fixed("arrange.canvasAlign", "arrange", "Align left / right / top / bottom / h-centres / v-centres (2+ selected, canvas)", [
    "L",
    "R",
    "T",
    "B",
    "C",
    "E",
  ]),
  fixed("arrange.nudge", "arrange", "Nudge (Shift large, Ctrl small, Ctrl+Shift tiny)", ["Left", "Right", "Up", "Down"]),
  fixed("arrange.rotate", "arrange", "Rotate 90° clockwise / counter-clockwise (Shift 45°, Ctrl 15°)", [".", ","]),

  app("view.wireframe", "view", "Toggle wireframe / filled", ["Alt+Shift+W"]),
  ws("view.preview", "view", "Preview (Laser workspace)", ["Alt+P"]),
  ws("view.zoomIn", "view", "Zoom in", ["Ctrl+=", "Ctrl+Shift++"]),
  ws("view.zoomOut", "view", "Zoom out", ["Ctrl+-"]),
  ws("view.zoomPage", "view", "Zoom to field", ["Ctrl+0"]),
  ws("view.zoomSelection", "view", "Zoom to selection", []),

  ws("laser.frameSelection", "laser", "Frame selection", ["Ctrl+Shift+A"]),
  ws("laser.frameSquare", "laser", "Frame (bounding box)", ["Alt+R"]),
  ws("laser.frameOutline", "laser", "Frame outline (rubber band)", ["Alt+Shift+R"]),
  ws("laser.frameStop", "laser", "Stop framing", []),
  ws("laser.requestRun", "laser", "Request run", []),
  ws("laser.pauseResume", "laser", "Pause / resume the job", []),
  ws("laser.stop", "laser", "Stop", ["Ctrl+Pause", "Ctrl+Cancel"]),
  // Library, test generators and laser tools (src/libs/useLibsUi.tsx); LightBurn gives these no default keys.
  ws("window.library", "view", "Material Library", []),
  ws("laser.materialTest", "laser", "Material test", []),
  ws("laser.intervalTest", "laser", "Interval test", []),
  ws("laser.focusTest", "laser", "Focus test", []),
  ws("tool.startPoint", "tools", "Set start point", []),
  ws("laser.printCut", "laser", "Print and Cut", []),
  ws("window.camera", "view", "Camera", []),
  ws("laser.jobLog", "laser", "Job log", []),

  ws("help.hotkeys", "help", "Keyboard shortcuts", ["F1", "Shift+?"]),
];

export const ACTION_BY_ID: ReadonlyMap<string, HotkeyAction> = new Map(ACTIONS.map((a) => [a.id, a]));

/** Chords a browser keeps for itself (the page never sees them, or cannot stop them). */
export const RESERVED = new Set(["Ctrl+W", "Ctrl+T", "Ctrl+Tab", "Ctrl+Shift+Tab", "Ctrl+Shift+N", "Ctrl+Shift+T", "Ctrl+Shift+W", "Alt+F4", "F11", "F12"]);

// --- chords ----------------------------------------------------------------------------------------------------------

const KEY_NAMES: Record<string, string> = {
  ArrowLeft: "Left",
  ArrowRight: "Right",
  ArrowUp: "Up",
  ArrowDown: "Down",
  PageUp: "PgUp",
  PageDown: "PgDn",
  Delete: "Del",
  Escape: "Esc",
  " ": "Space",
  Spacebar: "Space",
};
const KEY_VALUES: Record<string, string> = Object.fromEntries(Object.entries(KEY_NAMES).map(([k, v]) => [v, k]));
KEY_VALUES.Space = " ";

const MODIFIER_KEYS = new Set(["Control", "Shift", "Alt", "Meta", "AltGraph", "CapsLock", "OS", "Dead", "Unidentified", "Process"]);

export interface KeyLike {
  key: string;
  code?: string;
  ctrlKey: boolean;
  metaKey?: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/** The chord of a key event ("Ctrl+Shift+H"), or null for a lone modifier. Cmd counts as Ctrl. */
export function chordOf(e: KeyLike): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  let key = KEY_NAMES[e.key] ?? e.key;
  // Alt (Option on a Mac) can turn a letter into another character: use the physical key then.
  if (e.altKey && e.code && !/^[\x21-\x7e]$/.test(key)) {
    const m = /^(?:Key([A-Z])|Digit(\d))$/.exec(e.code);
    if (m) key = m[1] ?? m[2];
  }
  if (key.length === 1) key = key.toUpperCase();
  return joinChord({ ctrl: e.ctrlKey || !!e.metaKey, alt: e.altKey, shift: e.shiftKey }, key);
}

function joinChord(m: { ctrl: boolean; alt: boolean; shift: boolean }, key: string): string {
  return [m.ctrl ? "Ctrl" : "", m.alt ? "Alt" : "", m.shift ? "Shift" : "", key].filter(Boolean).join("+");
}

/** Split a chord into modifiers and key ("Ctrl+Shift++" has the key "+"), or null when it is not valid. */
export function parseChord(s: string): { ctrl: boolean; alt: boolean; shift: boolean; key: string } | null {
  const t = s.trim();
  if (!t) return null;
  // The key is the part after the last "+" that separates; a trailing "++" means the key is "+".
  const plusKey = t.endsWith("++") || t === "+";
  const head = plusKey ? t.slice(0, t === "+" ? 0 : -2) : t.slice(0, Math.max(0, t.lastIndexOf("+")));
  const key = plusKey ? "+" : t.slice(t.lastIndexOf("+") + 1);
  if (!key) return null;
  const mods = head ? head.split("+") : [];
  const m = { ctrl: false, alt: false, shift: false };
  for (const x of mods) {
    const k = x.toLowerCase();
    if (k === "ctrl" || k === "cmd" || k === "control") m.ctrl = true;
    else if (k === "alt" || k === "option") m.alt = true;
    else if (k === "shift") m.shift = true;
    else return null;
  }
  const name = key.length === 1 ? key.toUpperCase() : key;
  return { ...m, key: name };
}

/** A chord in canonical form ("shift+ctrl+h" -> "Ctrl+Shift+H"), or null. */
export function normalizeChord(s: string): string | null {
  const p = parseChord(s);
  return p ? joinChord(p, p.key) : null;
}

/** Physical key (`KeyboardEvent.code`, US layout) of a key name, for handlers that test `code`. */
const CODES: Record<string, string> = {
  "-": "Minus",
  "=": "Equal",
  "+": "Equal",
  "*": "Digit8",
  "`": "Backquote",
  ",": "Comma",
  "<": "Comma",
  ".": "Period",
  ">": "Period",
  "/": "Slash",
  "?": "Slash",
  " ": "Space",
};

function codeOf(key: string): string {
  if (/^[A-Za-z]$/.test(key)) return `Key${key.toUpperCase()}`;
  if (/^\d$/.test(key)) return `Digit${key}`;
  return CODES[key] ?? key;
}

/** A keyboard event init for chord `s` (what App.tsx's and EditTools' handlers would see for it). */
export function eventInitOf(s: string): { key: string; code: string; ctrlKey: boolean; altKey: boolean; shiftKey: boolean } | null {
  const p = parseChord(s);
  if (!p) return null;
  let key = KEY_VALUES[p.key] ?? p.key;
  if (key.length === 1 && /[A-Z]/.test(key) && !p.shift) key = key.toLowerCase();
  return { key, code: codeOf(key), ctrlKey: p.ctrl, altKey: p.alt, shiftKey: p.shift };
}

/** A chord without modifiers other than Shift, which the app only acts on while the canvas has focus. */
export function isBareKey(s: string): boolean {
  const p = parseChord(s);
  return !!p && !p.ctrl && !p.alt;
}

// --- bindings --------------------------------------------------------------------------------------------------------

/** The chords bound to `a` with overrides applied. */
export function chordsFor(a: HotkeyAction, overrides: Readonly<Record<string, string[]>>): string[] {
  if (a.fixed) return a.defaults;
  const o = overrides[a.id];
  return (o ?? a.defaults).map((c) => normalizeChord(c)).filter((c): c is string => !!c);
}

/** chord -> action ids bound to it, in catalogue order. */
export function bindingTable(overrides: Readonly<Record<string, string[]>>, actions: readonly HotkeyAction[] = ACTIONS): Map<string, string[]> {
  const t = new Map<string, string[]>();
  for (const a of actions) {
    if (a.fixed) continue;
    for (const c of chordsFor(a, overrides)) t.set(c, [...(t.get(c) ?? []), a.id]);
  }
  return t;
}

/**
 * What to do with a key press `chord` under `overrides`:
 * - `ws`: run the workspace action (and stop the event);
 * - `translate`: stop the event and send `to` (an app action's native chord) instead;
 * - `block`: a native chord whose action was moved to another key; nothing uses it now, so swallow it;
 * - `pass`: leave the event alone (App.tsx's handler or the browser deals with it).
 */
export type KeyResolution =
  | { kind: "ws"; action: string }
  | { kind: "translate"; action: string; to: string }
  | { kind: "block" }
  | { kind: "pass" };

export function resolveKey(chord: string, overrides: Readonly<Record<string, string[]>>, actions: readonly HotkeyAction[] = ACTIONS): KeyResolution {
  const table = bindingTable(overrides, actions);
  const ids = table.get(chord) ?? [];
  for (const id of ids) {
    const a = actions.find((x) => x.id === id);
    if (!a) continue;
    if (!a.native) return { kind: "ws", action: id };
    if (a.native.includes(chord)) return { kind: "pass" };
    return { kind: "translate", action: id, to: a.native[0] };
  }
  // Unbound now, but App.tsx would still act on it: swallow when its action was rebound elsewhere.
  // A key that also has a fixed meaning (R aligns right with two objects selected) is never swallowed.
  const owner = actions.find((a) => a.native?.includes(chord));
  const fixedUse = actions.some((a) => a.fixed && a.defaults.includes(chord));
  if (owner && !fixedUse && !chordsFor(owner, overrides).includes(chord)) return { kind: "block" };
  return { kind: "pass" };
}

/** Other actions already using `chord` (for the editor's conflict warning). */
export function conflictsOf(chord: string, actionId: string, overrides: Readonly<Record<string, string[]>>, actions: readonly HotkeyAction[] = ACTIONS): string[] {
  const c = normalizeChord(chord);
  if (!c) return [];
  const fixedHits = actions.filter((a) => a.fixed && a.defaults.includes(c)).map((a) => a.id);
  return [...(bindingTable(overrides, actions).get(c) ?? []).filter((id) => id !== actionId), ...fixedHits];
}

/**
 * New overrides with `chord` bound to `actionId` (added to its chords, or replacing chord index `replace`), taking it
 * away from any action that had it. Returns null for an invalid or reserved chord.
 */
export function bindChord(
  overrides: Readonly<Record<string, string[]>>,
  actionId: string,
  chord: string,
  replace?: number,
): Record<string, string[]> | null {
  const c = normalizeChord(chord);
  const a = ACTION_BY_ID.get(actionId);
  if (!c || !a || a.fixed || RESERVED.has(c)) return null;
  const next: Record<string, string[]> = { ...overrides };
  for (const other of ACTIONS) {
    if (other.id === actionId || other.fixed) continue;
    const cur = chordsFor(other, next);
    if (cur.includes(c)) next[other.id] = cur.filter((x) => x !== c);
  }
  const mine = chordsFor(a, next).filter((x) => x !== c);
  if (replace !== undefined && replace >= 0 && replace < mine.length + 1) mine.splice(replace, 1, c);
  else mine.push(c);
  next[actionId] = [...new Set(mine)].slice(0, 4);
  return simplify(next);
}

/** New overrides without chord `chord` on `actionId`. */
export function unbindChord(overrides: Readonly<Record<string, string[]>>, actionId: string, chord: string): Record<string, string[]> {
  const a = ACTION_BY_ID.get(actionId);
  if (!a) return { ...overrides };
  return simplify({ ...overrides, [actionId]: chordsFor(a, overrides).filter((c) => c !== chord) });
}

/** New overrides with `actionId` back on its defaults. */
export function resetAction(overrides: Readonly<Record<string, string[]>>, actionId: string): Record<string, string[]> {
  const next = { ...overrides };
  delete next[actionId];
  return next;
}

/** Drop overrides equal to the defaults. */
function simplify(o: Record<string, string[]>): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const [id, chords] of Object.entries(o)) {
    const a = ACTION_BY_ID.get(id);
    if (!a) continue;
    const same = chords.length === a.defaults.length && chords.every((c, i) => c === normalizeChord(a.defaults[i]));
    if (!same) out[id] = chords;
  }
  return out;
}

/** The first chord of an action, for menu hints ("" when unbound). */
export function hintFor(actionId: string, overrides: Readonly<Record<string, string[]>>): string {
  const a = ACTION_BY_ID.get(actionId);
  return a ? (chordsFor(a, overrides)[0] ?? "") : "";
}
