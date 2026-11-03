// Command search (Fusion's S; docs/ux/ui-plan-2026-11.md P2): every hotkeys.ts action plus the menu items, ranked
// against what was typed, with aliases ("settings" finds Preferences) and the recent picks first. Each entry keeps
// its action id (the same id the MCP tools and the hotkey editor use). Pure; tested in commandSearch.test.ts.

export interface Command {
  /** Unique key: the hotkeys.ts action id, or "menu:<path>" for a menu item without one. */
  key: string;
  label: string;
  /** Where it lives ("File › Export", "Arrange"). */
  path?: string;
  /** The hotkeys.ts action id it runs, if any (shown, and kept for MCP parity). */
  actionId?: string;
  hint?: string;
  aliases?: readonly string[];
  disabled?: boolean;
  run: () => void;
}

/** Other words people search for (English; a translated label is searched as well). */
export const ALIASES: Readonly<Record<string, readonly string[]>> = {
  "app.preferences": ["settings", "options", "config"],
  "help.hotkeys": ["shortcuts", "keys", "keyboard", "cheat sheet"],
  // "guide" alone finds the user guide first; the ruler guides ("Add guide…", "Clear guides") come after it.
  "help.guide": ["guide", "help", "manual", "documentation", "docs", "how to", "instructions"],
  "edit.delete": ["remove", "erase"],
  "edit.duplicate": ["clone", "copy"],
  "edit.selectAll": ["everything"],
  "tool.rect": ["rectangle", "box", "square"],
  "tool.ellipse": ["oval", "circle"],
  "tool.circle": ["round"],
  "tool.text": ["font", "type", "lettering"],
  "tool.line": ["polyline", "pen", "path", "draw"],
  "tool.nodes": ["vertices", "points", "bezier"],
  "tool.weld": ["merge", "union", "combine"],
  "tool.union": ["merge", "add"],
  "tool.subtract": ["difference", "minus", "cut out"],
  "tool.trim": ["cut lines", "scissors"],
  "tool.fillRegion": ["paint bucket", "bucket", "flood fill", "fill area", "enclosed area"],
  "tool.offset": ["outline", "inset", "grow", "shrink"],
  "tool.measure": ["ruler", "distance", "dimension"],
  "arrange.group": ["combine"],
  "arrange.flipH": ["mirror"],
  "arrange.flipV": ["mirror"],
  "arrange.centerField": ["middle", "center"],
  "view.zoomPage": ["fit", "zoom to fit", "home"],
  "view.fullscreen": ["kiosk", "maximise", "maximize"],
  "view.focusCanvas": ["distraction free", "hide panels", "zen"],
  "view.preview": ["simulate", "toolpath"],
  "laser.frameSquare": ["trace", "bounds"],
  "laser.frameSelection": ["trace"],
  "laser.stop": ["abort", "halt", "emergency", "cancel"],
  "laser.position": ["red dot", "pointer", "move laser"],
  "file.export": ["svg", "png", "dxf", "save as"],
  "file.import": ["open svg", "add file", "place file"],
  "window.library": ["materials", "presets"],
  "help.useAsApp": ["install", "home screen", "dock", "pwa"],
  "window.sidePanels": ["hide panels", "docks"],
};

/** Lower-case, accents removed, punctuation as spaces. */
export function normalize(s: string): string {
  return s
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9.]+/g, " ")
    .trim();
}

function words(s: string): string[] {
  return normalize(s).split(" ").filter(Boolean);
}

/** Whether the letters of `q` appear in order in `s` (fuzzy). */
function subsequence(q: string, s: string): boolean {
  let i = 0;
  for (const c of s) if (c === q[i]) i++;
  return i === q.length;
}

/** How well one query token matches a command (0 = not at all). */
function tokenScore(tok: string, c: Command): number {
  const label = normalize(c.label);
  const lw = words(c.label);
  let best = 0;
  if (label === tok) best = 1000;
  else if (label.startsWith(tok)) best = 800;
  else if (lw.some((w) => w.startsWith(tok))) best = 600;
  else if (tok.length >= 2 && lw.map((w) => w[0]).join("").startsWith(tok)) best = 450;
  else if (label.includes(tok)) best = 300;
  for (const a of c.aliases ?? []) {
    const n = normalize(a);
    if (n === tok || n.startsWith(tok)) best = Math.max(best, 520);
    else if (n.split(" ").some((w) => w.startsWith(tok))) best = Math.max(best, 400);
    else if (n.includes(tok)) best = Math.max(best, 250);
  }
  const id = (c.actionId ?? "").toLowerCase();
  if (id && (id === tok || id.split(".").some((p) => p.startsWith(tok)))) best = Math.max(best, 350);
  else if (id.includes(tok)) best = Math.max(best, 200);
  if (c.path && normalize(c.path).split(" ").some((w) => w.startsWith(tok))) best = Math.max(best, 150);
  if (!best && tok.length >= 3 && subsequence(tok, label.replace(/ /g, ""))) best = 80;
  return best;
}

/** The score of command `c` for `query` (0 = no match). Every word typed must match. */
export function scoreCommand(c: Command, query: string): number {
  const toks = normalize(query).split(" ").filter(Boolean);
  if (!toks.length) return 1;
  let sum = 0;
  for (const t of toks) {
    const s = tokenScore(t, c);
    if (!s) return 0;
    sum += s;
  }
  // The whole query is one of the command's aliases: as good as typing its label.
  const whole = toks.join(" ");
  if ((c.aliases ?? []).some((a) => normalize(a) === whole)) sum = Math.max(sum, 1000 * toks.length);
  return sum;
}

/**
 * The commands to list for `query`: with nothing typed, the recent ones (most recent first) and then the rest A-Z;
 * otherwise the matches best first, recent picks lifted a little, disabled ones after enabled ones of equal score.
 */
export function searchCommands(cmds: readonly Command[], query: string, recents: readonly string[] = [], limit = 60): Command[] {
  const recentRank = new Map(recents.map((k, i) => [k, i]));
  const byLabel = (a: Command, b: Command) => a.label.localeCompare(b.label);
  if (!normalize(query)) {
    const recent = recents.map((k) => cmds.find((c) => c.key === k)).filter((c): c is Command => !!c);
    const rest = cmds.filter((c) => !recentRank.has(c.key)).sort(byLabel);
    return [...recent, ...rest].slice(0, limit);
  }
  return cmds
    .map((c) => {
      const s = scoreCommand(c, query);
      const r = recentRank.get(c.key);
      return { c, s: s ? s + (r === undefined ? 0 : 60 - Math.min(50, r * 5)) - (c.disabled ? 30 : 0) : 0 };
    })
    .filter((x) => x.s > 0)
    .sort((a, b) => b.s - a.s || byLabel(a.c, b.c))
    .slice(0, limit)
    .map((x) => x.c);
}

/** `recents` with `key` moved to the front, at most `max` long. */
export function pushRecent(recents: readonly string[], key: string, max = 8): string[] {
  return [key, ...recents.filter((k) => k !== key)].slice(0, max);
}

/**
 * Commands without duplicates: a menu item that runs an action takes that action's entry over (its own run and its
 * place in the menus); two menu items with one action or one key are listed once.
 */
export function mergeCommands(actions: readonly Command[], menu: readonly Command[]): Command[] {
  const fromMenu = new Map<string, Command>();
  for (const m of menu) if (m.actionId && !fromMenu.has(m.actionId)) fromMenu.set(m.actionId, m);
  const out: Command[] = actions.map((a) => {
    const m = a.actionId ? fromMenu.get(a.actionId) : undefined;
    return m && !m.disabled ? { ...a, run: m.run, path: m.path ?? a.path } : a;
  });
  const taken = new Set<string>([...actions.map((a) => a.key), ...actions.map((a) => a.actionId ?? "")]);
  for (const m of menu) {
    if (taken.has(m.key) || (m.actionId && taken.has(m.actionId))) continue;
    taken.add(m.key);
    if (m.actionId) taken.add(m.actionId);
    out.push(m);
  }
  return out;
}
