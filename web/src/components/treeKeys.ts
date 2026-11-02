// Keyboard use of a `role="tree"` list (the Browser): Tab lands on the tree, Up/Down/Home/End move between the visible
// rows, Enter or Space acts like a click, Right opens and Left closes a header row. Rows carry tabIndex -1 so they
// take focus (and the :focus-visible ring) without each being a Tab stop. Keys pressed on a row's own buttons or
// inputs are left alone, and handled keys never reach the canvas (where arrows nudge the selection).

import type { FocusEvent, KeyboardEvent } from "react";

export type TreeKeyAction = { focus: number } | { activate: true } | null;

/** What a key does on row `index` of `count` visible rows (pure; `expanded` is the row's aria-expanded, if any). */
export function treeKeyAction(key: string, index: number, count: number, expanded: string | null): TreeKeyAction {
  if (count === 0) return null;
  switch (key) {
    case "ArrowDown":
      return { focus: index < 0 ? 0 : Math.min(index + 1, count - 1) };
    case "ArrowUp":
      return { focus: index < 0 ? 0 : Math.max(index - 1, 0) };
    case "Home":
      return { focus: 0 };
    case "End":
      return { focus: count - 1 };
    case "Enter":
    case " ":
      return index < 0 ? null : { activate: true };
    case "ArrowRight":
      return index >= 0 && expanded === "false" ? { activate: true } : null;
    case "ArrowLeft":
      return index >= 0 && expanded === "true" ? { activate: true } : null;
    default:
      return null;
  }
}

function rowsOf(tree: HTMLElement): HTMLElement[] {
  return [...tree.querySelectorAll<HTMLElement>('[role="treeitem"]')].filter((el) => el.getClientRects().length > 0);
}

/** onKeyDown for the tree container. */
export function onTreeKeyDown(e: KeyboardEvent<HTMLElement>): void {
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  const tree = e.currentTarget;
  const target = e.target as HTMLElement;
  const row = target.closest<HTMLElement>('[role="treeitem"]');
  // Only keys on the tree itself or on a row (not on a row's buttons, inputs or a rename field).
  if (target !== tree && target !== row) return;
  const rows = rowsOf(tree);
  const index = row ? rows.indexOf(row) : -1;
  const act = treeKeyAction(e.key, index, rows.length, row?.getAttribute("aria-expanded") ?? null);
  if (!act) return;
  e.preventDefault();
  e.stopPropagation();
  if ("focus" in act) rows[act.focus]?.focus();
  else row?.click();
}

/** onFocus for the tree container: Tab onto the tree moves focus to the selected row, else the first. */
export function onTreeFocus(e: FocusEvent<HTMLElement>): void {
  if (e.target !== e.currentTarget) return;
  // Shift+Tab from a row lands on the tree itself: let it rest there, so the next Shift+Tab leaves the tree.
  if (e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget)) return;
  const rows = rowsOf(e.currentTarget);
  (rows.find((r) => r.classList.contains("selected")) ?? rows[0])?.focus();
}
