// Folding a long context menu (the canvas's, about 40 entries) into at most about a dozen rows plus Arrange ▸,
// Modify ▸, Select ▸ and Laser ▸ submenus (docs/ux/ui-plan-2026-11.md P2 "Canvas context menu"). Short menus (layers,
// docks) are left alone. Pure; tested in contextMenu.test.ts.
//
// Where an item goes, first match wins:
// 1. its own `section` (the files, libs, design and text menus set it; "top" keeps it at the top);
// 2. its hotkeys.ts action id or its hint chord (the items App and the edit tools build);
// 3. the section most of the other items between the same two separators have (Lock beside Weld and Break apart);
// 4. a block of unplaced items between two blocks of the same section joins them (Dock left/right/up/down between
//    Align and Flip).
// Everything else stays at the top; when the top is still too long, its later blocks go under More ▸.

import type { MenuItem } from "./Menu.tsx";

export const CONTEXT_MENU_MAX = 14;

/** Submenus in the order they are listed. */
export const SECTION_ORDER = ["modify", "arrange", "select", "laser"] as const;

export type SectionLabels = Record<string, string>;

export const DEFAULT_SECTION_LABELS: SectionLabels = { modify: "Modify", arrange: "Arrange", select: "Select", laser: "Laser", more: "More" };

const HINT_SECTION: Record<string, string> = {
  L: "arrange",
  C: "arrange",
  R: "arrange",
  T: "arrange",
  E: "arrange",
  B: "arrange",
  P: "arrange",
  "Ctrl+Shift+H": "arrange",
  "Ctrl+Shift+V": "arrange",
  "Ctrl+PgUp": "arrange",
  "Ctrl+PgDn": "arrange",
  PgUp: "arrange",
  PgDn: "arrange",
  "Alt+Home": "arrange",
  "Alt+Left": "arrange",
  "Alt+Right": "arrange",
  "Alt+Up": "arrange",
  "Alt+Down": "arrange",
  "Ctrl+A": "select",
  "Ctrl+Shift+I": "select",
  "Ctrl+`": "modify",
  "Ctrl+W": "modify",
  "Alt+B": "modify",
  "Ctrl+Shift+C": "modify",
  "Alt+O": "modify",
  "Alt+J": "modify",
  "Alt+D": "modify",
  "Alt+C": "modify",
  "Alt+Shift+C": "modify",
  "Alt+T": "modify",
  "Alt+I": "modify",
  "Ctrl+Shift+B": "modify",
  "Ctrl+Shift+M": "modify",
};

type Labelled = Exclude<MenuItem, { sep: true }>;

/** `section: "top"` keeps an item at the top level whatever its neighbours are. */
export const TOP = "top";

/** The section an item names or implies by its action id or hint (rules 1 and 2), or undefined. */
export function sectionOf(it: Labelled): string | undefined {
  if (it.section) return it.section;
  const id = it.actionId;
  if (id) {
    if (id === "edit.selectAll" || id === "edit.invert" || id.startsWith("edit.select")) return "select";
    if (id.startsWith("arrange.")) return "arrange";
    if (id.startsWith("laser.")) return "laser";
    if (id.startsWith("tool.") || id.startsWith("tools.") || id.startsWith("image.")) return "modify";
  }
  return it.hint ? HINT_SECTION[it.hint] : undefined;
}

function blocksOf(items: readonly MenuItem[]): Labelled[][] {
  const out: Labelled[][] = [[]];
  for (const it of items) {
    if (it.sep) {
      if (out[out.length - 1].length) out.push([]);
    } else out[out.length - 1].push(it);
  }
  if (!out[out.length - 1].length) out.pop();
  return out;
}

function majority(sections: (string | undefined)[]): string | undefined {
  const n = new Map<string, number>();
  for (const s of sections) if (s) n.set(s, (n.get(s) ?? 0) + 1);
  let best: string | undefined;
  let bestN = 0;
  for (const [s, k] of n) if (k > bestN) [best, bestN] = [s, k];
  return best;
}

function joinBlocks(blocks: Labelled[][]): MenuItem[] {
  const out: MenuItem[] = [];
  for (const b of blocks) {
    if (!b.length) continue;
    if (out.length) out.push({ sep: true });
    out.push(...b);
  }
  return out;
}

function count(items: readonly MenuItem[]): number {
  return items.filter((x) => !x.sep).length;
}

/**
 * `items` folded into submenus when it has more than `max` entries (see the rules at the top); otherwise unchanged.
 * Items that already open a submenu stay where they are.
 */
export function structureMenu(items: MenuItem[], labels: SectionLabels = DEFAULT_SECTION_LABELS, max = CONTEXT_MENU_MAX): MenuItem[] {
  if (count(items) <= max) return items;
  const blocks = blocksOf(items);
  // Rules 1-3 per item.
  const placed = blocks.map((b) => {
    const own = b.map((it) => (it.submenu ? undefined : sectionOf(it)));
    const inherit = majority(own.filter((s) => s !== TOP));
    return b.map((it, i) => (it.submenu || own[i] === TOP ? undefined : (own[i] ?? inherit)));
  });
  // Rule 4: an unplaced block between two blocks of one section joins it.
  const blockSection = placed.map((s) => (s.every((x) => x !== undefined) ? majority(s) : undefined));
  for (let i = 1; i < blocks.length - 1; i++) {
    if (placed[i].some((x) => x !== undefined)) continue;
    const prev = blockSection[i - 1];
    if (prev && prev === blockSection[i + 1]) placed[i] = placed[i].map(() => prev);
  }
  const top: Labelled[][] = [];
  const sections = new Map<string, Labelled[][]>();
  blocks.forEach((b, bi) => {
    const topPart: Labelled[] = [];
    const parts = new Map<string, Labelled[]>();
    b.forEach((it, i) => {
      const s = placed[bi][i];
      if (!s) topPart.push(it);
      else parts.set(s, [...(parts.get(s) ?? []), it]);
    });
    top.push(topPart);
    for (const [s, part] of parts) sections.set(s, [...(sections.get(s) ?? []), part]);
  });
  const order = [...SECTION_ORDER.filter((s) => sections.has(s)), ...[...sections.keys()].filter((s) => !(SECTION_ORDER as readonly string[]).includes(s))];
  const subRows: MenuItem[] = order.map((s) => {
    const sub = joinBlocks(sections.get(s) ?? []);
    return { label: labels[s] ?? s, submenu: sub, disabled: sub.every((x) => x.sep || x.disabled) };
  });
  // Still too long at the top: later blocks go under More.
  const room = Math.max(1, max - subRows.length - 1);
  const keep: Labelled[][] = [];
  const more: Labelled[][] = [];
  let n = 0;
  for (const b of top.filter((x) => x.length)) {
    if (!keep.length || n + b.length <= room) {
      keep.push(b);
      n += b.length;
    } else more.push(b);
  }
  if (more.length) {
    const sub = joinBlocks(more);
    subRows.push({ label: labels.more ?? "More", submenu: sub, disabled: sub.every((x) => x.sep || x.disabled) });
  }
  const out = joinBlocks(keep);
  if (subRows.length) {
    if (out.length) out.push({ sep: true });
    out.push(...subRows);
  }
  return out;
}
