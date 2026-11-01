// Project folders and locations (UX-013; engine crates/engine/src/folders.rs): the folder tree the Save / Open dialogs
// show, which projects sit in a folder, and the checks of the Project locations setting. A project name carries its
// folders (`Clients/Acme/badge`); a location is mounted as a top folder under its label. Pure; tested in files.test.ts.

import { validateProjectName } from "../api/project.ts";

/** A row of the folder tree: `path` "" is the projects directory itself. */
export interface FolderRow {
  path: string;
  /** Last segment ("" for the root). */
  name: string;
  depth: number;
  /** A mounted location (a top folder that is another directory). */
  location: boolean;
  /** The location's directory is not reachable now. */
  unreachable: boolean;
}

/** A project location as GET /api/projects/locations lists it. */
export interface ProjectLocation {
  label: string;
  path: string;
  /** "cli" (`--projects-mount`, fixed) or "settings" (saved from the UI). */
  source: string;
  reachable: boolean;
}

const str = (v: unknown): string => (typeof v === "string" ? v : "");

/** The locations of a GET /api/projects/locations (or /folders) answer, valid rows only. */
export function parseLocations(raw: unknown): ProjectLocation[] {
  const list = raw && typeof raw === "object" ? (raw as { locations?: unknown }).locations : null;
  if (!Array.isArray(list)) return [];
  return list
    .filter((l): l is Record<string, unknown> => !!l && typeof l === "object" && typeof (l as { label?: unknown }).label === "string")
    .map((l) => ({ label: str(l.label), path: str(l.path), source: str(l.source) || "settings", reachable: l.reachable !== false }));
}

/** The folders of a GET /api/projects/folders answer. */
export function parseFolders(raw: unknown): string[] {
  const list = raw && typeof raw === "object" ? (raw as { folders?: unknown }).folders : null;
  return Array.isArray(list) ? list.filter((f): f is string => typeof f === "string" && f.length > 0) : [];
}

/** The folder a project name sits in ("" for the projects directory). */
export function folderOf(name: string): string {
  const i = name.lastIndexOf("/");
  return i < 0 ? "" : name.slice(0, i);
}

/** A project name without its folders. */
export function baseNameOf(name: string): string {
  return name.slice(name.lastIndexOf("/") + 1);
}

/** `folder/name`, or `name` in the projects directory. */
export function joinProject(folder: string, name: string): string {
  return folder ? `${folder}/${name}` : name;
}

/**
 * The tree rows, depth first and sorted by name (case-insensitive) under each folder: the root, every folder the
 * engine lists, the folders of the listed projects (in case the folder list was cut off) and every location.
 */
export function folderRows(folders: readonly string[], projectNames: readonly string[] = [], locations: readonly ProjectLocation[] = []): FolderRow[] {
  // Keyed by the lower-case path: folders differing only in case are one folder on Windows and macOS. A deeper path
  // takes the spelling of the folders above it already seen.
  const all = new Map<string, string>();
  const add = (path: string) => {
    const segs = path.split("/").filter(Boolean);
    let spelled = "";
    for (let i = 0; i < segs.length; i++) {
      const k = segs.slice(0, i + 1).join("/").toLowerCase();
      const known = all.get(k);
      spelled = known ?? (spelled ? `${spelled}/${segs[i]}` : segs[i]!);
      if (!known) all.set(k, spelled);
    }
  };
  folders.forEach(add);
  projectNames.forEach((n) => add(folderOf(n)));
  locations.forEach((l) => add(l.label));
  const loc = new Map(locations.map((l) => [l.label.toLowerCase(), l]));
  const key = (p: string) => p.split("/").map((s) => s.toLowerCase());
  const sorted = [...all.values()].sort((a, b) => {
    const ka = key(a);
    const kb = key(b);
    for (let i = 0; i < Math.min(ka.length, kb.length); i++) {
      const c = ka[i]!.localeCompare(kb[i]!);
      if (c !== 0) return c;
    }
    return ka.length - kb.length;
  });
  return [
    { path: "", name: "", depth: 0, location: false, unreachable: false },
    ...sorted.map((path) => {
      const segs = path.split("/");
      const l = segs.length === 1 ? loc.get(path.toLowerCase()) : undefined;
      return { path, name: segs[segs.length - 1]!, depth: segs.length, location: !!l, unreachable: !!l && !l.reachable };
    }),
  ];
}

/** The rows shown when only the folders in `open` are expanded (the root's children always show). */
export function visibleRows(rows: readonly FolderRow[], open: ReadonlySet<string>): FolderRow[] {
  return rows.filter((r) => {
    if (r.depth <= 1) return true;
    const segs = r.path.split("/");
    for (let i = 1; i < segs.length; i++) if (!open.has(segs.slice(0, i).join("/"))) return false;
    return true;
  });
}

/** Whether `row` has folders under it. */
export function hasChildren(rows: readonly FolderRow[], row: FolderRow): boolean {
  return rows.some((r) => r.depth === row.depth + 1 && r.path.startsWith(row.path ? `${row.path}/` : ""));
}

/** The projects directly in `folder`. */
export function projectsIn<T extends { name: string }>(list: readonly T[], folder: string): T[] {
  return list.filter((p) => folderOf(p.name).toLowerCase() === folder.toLowerCase());
}

/** Why a new folder `name` cannot go under `parent`, or null. */
export function newFolderProblem(parent: string, name: string): string | null {
  const n = name.trim();
  if (!n) return "Give the folder a name.";
  if (n.includes("/")) return "One folder at a time: no \"/\" in the name.";
  // A folder path follows the project name rules with one folder fewer (the name itself).
  const problem = validateProjectName(`${joinProject(parent, n)}/x`);
  return problem;
}

/** A location row being edited in the Project locations setting. */
export interface LocationDraft {
  label: string;
  path: string;
}

/** The engine's limit on locations. */
export const MAX_LOCATIONS = 16;

/**
 * Why the saved locations cannot be stored (the engine's checks it can make without the disk: label rules, unique
 * labels, absolute paths), or null. `fixed` are the command line's locations, which stay.
 */
export function locationsProblem(drafts: readonly LocationDraft[], fixed: readonly ProjectLocation[] = []): string | null {
  if (drafts.length + fixed.length > MAX_LOCATIONS) return `At most ${MAX_LOCATIONS} locations.`;
  const seen = new Set(fixed.map((l) => l.label.toLowerCase()));
  for (const d of drafts) {
    const label = d.label.trim();
    const p = validateProjectName(label);
    if (p) return `Label "${label}": ${p}`;
    if (label.includes("/")) return `Label "${label}": one name, no "/".`;
    if (["templates", "art-libraries"].includes(label.toLowerCase())) return `"${label}" holds OpenEtch's own files; pick another label.`;
    if (seen.has(label.toLowerCase())) return `Two locations are labelled "${label}".`;
    seen.add(label.toLowerCase());
    const path = d.path.trim();
    if (!(path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\"))) return `Location "${label}": give the folder's full path (C:\\… or /…).`;
  }
  return null;
}
