// Project, template and file names, checked as the engine checks them (crates/engine/src/projects.rs), with the
// same messages, so the design-only build answers like the engine. A project path is a name inside at most four
// `/`-separated folders.

export const MAX_NAME_CHARS = 100;
export const MAX_FOLDER_DEPTH = 4;
/** Top folders that hold OpenEtch's own files, never projects. */
export const INTERNAL_DIRS = ["templates", "art-libraries"];
export const PROJECT_EXT = "openetch";

const RESERVED = new Set([
  "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "com¹",
  "com²", "com³", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²",
  "lpt³",
]);

/** Rust's `{:?}` of a string, near enough for messages. */
function debug(s: string): string {
  return JSON.stringify(s);
}

/** Whether two names name the same file on a case-insensitive file system. */
export function sameNameIgnoringCase(a: string, b: string): boolean {
  return a === b || a.toLowerCase() === b.toLowerCase();
}

/** The problem with a single name (no folders), or null. */
export function nameProblem(name: string): string | null {
  const bad = (why: string) =>
    `invalid project name ${debug(name)}: ${why} (use 1-${MAX_NAME_CHARS} letters, digits, spaces and - _ . ( ))`;
  const chars = [...name];
  if (chars.length === 0) return bad("it is empty");
  if (chars.length > MAX_NAME_CHARS) return bad("it is too long");
  const wrong = chars.find((c) => !/^[\p{Alphabetic}\p{N} \-_.()]$/u.test(c));
  if (wrong !== undefined) return bad(`'${wrong}' is not allowed`);
  if (name.startsWith(".")) return bad("it starts with a dot");
  if (name.includes("..")) return bad('it contains ".."');
  if (name.startsWith(" ") || name.endsWith(" ")) return bad("it starts or ends with a space");
  const stem = (name.split(".")[0] ?? "").trimEnd().toLowerCase();
  if (RESERVED.has(stem)) return bad("it is a reserved device name on Windows");
  return null;
}

function pathProblem(path: string, what: "project name" | "folder"): string | null {
  const segs = path.split("/");
  if (segs.length > MAX_FOLDER_DEPTH + 1) return `invalid ${what} ${debug(path)}: at most ${MAX_FOLDER_DEPTH} folders deep`;
  for (const s of segs) {
    const p = nameProblem(s);
    if (p) return p;
  }
  const top = what === "folder" ? segs[0] : segs.length > 1 ? segs[0] : undefined;
  if (top !== undefined && INTERNAL_DIRS.some((d) => d.toLowerCase() === top.toLowerCase())) {
    return `invalid ${what} ${debug(path)}: "${top}" holds OpenEtch's own files, not projects`;
  }
  return null;
}

/** The problem with a project path (folders and name), or null. */
export function projectNameProblem(name: string): string | null {
  return pathProblem(name, "project name");
}

/** The problem with a folder path (`""` is the top), or null. */
export function folderProblem(path: string): string | null {
  return path === "" ? null : pathProblem(path, "folder");
}

/** The folders part of a project path (`""` at the top). */
export function folderOf(name: string): string {
  const i = name.lastIndexOf("/");
  return i < 0 ? "" : name.slice(0, i);
}

/** A project path as one flat file name, for autosaves and backups (`Clients/Acme/badge` → `Clients__Acme__badge`). */
export function flatName(name: string): string {
  return name.split("/").join("__");
}

/** A file name as a display name: control characters and a known extension dropped, trimmed, ≤ 200 characters. */
export function displayName(raw: string): string | null {
  // eslint-disable-next-line no-control-regex
  let name = raw.replace(/[\u0000-\u001f\u007f-\u009f/\\]/g, "").trim();
  for (const ext of [".lbrn2", ".lbrn", ".lbt", ".openetch"]) {
    if (name.length >= ext.length && name.slice(name.length - ext.length).toLowerCase() === ext) {
      name = name.slice(0, name.length - ext.length).trimEnd();
      break;
    }
  }
  name = [...name].slice(0, 200).join("");
  return name === "" ? null : name;
}

/** A stored copy's file name (`<stem>.<unix>.<ext>`) split, or null when it is not one. */
export function splitStamped(file: string, ext: string): { stem: string; unix: number } | null {
  const suffix = `.${ext}`;
  if (!file.endsWith(suffix)) return null;
  const rest = file.slice(0, -suffix.length);
  const i = rest.lastIndexOf(".");
  if (i < 0) return null;
  const ts = rest.slice(i + 1);
  if (!/^\d+$/.test(ts)) return null;
  return { stem: rest.slice(0, i), unix: Number(ts) };
}

/** A file name inside one of the shell's stores: no separators, no leading dot, ends with `ext`. */
export function plainFileProblem(name: string, ext: string): string | null {
  const ok =
    name !== "" && name.length <= 200 && !name.startsWith(".") && !/[/\\:]/.test(name) && !name.includes("..") && name.endsWith(ext);
  return ok ? null : `bad file name ${debug(name)}`;
}
