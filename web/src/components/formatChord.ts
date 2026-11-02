// Shortcut hints for menus, tooltips and command search (docs/ux/ui-plan-2026-11.md P2 "Hints"). Chords are written
// the hotkeys.ts way ("Ctrl+Shift+H"; Cmd counts as Ctrl there); on Apple platforms they read as ⌥⇧⌘H, elsewhere
// they stay as written. Text that is not a chord (".lbrn2", "↗") comes back unchanged. Pure; no DOM.

export type Platform = "mac" | "win";

/** Where the app runs: "mac" on macOS, iPadOS and iOS (Cmd stands for Ctrl there), else "win". */
export function platformOf(nav: { platform?: string; userAgent?: string } | undefined): Platform {
  if (!nav) return "win";
  const s = `${nav.platform ?? ""} ${nav.userAgent ?? ""}`;
  return /Mac|iPhone|iPad|iPod/.test(s) ? "mac" : "win";
}

/** The current platform (from `navigator`; "win" outside a browser). */
export function currentPlatform(): Platform {
  return platformOf(typeof navigator === "undefined" ? undefined : navigator);
}

const MAC_KEYS: Record<string, string> = {
  Left: "←",
  Right: "→",
  Up: "↑",
  Down: "↓",
  Del: "⌦",
  Backspace: "⌫",
  Esc: "⎋",
  Enter: "↩",
  Return: "↩",
  Tab: "⇥",
  PgUp: "⇞",
  PgDn: "⇟",
  Home: "↖",
  End: "↘",
};

const MODS = new Set(["ctrl", "cmd", "control", "alt", "option", "shift"]);

/** Split "Ctrl+Shift++" into ["Ctrl", "Shift", "+"]; null when a part before the key is not a modifier. */
function split(chord: string): { mods: string[]; key: string } | null {
  const t = chord.trim();
  if (!t) return null;
  const plusKey = t === "+" || t.endsWith("++");
  const head = plusKey ? t.slice(0, t === "+" ? 0 : -2) : t.slice(0, Math.max(0, t.lastIndexOf("+")));
  const key = plusKey ? "+" : t.slice(t.lastIndexOf("+") + 1);
  if (!key) return null;
  const mods = head ? head.split("+") : [];
  if (!mods.every((m) => MODS.has(m.toLowerCase()))) return null;
  return { mods: mods.map((m) => m.toLowerCase()), key };
}

/**
 * A chord for display: "Ctrl+Shift+H" stays on Windows and Linux and becomes "⇧⌘H" on a Mac (Option ⌥, Shift ⇧,
 * Cmd ⌘, in Apple's order). Several chords joined with ", " or " / " are formatted one by one.
 */
export function formatChord(chord: string, platform: Platform = currentPlatform()): string {
  if (!chord) return "";
  if (chord.includes(", ")) return chord.split(", ").map((c) => formatChord(c, platform)).join(", ");
  if (chord.includes(" / ")) return chord.split(" / ").map((c) => formatChord(c, platform)).join(" / ");
  if (platform !== "mac") return chord;
  const p = split(chord);
  if (!p) return chord;
  // A bare word key that is not a known key name is plain text (".lbrn2", "Double-click").
  if (!p.mods.length && p.key.length > 1 && !(p.key in MAC_KEYS) && !/^F\d{1,2}$/.test(p.key)) return chord;
  const has = (...names: string[]) => p.mods.some((m) => names.includes(m));
  const key = MAC_KEYS[p.key] ?? (p.key.length === 1 ? p.key.toUpperCase() : p.key);
  return `${has("alt", "option") ? "⌥" : ""}${has("shift") ? "⇧" : ""}${has("ctrl", "cmd", "control") ? "⌘" : ""}${key}`;
}
