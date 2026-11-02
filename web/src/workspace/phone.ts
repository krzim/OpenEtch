// Pure rules of the phone shell (docs/ux/ui-plan-2026-11.md 1.1): the bottom sheet's heights and where a drag on its
// handle settles. No React, so `npm test` covers it (shell.test.ts).

export type SheetSnap = "peek" | "half" | "full";
export const SHEET_SNAPS: readonly SheetSnap[] = ["peek", "half", "full"];

/** Share of the space between the top bar and the tab bar per height. */
export const SNAP_FRACTION: Record<SheetSnap, number> = { peek: 0.3, half: 0.55, full: 1 };

/** The height a drag ending at `fraction` of the space settles on, or null to close (dragged well under the peek). */
export function snapFor(fraction: number): SheetSnap | null {
  if (fraction < SNAP_FRACTION.peek * 0.6) return null;
  let best: SheetSnap = "peek";
  for (const s of SHEET_SNAPS) if (Math.abs(SNAP_FRACTION[s] - fraction) < Math.abs(SNAP_FRACTION[best] - fraction)) best = s;
  return best;
}
