// The ribbon's own option (owner decision 6 of docs/ux/ui-plan-2026-11.md): LightBurn-style creation tools on a
// vertical rail at the canvas's left edge on desktop. Off by default. Stored with the other preferences
// (workspace/prefs.ts `creationRail`); toggled from the ribbon's right-click menu and command search. A choice saved
// under the ribbon's old key (before it moved into the preferences) is carried over once.

import { getPrefs, updatePrefs } from "../workspace/prefs.ts";
import { usePrefs } from "../workspace/usePrefs.ts";

const OLD_KEY = "openetch.ribbon.creationRail";

/** Carry a choice saved under the old key into the preferences, then drop the key. */
export function migrateCreationRail(store: Pick<Storage, "getItem" | "removeItem"> | null): void {
  try {
    if (!store || store.getItem(OLD_KEY) === null) return;
    if (store.getItem(OLD_KEY) === "1") updatePrefs({ creationRail: true });
    store.removeItem(OLD_KEY);
  } catch {
    // Storage blocked (private window): nothing to carry over.
  }
}

migrateCreationRail(oldStore());

function oldStore(): Pick<Storage, "getItem" | "removeItem"> | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function getCreationRail(): boolean {
  return getPrefs().creationRail;
}

export function setCreationRail(on: boolean): void {
  updatePrefs({ creationRail: on });
}

export function useCreationRail(): boolean {
  return usePrefs().creationRail;
}
