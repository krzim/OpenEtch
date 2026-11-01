// Pure model of the Library window, layer/optimization defaults, the Job Checklist and Require Framing, the IO ports,
// the job log, the update check, Find My Laser, the start point tool, Print and Cut, Batch and Repeat Marking (ARCHITECTURE.md
// "Libraries, defaults, logs, diagnostics, camera" and "Safety model"). Everything the components show or send is
// built here so `npm test` covers it (model.test.ts).

import type { GDocument, Layer, MachineStatus, Vec2 } from "../api/types.ts";
import type { NodeList } from "../nodes.ts";

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const obj = (v: unknown): Record<string, unknown> | null => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null);

// ---- material library -----------------------------------------------------------------------------------------------

export interface EntrySummary {
  mode: string;
  power_pct: number;
  speed_mm_s: number;
  frequency_khz: number;
  pulse_width_ns: number | null;
  passes: number;
  interval_mm: number;
  sub_layers: number;
}

export interface LibEntry {
  /** `m<material>.e<entry>`: the engine's id within its library (changes when entries move). */
  id: string;
  material: string;
  thickness_mm: number | null;
  title: string;
  /** The thickness or title folder. */
  group: string;
  description: string;
  /** LightBurn's LinkPath: `material/thickness or title/description`. */
  path: string;
  summary: EntrySummary | null;
}

export interface LibGroup {
  title: string;
  entries: LibEntry[];
}

export interface LibMaterial {
  name: string;
  groups: LibGroup[];
}

export interface LibraryInfo {
  name: string;
  entries: number;
  materials: number;
  /** Changed since it was last saved (edits stay in memory until Save). */
  dirty: boolean;
}

export interface LibraryView {
  selected: string | null;
  libraries: LibraryInfo[];
  /** The selected library's tree. */
  tree: LibMaterial[];
}

function parseSummary(v: unknown): EntrySummary | null {
  const s = obj(v);
  if (!s) return null;
  return {
    mode: str(s.mode) || "line",
    power_pct: isNum(s.power_pct) ? s.power_pct : 0,
    speed_mm_s: isNum(s.speed_mm_s) ? s.speed_mm_s : 0,
    frequency_khz: isNum(s.frequency_khz) ? s.frequency_khz : 0,
    pulse_width_ns: isNum(s.pulse_width_ns) ? s.pulse_width_ns : null,
    passes: isNum(s.passes) ? s.passes : 1,
    interval_mm: isNum(s.interval_mm) ? s.interval_mm : 0,
    sub_layers: isNum(s.sub_layers) ? s.sub_layers : 0,
  };
}

/** The `tree` of GET /api/library (or GET /api/library/{name}). */
export function parseTree(raw: unknown): LibMaterial[] {
  const t = obj(raw);
  const mats = Array.isArray(t?.materials) ? t.materials : [];
  return mats.flatMap((m): LibMaterial[] => {
    const mo = obj(m);
    if (!mo) return [];
    const groups = (Array.isArray(mo.groups) ? mo.groups : []).flatMap((g): LibGroup[] => {
      const go = obj(g);
      if (!go) return [];
      const entries = (Array.isArray(go.entries) ? go.entries : []).flatMap((e): LibEntry[] => {
        const eo = obj(e);
        if (!eo || typeof eo.id !== "string") return [];
        return [
          {
            id: eo.id,
            material: str(eo.material) || str(mo.name),
            thickness_mm: isNum(eo.thickness_mm) ? eo.thickness_mm : null,
            title: str(eo.title),
            group: str(eo.group) || str(go.title),
            description: str(eo.description),
            path: str(eo.path),
            summary: parseSummary(eo.summary),
          },
        ];
      });
      return [{ title: str(go.title), entries }];
    });
    return [{ name: str(mo.name), groups }];
  });
}

/** GET /api/library. */
export function parseLibraryView(raw: unknown): LibraryView {
  const v = obj(raw) ?? {};
  const libraries = (Array.isArray(v.libraries) ? v.libraries : []).flatMap((l): LibraryInfo[] => {
    const lo = obj(l);
    if (!lo || typeof lo.name !== "string") return [];
    return [{ name: lo.name, entries: isNum(lo.entries) ? lo.entries : 0, materials: isNum(lo.materials) ? lo.materials : 0, dirty: lo.dirty === true }];
  });
  return { selected: typeof v.selected === "string" ? v.selected : null, libraries, tree: parseTree(v.tree) };
}

/** Every entry of a tree, in tree order. */
export function treeEntries(tree: readonly LibMaterial[]): LibEntry[] {
  return tree.flatMap((m) => m.groups.flatMap((g) => g.entries));
}

/** The tree with only the entries whose material, folder or description contains `query` (case-insensitive). */
export function filterTree(tree: readonly LibMaterial[], query: string): LibMaterial[] {
  const q = query.trim().toLowerCase();
  if (!q) return [...tree];
  return tree.flatMap((m) => {
    const hitM = m.name.toLowerCase().includes(q);
    const groups = m.groups.flatMap((g) => {
      const hitG = hitM || g.title.toLowerCase().includes(q);
      const entries = hitG ? g.entries : g.entries.filter((e) => e.description.toLowerCase().includes(q));
      return entries.length ? [{ ...g, entries }] : [];
    });
    return groups.length ? [{ ...m, groups }] : [];
  });
}

/** A library name from a picked file: its name without `.clb`, characters a file name cannot hold replaced. */
export function libraryNameFromFile(fileName: string): string {
  const base = fileName.replace(/^.*[\\/]/, "").replace(/\.clb$/i, "");
  const clean = base.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim();
  return clean || "Library";
}

/** A library name the engine accepts (`projects::validate_name`'s rules, roughly): null when fine, else why not. */
export function libraryNameProblem(name: string, existing: readonly string[] = [], current?: string): string | null {
  const n = name.trim();
  if (!n) return "Give the library a name.";
  if (n.length > 120) return "At most 120 characters.";
  if (/[<>:"/\\|?*\u0000-\u001f]/.test(n) || n.startsWith(".")) return "A name cannot hold < > : \" / \\ | ? * or start with a dot.";
  const lower = n.toLowerCase();
  if (existing.some((e) => e.toLowerCase() === lower && e.toLowerCase() !== (current ?? "").toLowerCase())) return `A library named "${n}" is already loaded.`;
  return null;
}

export interface EntryMeta {
  material: string;
  /** null with No Thickness. */
  thickness_mm: number | null;
  title: string;
  description: string;
}

/** `EntryMeta::check` of the engine: material and description 1-200 characters, no '/'; a title with No Thickness. */
export function entryMetaProblem(m: EntryMeta): Partial<Record<keyof EntryMeta, string>> {
  const err: Partial<Record<keyof EntryMeta, string>> = {};
  const text = (k: keyof EntryMeta, v: string, required: boolean) => {
    if (required && !v.trim()) err[k] = "Required";
    else if ([...v].length > 200) err[k] = "At most 200 characters";
    else if (v.includes("/")) err[k] = "No '/' (it separates the link path)";
    else if (/[\u0000-\u001f\u007f]/.test(v)) err[k] = "No control characters";
  };
  text("material", m.material, true);
  text("description", m.description, true);
  text("title", m.title, m.thickness_mm === null);
  if (m.thickness_mm !== null && !(isNum(m.thickness_mm) && m.thickness_mm > 0 && m.thickness_mm <= 1000)) err.thickness_mm = "0 to 1000 mm";
  return err;
}

/** The metadata of an existing entry, for Edit Description. */
export function entryMeta(e: LibEntry): EntryMeta {
  return { material: e.material, thickness_mm: e.thickness_mm, title: e.title, description: e.description };
}

/** A starting point for "Create new from layer": the layer's name as the description, a 1 mm thickness. */
export function newEntryMeta(layer: Layer | null, material = ""): EntryMeta {
  return { material, thickness_mm: null, title: layer ? modeTitle(layer.settings.mode) : "", description: layer?.name ?? "" };
}

function modeTitle(mode: string): string {
  return mode === "line" ? "Score" : mode === "fill" || mode === "offset_fill" ? "Engrave" : mode === "fill_then_line" ? "Engrave + score" : "Engrave";
}

/** The preset a layer is linked to (`Layer.link`), or null. */
export function layerLink(layer: Layer | null | undefined): { library: string; path: string } | null {
  const l = obj((layer as { link?: unknown } | null | undefined)?.link);
  return l && typeof l.library === "string" && typeof l.path === "string" ? { library: l.library, path: l.path } : null;
}

/** Layers linked to `path` in `library` (names compared ignoring case, as the engine does). */
export function linkedLayers(doc: GDocument | null, library: string, path: string): Layer[] {
  if (!doc) return [];
  const lib = library.toLowerCase();
  return doc.layers.filter((l) => {
    const k = layerLink(l);
    return !!k && k.library.toLowerCase() === lib && k.path === path;
  });
}

/** The entry at a link path in a tree, or null. */
export function entryAtPath(tree: readonly LibMaterial[], path: string): LibEntry | null {
  return treeEntries(tree).find((e) => e.path === path) ?? null;
}

/** "Fill · 30% · 1000 mm/s · 40 kHz · 200 ns · ×2 · +1 sub-layer". */
export function entrySummaryText(s: EntrySummary | null, speed: (mmS: number) => string = (v) => `${round(v, 1)} mm/s`): string {
  if (!s) return "Settings not readable";
  const mode = s.mode === "fill" ? "Fill" : s.mode === "fill_then_line" ? "Fill+Line" : s.mode === "offset_fill" ? "Offset Fill" : s.mode === "line" ? "Line" : s.mode;
  const parts = [mode, `${round(s.power_pct, 1)}%`, speed(s.speed_mm_s), `${round(s.frequency_khz, 1)} kHz`];
  if (s.pulse_width_ns !== null) parts.push(`${round(s.pulse_width_ns, 0)} ns`);
  if (s.passes !== 1) parts.push(`×${s.passes}`);
  if (s.mode !== "line") parts.push(`${round(s.interval_mm, 4)} mm`);
  if (s.sub_layers > 0) parts.push(`+${s.sub_layers} sub-layer${s.sub_layers === 1 ? "" : "s"}`);
  return parts.join(" · ");
}

export function round(v: number, digits: number): number {
  const f = 10 ** digits;
  return Math.round(v * f) / f;
}

export interface CutEdit {
  mode: string;
  power_pct: number;
  speed_mm_s: number;
  frequency_khz: number;
  pulse_width_ns: number | null;
  passes: number;
  interval_mm: number;
}

/** The fields Edit Cut offers, from an entry's summary. */
export function cutEditOf(s: EntrySummary): CutEdit {
  return { mode: s.mode, power_pct: s.power_pct, speed_mm_s: s.speed_mm_s, frequency_khz: s.frequency_khz, pulse_width_ns: s.pulse_width_ns, passes: s.passes, interval_mm: s.interval_mm };
}

/**
 * The complete CutSettings Edit Cut sends: `base` (a layer linked to the preset, which holds every setting of it) with
 * the edited fields, or only the edited fields (the engine fills in its defaults for the rest).
 */
export function editCutSettings(base: Record<string, unknown> | null, e: CutEdit): Record<string, unknown> {
  const b = base ? structuredCloneJson(base) : {};
  const hatch = { ...(obj(b.hatch) ?? {}), interval_mm: e.interval_mm };
  return { ...b, mode: e.mode, power_pct: e.power_pct, speed_mm_s: e.speed_mm_s, frequency_khz: e.frequency_khz, pulse_width_ns: e.pulse_width_ns, passes: Math.max(1, Math.round(e.passes)), hatch };
}

function structuredCloneJson<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T;
}

/** A problem with the edited cut values, or null. */
export function cutEditProblem(e: CutEdit): string | null {
  if (!(e.power_pct >= 0 && e.power_pct <= 100)) return "Power is 0 to 100 %.";
  if (!(e.speed_mm_s > 0)) return "Speed must be more than 0.";
  if (!(e.frequency_khz > 0)) return "Frequency must be more than 0.";
  if (!(Number.isInteger(e.passes) && e.passes >= 1 && e.passes <= 1000)) return "Passes are 1 to 1000.";
  if (e.mode !== "line" && !(e.interval_mm > 0)) return "The interval must be more than 0.";
  return null;
}

// ---- preferences ----------------------------------------------------------------------------------------------------

export interface Checklist {
  enabled: boolean;
  items: string[];
}

export interface DevicePrefs {
  device: string | null;
  require_framing: boolean;
  play_sound: boolean;
  checklist: Checklist;
  load_defaults_on_new: boolean;
  /** Saved per-colour layer defaults (colour keys, `*` for Make Default for All). */
  layer_defaults: string[];
  has_optimization_defaults: boolean;
  library: string | null;
  debug_log: boolean;
  check_for_updates: boolean;
}

export const NO_PREFS: DevicePrefs = {
  device: null,
  require_framing: false,
  play_sound: false,
  checklist: { enabled: false, items: [] },
  load_defaults_on_new: false,
  layer_defaults: [],
  has_optimization_defaults: false,
  library: null,
  debug_log: false,
  check_for_updates: false,
};

/** GET /api/prefs. */
export function parsePrefs(raw: unknown): DevicePrefs {
  const v = obj(raw);
  if (!v) return NO_PREFS;
  const d = obj(v.device_prefs) ?? {};
  const c = obj(d.checklist) ?? {};
  return {
    device: typeof v.device === "string" ? v.device : null,
    require_framing: d.require_framing === true,
    play_sound: d.play_sound === true,
    checklist: { enabled: c.enabled === true, items: Array.isArray(c.items) ? c.items.filter((x): x is string => typeof x === "string") : [] },
    load_defaults_on_new: d.load_defaults_on_new === true,
    layer_defaults: Object.keys(obj(d.layer_defaults) ?? {}),
    has_optimization_defaults: obj(d.optimization_defaults) !== null,
    library: typeof d.library === "string" ? d.library : null,
    debug_log: v.debug_log === true,
    check_for_updates: v.check_for_updates === true,
  };
}

export const MAX_CHECKLIST_ITEMS = 30;
export const MAX_CHECKLIST_ITEM_CHARS = 300;

/** The engine's `Checklist::problem`: at most 30 items of 1-300 characters; an enabled list needs one. */
export function checklistProblem(c: Checklist): string | null {
  if (c.items.length > MAX_CHECKLIST_ITEMS) return `At most ${MAX_CHECKLIST_ITEMS} checklist items.`;
  const bad = c.items.find((i) => !i.trim() || [...i].length > MAX_CHECKLIST_ITEM_CHARS);
  if (bad !== undefined) return `Each item needs 1 to ${MAX_CHECKLIST_ITEM_CHARS} characters.`;
  if (c.enabled && c.items.length === 0) return "An enabled checklist needs at least one item.";
  return null;
}

/** LightBurn-like starting items for a new checklist. */
export const DEFAULT_CHECKLIST = ["Enclosure closed and door interlock working", "Laser safety eyewear on", "Focus and material height checked", "Fume extraction on"];

/** Checklist items as edited in a text area: one per line, blank lines dropped. */
export function checklistFromText(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
}

/** The ticks the approval sends: null when the checklist is off. */
export function checklistTicks(c: Checklist, ticked: readonly boolean[]): boolean[] | null {
  if (!c.enabled) return null;
  return c.items.map((_, i) => ticked[i] === true);
}

/** Every item ticked (always true with the checklist off). */
export function checklistComplete(c: Checklist, ticked: readonly boolean[]): boolean {
  return !c.enabled || (c.items.length > 0 && c.items.every((_, i) => ticked[i] === true));
}

// ---- IO ports (MachineProfile.io) -----------------------------------------------------------------------------------

export interface InputPort {
  bit: number;
  active_low: boolean;
}

export interface IoPorts {
  door_protect: InputPort | null;
  start_framing: InputPort | null;
  busy_light: number | null;
  ready_light: number | null;
  done_marking: number | null;
  done_pulse_ms: number;
  red_dot_always_on: boolean;
}

function input(v: unknown): InputPort | null {
  const o = obj(v);
  return o && isNum(o.bit) ? { bit: o.bit, active_low: o.active_low === true } : null;
}

/** A device profile's `io` (absent fields are the defaults). */
export function ioPorts(profile: Record<string, unknown> | null | undefined): IoPorts {
  const io = obj(profile?.io) ?? {};
  const bit = (v: unknown) => (isNum(v) ? v : null);
  return {
    door_protect: input(io.door_protect),
    start_framing: input(io.start_framing),
    busy_light: bit(io.busy_light),
    ready_light: bit(io.ready_light),
    done_marking: bit(io.done_marking),
    done_pulse_ms: isNum(io.done_pulse_ms) ? io.done_pulse_ms : 500,
    red_dot_always_on: io.red_dot_always_on === true,
  };
}

/** "input 3 (active low)". */
export function inputText(p: InputPort): string {
  return `input ${p.bit}${p.active_low ? " (active low)" : ""}`;
}

// ---- approval -------------------------------------------------------------------------------------------------------

/** The engine runs on the simulator test PIN (`--sim-pin`): the "SIMULATOR TEST PIN" badge. */
export function simPinActive(m: MachineStatus | null): boolean {
  const a = obj(m?.approval);
  return a?.sim_pin === true;
}

/** What the approval card shows besides the PIN: the gates of the active device. */
export interface ApprovalGates {
  requireFraming: boolean;
  checklist: Checklist;
  door: InputPort | null;
}

export function approvalGates(p: DevicePrefs, io: IoPorts | null): ApprovalGates {
  return { requireFraming: p.require_framing, checklist: p.checklist, door: io?.door_protect ?? null };
}

/** A friendlier text for the engine's refusals of an approval that the PIN codes do not cover (409s), or null. */
export function approvalRefusalText(detail: string): string | null {
  const d = detail.toLowerCase();
  if (d.includes("require framing")) return "Require Framing is on: frame the job (Frame in the Laser window) since its last edit, on this device, then try again.";
  if (d.includes("job checklist")) return "Tick every item of the Job Checklist first.";
  if (d.includes("door interlock")) {
    const m = /input (\d+)/.exec(detail);
    return `Door Protect: the enclosure is open${m ? ` (input ${m[1]})` : ""}. Close it, then try again.`;
  }
  if (d.includes("another run is in progress")) return "Another run holds the machine. Wait for it to end (or STOP it), then try again.";
  if (d.includes("sim_pin_real_link") || d.includes("simulator test pin")) return "The simulator test PIN cannot approve runs on a real laser.";
  return null;
}

// ---- job log --------------------------------------------------------------------------------------------------------

export interface JobLogEntry {
  time_unix_s: number;
  run_id: string;
  state: string;
  origin: string;
  device: string;
  project: string | null;
  summary: string;
  duration_s: number;
  completed: number;
  error: string | null;
}

/** GET /api/job-log `entries`, newest first. */
export function parseJobLog(raw: unknown): JobLogEntry[] {
  const v = obj(raw);
  const list = Array.isArray(v?.entries) ? v.entries : Array.isArray(raw) ? raw : [];
  const rows = list.flatMap((e): JobLogEntry[] => {
    const o = obj(e);
    if (!o) return [];
    return [
      {
        time_unix_s: isNum(o.time_unix_s) ? o.time_unix_s : 0,
        run_id: str(o.run_id),
        state: str(o.state),
        origin: str(o.origin),
        device: str(o.device),
        project: typeof o.project === "string" ? o.project : null,
        summary: str(o.summary),
        duration_s: isNum(o.duration_s) ? o.duration_s : 0,
        completed: isNum(o.completed) ? o.completed : 0,
        error: typeof o.error === "string" ? o.error : null,
      },
    ];
  });
  return rows.sort((a, b) => b.time_unix_s - a.time_unix_s);
}

/** "You", "Script" or the agent's name. */
export function originText(origin: string): string {
  if (origin === "ui") return "You";
  if (origin === "script") return "Script";
  if (origin.startsWith("agent:")) return origin.slice(6) || "Agent";
  return origin || "?";
}

/** Totals over the rows shown: runs per state and the marking time. */
export function jobLogTotals(rows: readonly JobLogEntry[]): { runs: number; finished: number; failed: number; seconds: number } {
  return {
    runs: rows.length,
    finished: rows.filter((r) => r.state === "finished").length,
    failed: rows.filter((r) => r.state === "failed" || r.state === "aborted").length,
    seconds: rows.reduce((s, r) => s + r.duration_s, 0),
  };
}

// ---- updates --------------------------------------------------------------------------------------------------------

export interface UpdateCheck {
  current: string;
  latest: string | null;
  newer: boolean;
  url: string | null;
  error: string | null;
  checked_unix_s: number;
}

export function parseUpdateCheck(raw: unknown): UpdateCheck | null {
  const o = obj(raw);
  if (!o || typeof o.current !== "string") return null;
  return {
    current: o.current,
    latest: typeof o.latest === "string" ? o.latest : null,
    newer: o.newer === true,
    url: typeof o.url === "string" && /^https:\/\/github\.com\//.test(o.url) ? o.url : null,
    error: typeof o.error === "string" ? o.error : null,
    checked_unix_s: isNum(o.checked_unix_s) ? o.checked_unix_s : 0,
  };
}

export function updateText(u: UpdateCheck): string {
  if (u.error) return `The update check failed: ${u.error}`;
  if (u.newer && u.latest) return `OpenEtch ${u.latest} is available (you have ${u.current}).`;
  if (u.latest) return `OpenEtch ${u.current} is up to date (latest release ${u.latest}).`;
  return `OpenEtch ${u.current}: no release found.`;
}

// ---- Find My Laser --------------------------------------------------------------------------------------------------

export interface FoundBoard {
  vid: number;
  pid: number;
  location: string;
  serial: string | null;
  supported: boolean;
  note: string;
}

export function parseScan(raw: unknown): { usb: boolean; boards: FoundBoard[]; note: string | null } {
  const o = obj(raw) ?? {};
  const boards = (Array.isArray(o.boards) ? o.boards : []).flatMap((b): FoundBoard[] => {
    const x = obj(b);
    if (!x) return [];
    return [
      {
        vid: isNum(x.vid) ? x.vid : 0,
        pid: isNum(x.pid) ? x.pid : 0,
        location: str(x.location),
        serial: typeof x.serial === "string" && x.serial ? x.serial : null,
        supported: x.supported === true,
        note: str(x.note),
      },
    ];
  });
  return { usb: o.usb_support === true, boards, note: typeof o.note === "string" ? o.note : null };
}

export function hex4(n: number): string {
  return n.toString(16).padStart(4, "0").toUpperCase();
}

/** A device name from an EzCad config folder or file name. */
export function wizardDeviceName(markcfgName: string, corName: string | null): string {
  const strip = (s: string) => s.replace(/^.*[\\/]/, "").replace(/\.[^.]*$/, "");
  const cor = corName ? strip(corName) : "";
  const base = strip(markcfgName);
  return cor && !/^markcfg/i.test(cor) ? `Galvo ${cor}` : base && !/^markcfg/i.test(base) ? base : "Galvo";
}

// ---- start point (LightBurn's Set Start Point) ----------------------------------------------------------------------

/** The node nearest `p` within `tol` mm on a closed subpath (only closed outlines take a start point), or null. */
export function nearestStartNode(nl: NodeList, p: Vec2, tol: number): number | null {
  const closed = new Set(nl.subpaths.filter((s) => s.closed).map((s) => s.index));
  let best: { i: number; d: number } | null = null;
  for (const n of nl.nodes) {
    if (nl.subpaths.length && !closed.has(n.subpath)) continue;
    const d = Math.hypot(n.point[0] - p[0], n.point[1] - p[1]);
    if (d <= tol && (!best || d < best.d)) best = { i: n.index, d };
  }
  return best?.i ?? null;
}

export interface StartMark {
  id: string;
  /** Field mm. */
  at: Vec2;
  reverse: boolean;
}

/** Start points set on the document's objects (`object_options[id].start`), in field mm. */
export function startMarks(doc: GDocument | null): StartMark[] {
  const opts = obj((doc as { object_options?: unknown } | null)?.object_options);
  if (!doc || !opts) return [];
  const out: StartMark[] = [];
  for (const o of doc.objects) {
    const s = obj(obj(opts[o.id])?.start);
    const local = s?.local;
    if (!s || !Array.isArray(local) || !isNum(local[0]) || !isNum(local[1])) continue;
    const [a, b, c, d, e, f] = o.transform;
    out.push({ id: o.id, at: [a * local[0] + c * local[1] + e, b * local[0] + d * local[1] + f], reverse: s.reverse === true });
  }
  return out;
}

export function setStartPoint(id: string, node: number, reverse: boolean): { id: string; params: Record<string, unknown> } {
  return { id: "object.set_start_point", params: reverse ? { id, node, reverse } : { id, node } };
}

export function clearStartPoint(ids: readonly string[]): { id: string; params: Record<string, unknown> } {
  return { id: "object.clear_start_point", params: { ids: [...ids] } };
}

// ---- Print and Cut --------------------------------------------------------------------------------------------------

/** The engine refuses targets closer than this. */
export const MIN_TARGET_GAP_MM = 5;

/** Why the two targets cannot be aligned, or null. */
export function printCutProblem(targets: readonly (string | null)[], positions: readonly (Vec2 | null)[], centres: readonly (Vec2 | null)[]): string | null {
  if (!targets[0] || !targets[1]) return "Pick both targets.";
  if (targets[0] === targets[1]) return "The two targets must be different shapes.";
  if (!positions[0] || !positions[1]) return "Set both target positions with the red dot.";
  const [c0, c1] = centres;
  if (c0 && c1 && Math.hypot(c0[0] - c1[0], c0[1] - c1[1]) < MIN_TARGET_GAP_MM) return `The targets must be at least ${MIN_TARGET_GAP_MM} mm apart.`;
  if (Math.hypot(positions[0][0] - positions[1][0], positions[0][1] - positions[1][1]) < MIN_TARGET_GAP_MM) return `The two positions must be at least ${MIN_TARGET_GAP_MM} mm apart.`;
  return null;
}

export function alignToTargets(targets: [string, string], positions: [Vec2, Vec2], scale: boolean): { id: string; params: Record<string, unknown> } {
  return { id: "object.align_to_targets", params: { targets, positions, scale } };
}

// ---- Batch and Repeat Marking (POST /api/runs options) --------------------------------------------------------------

/** Most projects in one batch (engine). */
export const MAX_QUEUE = 50;

/** The run body with a batch of saved projects: one approval for all of them. */
export function batchBody(base: Record<string, unknown>, queue: readonly string[]): Record<string, unknown> {
  if (queue.length === 0) throw new Error("Add at least one saved project to the batch.");
  if (queue.length > MAX_QUEUE) throw new Error(`At most ${MAX_QUEUE} projects in one batch.`);
  const { selection: _s, from_segment: _f, ...rest } = base;
  return { ...rest, queue: [...queue] };
}

export interface TableOptions {
  /** mm (linear table) or degrees (rotary table) per iteration. */
  increment: number;
  settle_ms: number;
  return_to_start: boolean;
  repeat: number;
}

/** The axis roles that can carry Repeat Marking. */
export function tableRole(profile: Record<string, unknown> | null | undefined): "linear_table" | "rotary_table" | null {
  const r = obj(profile?.axis)?.role;
  return r === "linear_table" || r === "rotary_table" ? r : null;
}

/** The run body for Repeat Marking with the external axis table. */
export function tableBody(base: Record<string, unknown>, t: TableOptions): Record<string, unknown> {
  if (!isNum(t.increment) || t.increment === 0) throw new Error("Give the table a step per run (not 0).");
  if (!(Number.isInteger(t.repeat) && t.repeat >= 2 && t.repeat <= 10000)) throw new Error("Repeat Marking needs 2 to 10000 runs.");
  if (!(t.settle_ms >= 0 && t.settle_ms <= 60000)) throw new Error("Settle time is 0 to 60000 ms.");
  const { continuous: _c, ...rest } = base;
  return { ...rest, repeat: t.repeat, table: { increment: t.increment, settle_ms: Math.round(t.settle_ms), return_to_start: t.return_to_start } };
}

// ---- sound ----------------------------------------------------------------------------------------------------------

/** The notes of the job-finished chime: [frequency Hz, start s, length s]. */
export const CHIME: [number, number, number][] = [
  [880, 0, 0.18],
  [1175, 0.2, 0.18],
  [1568, 0.4, 0.32],
];
