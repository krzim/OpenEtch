// File pickers on iOS and iPadOS (docs/ux/ui-plan-2026-11.md 1.2, audit T18): Safari maps `accept` to file types it
// knows, and greys out files whose extension it does not (.lbrn2, .openetch, .clb, .oeart, .lbart, .lbt, ...), so they
// cannot be picked at all. There the picker gets no `accept`, and the file is checked after the pick instead, with a
// clear message for the wrong type. Elsewhere `accept` stays as it was. Pure (filePick.test.ts).

/** Something like `navigator` (tests pass their own). */
export interface NavLike {
  userAgent?: string;
  platform?: string;
  maxTouchPoints?: number;
}

const nav = (): NavLike | undefined => (typeof navigator === "undefined" ? undefined : navigator);

/** iPhone, iPad or iPod, including iPadOS that reports itself as a Mac (a "Mac" with a touch screen). */
export function isAppleTouch(n: NavLike | undefined = nav()): boolean {
  if (!n) return false;
  if (/iPad|iPhone|iPod/.test(n.userAgent ?? "")) return true;
  return n.platform === "MacIntel" && (n.maxTouchPoints ?? 0) > 1;
}

/** The `accept` attribute for a file input: the list as given, or none on iOS/iPadOS. App.tsx and FileMenu.tsx call it
 * too (`<input type="file" accept={pickerAccept(list)} />`). */
export function pickerAccept(list: string, n: NavLike | undefined = nav()): string | undefined {
  return isAppleTouch(n) ? undefined : list;
}

/** Whether a picked file matches an `accept` list (extensions such as ".svg" and MIME types such as "image/*"). An
 * empty list accepts everything. */
export function acceptsFile(file: { name: string; type?: string }, list: string): boolean {
  const parts = list
    .split(",")
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);
  if (!parts.length) return true;
  const name = file.name.toLowerCase();
  const type = (file.type ?? "").toLowerCase();
  return parts.some((a) => {
    if (a.startsWith(".")) return name.endsWith(a);
    if (a.endsWith("/*")) return type.startsWith(a.slice(0, -1));
    return type === a;
  });
}

/** The files of a pick that fit `list`, and the names of those that do not (for one message). */
export function splitPicked<F extends { name: string; type?: string }>(files: readonly F[], list: string): { ok: F[]; wrong: string[] } {
  const ok: F[] = [];
  const wrong: string[] = [];
  for (const f of files) {
    if (acceptsFile(f, list)) ok.push(f);
    else wrong.push(f.name);
  }
  return { ok, wrong };
}

/** The extensions of an `accept` list for a message (".clb", ".oeart or .lbart"). */
export function acceptLabel(list: string): string {
  const parts = list
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  if (parts.length <= 1) return parts[0] ?? "";
  if (parts.length > 6) return `${parts.slice(0, 6).join(", ")}, …`;
  return `${parts.slice(0, -1).join(", ")} or ${parts[parts.length - 1]}`;
}
