// Which ribbon groups collapse into `GROUP ▾` buttons so the toolbar never scrolls (docs/ux/ui-plan-2026-11.md P2
// "Ribbon", V2/L3/UX-02). Pure; Ribbon.tsx measures the groups and asks `fitGroups`. Tested in ribbon.test.ts.

export interface GroupFit {
  id: string;
  /** Width when shown inline (px). */
  width: number;
  /** Width as a collapsed `GROUP ▾` button (px). */
  collapsedWidth: number;
  /** Higher stays inline longer. */
  priority: number;
  /** Never collapses. */
  pinned?: boolean;
}

/**
 * The order groups collapse in: lowest priority first; among equal priorities the later group first (the ribbon
 * folds from its end, like Fusion's). Pinned groups never collapse.
 */
export function collapseOrder(groups: readonly GroupFit[]): string[] {
  return groups
    .map((g, i) => ({ g, i }))
    .filter(({ g }) => !g.pinned)
    .sort((a, b) => a.g.priority - b.g.priority || b.i - a.i)
    .map(({ g }) => g.id);
}

/**
 * The groups to collapse so that everything fits in `available` px. Groups collapse one at a time in
 * `collapseOrder`, so the result only grows as `available` shrinks (it is always a prefix of that order). When even
 * all of them collapsed do not fit, every collapsible group is collapsed (the toolbar then scrolls as a last resort).
 */
export function fitGroups(groups: readonly GroupFit[], available: number): Set<string> {
  let total = groups.reduce((s, g) => s + g.width, 0);
  const out = new Set<string>();
  if (total <= available) return out;
  const byId = new Map(groups.map((g) => [g.id, g]));
  for (const id of collapseOrder(groups)) {
    const g = byId.get(id);
    if (!g) continue;
    out.add(id);
    total += g.collapsedWidth - g.width;
    if (total <= available) break;
  }
  return out;
}

export interface RailFit {
  id: string;
  /** Height on the rail (px): a pinned group's tools inline, another group's button. */
  height: number;
  priority: number;
  pinned?: boolean;
}

/**
 * The tablet rail's groups that move into its More flyout so the rest fits in `available` px (the rail never hides a
 * tool below the window: an iPad in landscape has room for about 14 of its 44 px buttons). As the canvas HUD keeps
 * the required controls and lets the optional ones give way, the lowest-priority groups go first (`collapseOrder`,
 * pinned never), and the More button itself (`moreHeight`, with its gap) is counted once anything moves. When even
 * that does not fit, every movable group is in More and the rail scrolls as a last resort.
 */
export function railOverflow(groups: readonly RailFit[], available: number, moreHeight: number): Set<string> {
  let total = groups.reduce((s, g) => s + g.height, 0);
  const out = new Set<string>();
  if (total <= available) return out;
  total += moreHeight;
  const byId = new Map(groups.map((g) => [g.id, g]));
  for (const id of collapseOrder(groups.map((g) => ({ id: g.id, width: 0, collapsedWidth: 0, priority: g.priority, pinned: g.pinned })))) {
    const g = byId.get(id);
    if (!g) continue;
    out.add(id);
    total -= g.height;
    if (total <= available) break;
  }
  return out;
}

/** Same sets (to skip a re-render when the fit did not change). */
export function sameSet(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  if (a.size !== b.size) return false;
  for (const x of a) if (!b.has(x)) return false;
  return true;
}

/** Whether a flyout already sits at `r`: placed (no hidden starting style left) and the same box. Its position effect
 *  runs after every render and sets state only when this is false, so it never re-renders itself forever. */
export function samePos(
  old: { left?: unknown; top?: unknown; maxHeight?: unknown; visibility?: unknown },
  r: { left: number; top: number; maxHeight?: number },
): boolean {
  return old.visibility === undefined && old.left === r.left && old.top === r.top && old.maxHeight === r.maxHeight;
}
