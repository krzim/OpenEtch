// The window layout (LightBurn's dockable windows, Window menu and "Reset to Default Layout"): where each tool window
// (a `Dock`, by its id) sits — the left or right side panel, the bottom strip under the canvas, or floating over it —
// in which order, whether it is shown, and which windows share one frame as tabs. Kept per browser in localStorage
// (every access in try/catch; without storage the layout lasts for the visit) and changed only through the functions
// here, so `npm test` covers the rules. A tiny external store like prefs.ts; components read it with `useLayout()`
// (Dock.tsx).
//
// It also keeps the side panels' sizes (resized with the splitters) and whether each side is pinned or a rail of window
// titles that opens as an overlay (docs/ux/ui-plan-2026-11.md 1.1). `dockPolicy` decides what a side really is for a
// window width: the canvas column never goes below 640 px while a side can be railed instead (left first).

export type DockArea = "left" | "right" | "bottom" | "float";
export const DOCK_AREAS: DockArea[] = ["left", "right", "bottom", "float"];

export interface FloatRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface WindowPlace {
  area: DockArea;
  /** Sort key within the area (smaller first). */
  order: number;
  visible: boolean;
  /** Windows with the same group share a frame as tabs (the group id is its first member's window id). */
  group: string | null;
  /** Where the window floats (also kept while docked, so floating it again puts it back there). */
  float: FloatRect | null;
}

/** The side panels (the bottom strip has a size but no rail). */
export type SideArea = "left" | "right";
export const SIDE_AREAS: readonly SideArea[] = ["left", "right"];

/** A side panel docked beside the canvas, a rail of window titles that opens as an overlay, or hidden. */
export type AreaMode = "pinned" | "rail" | "hidden";
export const AREA_MODES: readonly AreaMode[] = ["pinned", "rail", "hidden"];

/** Widths of the side panels and the height of the bottom strip, CSS px. */
export interface DockSizes {
  left: number;
  right: number;
  bottom: number;
}

export const DEFAULT_SIZES: DockSizes = { left: 260, right: 320, bottom: 240 };
/** [min, max] per area (the splitters clamp to these). */
export const SIZE_LIMITS: Record<keyof DockSizes, readonly [number, number]> = { left: [200, 420], right: [260, 480], bottom: [120, 600] };

export interface Layout {
  windows: Record<string, WindowPlace>;
  /** The tab shown in each group. */
  activeTab: Record<string, string>;
  sizes: DockSizes;
  /** The user's choice per side; a side not listed follows the layout mode (`dockPolicy`). */
  areaMode: Partial<Record<SideArea, AreaMode>>;
}

/** The known tool windows in their default place (any other Dock id starts on the right, after these). */
export interface WindowInfo {
  id: string;
  area: DockArea;
  /** Message key of its title in the Window menu. */
  titleKey: string;
}

export const WINDOWS: WindowInfo[] = [
  { id: "browser", area: "left", titleKey: "win.browser" },
  { id: "laser", area: "right", titleKey: "win.laser" },
  { id: "pages", area: "right", titleKey: "win.pages" },
  { id: "cuts", area: "right", titleKey: "win.cuts" },
  { id: "library", area: "right", titleKey: "win.library" },
  { id: "artLibrary", area: "right", titleKey: "win.artLibrary" },
  { id: "varText", area: "right", titleKey: "win.variableText" },
  { id: "docking", area: "right", titleKey: "win.docking" },
  { id: "properties", area: "right", titleKey: "win.properties" },
  { id: "machine", area: "right", titleKey: "win.machine" },
];

export const LAYOUT_KEY = "openetch.layout";

const DEFAULT_FLOAT: FloatRect = { x: 120, y: 120, w: 320, h: 360 };

function defaultPlace(id: string): WindowPlace {
  const i = WINDOWS.findIndex((w) => w.id === id);
  return { area: i >= 0 ? WINDOWS[i].area : "right", order: i >= 0 ? i : WINDOWS.length + 100, visible: true, group: null, float: null };
}

/** A window's place: stored, else its default. */
export function placeOf(l: Layout, id: string): WindowPlace {
  return l.windows[id] ?? defaultPlace(id);
}

export const DEFAULT_LAYOUT: Layout = { windows: {}, activeTab: {}, sizes: DEFAULT_SIZES, areaMode: {} };

/** A size clamped to its area's limits (anything but a finite number gives the default). */
export function clampSize(area: keyof DockSizes, v: unknown): number {
  const [lo, hi] = SIZE_LIMITS[area];
  if (typeof v !== "number" || !Number.isFinite(v)) return DEFAULT_SIZES[area];
  return Math.round(Math.min(hi, Math.max(lo, v)));
}

const finite = (v: unknown, lo: number, hi: number): number | null => (typeof v === "number" && Number.isFinite(v) && v >= lo && v <= hi ? v : null);
const ID = /^[A-Za-z][\w.-]{0,63}$/;

/** A valid layout from anything (stored JSON from another build, or garbage); unusable entries are dropped. */
export function sanitizeLayout(raw: unknown): Layout {
  const r = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const windows: Record<string, WindowPlace> = {};
  const ws = r.windows && typeof r.windows === "object" ? (r.windows as Record<string, unknown>) : {};
  for (const [id, v] of Object.entries(ws).slice(0, 64)) {
    if (!ID.test(id) || !v || typeof v !== "object") continue;
    const o = v as Record<string, unknown>;
    const area = DOCK_AREAS.includes(o.area as DockArea) ? (o.area as DockArea) : defaultPlace(id).area;
    const f = o.float && typeof o.float === "object" ? (o.float as Record<string, unknown>) : null;
    const rect =
      f && finite(f.x, -5000, 20000) !== null && finite(f.y, -5000, 20000) !== null && finite(f.w, 120, 5000) !== null && finite(f.h, 80, 5000) !== null
        ? { x: f.x as number, y: f.y as number, w: f.w as number, h: f.h as number }
        : null;
    windows[id] = {
      area,
      order: finite(o.order, -1e6, 1e6) ?? defaultPlace(id).order,
      visible: typeof o.visible === "boolean" ? o.visible : true,
      group: typeof o.group === "string" && ID.test(o.group) ? o.group : null,
      float: rect,
    };
  }
  const activeTab: Record<string, string> = {};
  const at = r.activeTab && typeof r.activeTab === "object" ? (r.activeTab as Record<string, unknown>) : {};
  for (const [g, id] of Object.entries(at)) if (ID.test(g) && typeof id === "string" && ID.test(id)) activeTab[g] = id;
  const sz = r.sizes && typeof r.sizes === "object" ? (r.sizes as Record<string, unknown>) : {};
  const sizes: DockSizes = { left: clampSize("left", sz.left), right: clampSize("right", sz.right), bottom: clampSize("bottom", sz.bottom) };
  const am = r.areaMode && typeof r.areaMode === "object" ? (r.areaMode as Record<string, unknown>) : {};
  const areaMode: Partial<Record<SideArea, AreaMode>> = {};
  for (const side of SIDE_AREAS) if (AREA_MODES.includes(am[side] as AreaMode)) areaMode[side] = am[side] as AreaMode;
  return normalizeGroups({ windows, activeTab, sizes, areaMode });
}

/** Groups only hold windows in one area; a group with a single member is dissolved. */
function normalizeGroups(l: Layout): Layout {
  const windows = { ...l.windows };
  const members = new Map<string, string[]>();
  for (const [id, w] of Object.entries(windows)) if (w.group) members.set(w.group, [...(members.get(w.group) ?? []), id]);
  for (const [g, ids] of members) {
    const area = windows[ids[0]].area;
    const same = ids.filter((id) => windows[id].area === area);
    for (const id of ids) if (!same.includes(id) || same.length < 2) windows[id] = { ...windows[id], group: null };
    void g;
  }
  const activeTab: Record<string, string> = {};
  for (const [g, id] of Object.entries(l.activeTab)) if (windows[id]?.group === g) activeTab[g] = id;
  return { ...l, windows, activeTab };
}

// ---- changes ------------------------------------------------------------------------------------------------------------

function withPlace(l: Layout, id: string, patch: Partial<WindowPlace>): Layout {
  return normalizeGroups({ ...l, windows: { ...l.windows, [id]: { ...placeOf(l, id), ...patch } } });
}

/** The last order in `area`, plus one. */
function nextOrder(l: Layout, area: DockArea, ids: readonly string[]): number {
  const orders = ids.map((id) => placeOf(l, id)).filter((w) => w.area === area).map((w) => w.order);
  return orders.length ? Math.max(...orders) + 1 : 0;
}

/** Dock window `id` in `area` (at the end), out of any tab group. `known` lists the window ids in use. */
export function dockTo(l: Layout, id: string, area: DockArea, known: readonly string[]): Layout {
  const cur = placeOf(l, id);
  const float = area === "float" ? (cur.float ?? floatSlot(l, known)) : cur.float;
  return withPlace(l, id, { area, order: nextOrder(l, area, known.filter((k) => k !== id)), group: null, visible: true, float });
}

/** A float position that does not sit exactly on another floating window. */
function floatSlot(l: Layout, known: readonly string[]): FloatRect {
  const taken = known.map((k) => placeOf(l, k)).filter((w) => w.area === "float" && w.float).length;
  return { ...DEFAULT_FLOAT, x: DEFAULT_FLOAT.x + 28 * taken, y: DEFAULT_FLOAT.y + 28 * taken };
}

/** Put window `id` into `target`'s frame as a tab (same area and place), and show it. */
export function tabWith(l: Layout, id: string, target: string): Layout {
  if (id === target) return l;
  const t = placeOf(l, target);
  const group = t.group ?? target;
  // Both places change before the groups are checked (a group of one would be dissolved on the way).
  const windows = { ...l.windows, [target]: { ...t, group }, [id]: { ...placeOf(l, id), area: t.area, order: t.order, group, visible: true } };
  return normalizeGroups({ ...l, windows, activeTab: { ...l.activeTab, [group]: id } });
}

/** Take window `id` out of its tab group (it stays in the same area, right after the group). */
export function untab(l: Layout, id: string): Layout {
  const w = placeOf(l, id);
  return w.group ? withPlace(l, id, { group: null, order: w.order + 0.5 }) : l;
}

export function setVisible(l: Layout, id: string, visible: boolean): Layout {
  return withPlace(l, id, { visible });
}

export function setActiveTab(l: Layout, group: string, id: string): Layout {
  return { ...l, activeTab: { ...l.activeTab, [group]: id } };
}

export function setFloatRect(l: Layout, id: string, r: FloatRect): Layout {
  return withPlace(l, id, { float: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(Math.max(160, r.w)), h: Math.round(Math.max(100, r.h)) } });
}

/** Move window `id` one place up (-1) or down (+1) among the units of its area (a tab group moves as one). */
export function moveInArea(l: Layout, id: string, dir: -1 | 1, known: readonly string[]): Layout {
  const units = areaUnits(l, placeOf(l, id).area, known);
  const i = units.findIndex((u) => u.members.includes(id));
  const j = i + dir;
  if (i < 0 || j < 0 || j >= units.length) return l;
  const swapped = [...units];
  [swapped[i], swapped[j]] = [swapped[j], swapped[i]];
  let next = l;
  swapped.forEach((u, k) => {
    for (const m of u.members) next = withPlace(next, m, { order: k });
  });
  return next;
}

/** A frame in an area: one window, or a tab group (`members` in tab order, `active` the one shown). */
export interface AreaUnit {
  key: string;
  members: string[];
  active: string;
}

/** The frames of `area` in order, from the windows in `ids` that are shown. */
export function areaUnits(l: Layout, area: DockArea, ids: readonly string[]): AreaUnit[] {
  const placed = ids
    .map((id) => ({ id, w: placeOf(l, id) }))
    .filter((x) => x.w.area === area && x.w.visible)
    .sort((a, b) => a.w.order - b.w.order || ids.indexOf(a.id) - ids.indexOf(b.id));
  const units: AreaUnit[] = [];
  const byGroup = new Map<string, AreaUnit>();
  for (const { id, w } of placed) {
    if (w.group) {
      const u = byGroup.get(w.group);
      if (u) {
        u.members.push(id);
        continue;
      }
      const nu = { key: `g:${w.group}`, members: [id], active: id };
      byGroup.set(w.group, nu);
      units.push(nu);
    } else units.push({ key: id, members: [id], active: id });
  }
  for (const [g, u] of byGroup) {
    const want = l.activeTab[g];
    u.active = want && u.members.includes(want) ? want : u.members[0];
  }
  return units;
}

// ---- sizes, rails and presets -------------------------------------------------------------------------------------------

/** Set a side panel's width or the bottom strip's height (clamped). */
export function setDockSize(l: Layout, area: keyof DockSizes, px: number): Layout {
  return { ...l, sizes: { ...l.sizes, [area]: clampSize(area, px) } };
}

/** Pin a side, make it a rail, hide it, or (null) let the layout mode decide again. */
export function setAreaMode(l: Layout, side: SideArea, mode: AreaMode | null): Layout {
  const areaMode = { ...l.areaMode };
  if (mode === null) delete areaMode[side];
  else areaMode[side] = mode;
  return { ...l, areaMode };
}

/** The layout modes dockPolicy knows (layoutMode.ts `LayoutMode`, repeated so this file stays free of React). */
export type DockLayoutMode = "wide" | "compact" | "tablet" | "phone";

/** What a side is when the user has not chosen: wide pins both, compact rails the Browser (owner decision 9), a tablet
 * rails both, and a phone has no side panels (they are sheets there). */
export function defaultAreaMode(layout: DockLayoutMode, side: SideArea): AreaMode {
  if (layout === "phone") return "hidden";
  if (layout === "tablet") return "rail";
  if (layout === "compact") return side === "left" ? "rail" : "pinned";
  return "pinned";
}

export interface DockPolicyOptions {
  layout?: DockLayoutMode;
  /** The user's choices (Layout.areaMode). */
  modes?: Partial<Record<SideArea, AreaMode>>;
  /** Width of a rail (--dock-rail-w). */
  railW?: number;
  /** Width taken by other chrome in the row (the tablet's tool rail). */
  reserved?: number;
  /** The narrowest the canvas column may get before a side is railed (--canvas-min-w). */
  minCanvas?: number;
}

/**
 * What each side really is at window width `viewW`: the user's choice or the layout's default; then, while the canvas
 * column would be narrower than `minCanvas` (640 px), pinned sides become rails, the left first, then the right. A side
 * the user pinned explicitly goes last.
 */
export function dockPolicy(viewW: number, sizes: DockSizes, o: DockPolicyOptions = {}): Record<SideArea, AreaMode> {
  const layout = o.layout ?? "wide";
  const railW = o.railW ?? (layout === "tablet" ? 40 : 32);
  const minCanvas = o.minCanvas ?? 640;
  const reserved = o.reserved ?? 0;
  if (layout === "phone") return { left: "hidden", right: "hidden" };
  const out: Record<SideArea, AreaMode> = {
    left: o.modes?.left ?? defaultAreaMode(layout, "left"),
    right: o.modes?.right ?? defaultAreaMode(layout, "right"),
  };
  const width = (side: SideArea) => (out[side] === "pinned" ? clampSize(side, sizes[side]) : out[side] === "rail" ? railW : 0);
  const canvas = () => viewW - reserved - width("left") - width("right");
  const order = [...SIDE_AREAS].sort((a, b) => Number(o.modes?.[a] === "pinned") - Number(o.modes?.[b] === "pinned"));
  for (const side of order) {
    if (canvas() >= minCanvas) break;
    if (out[side] === "pinned") out[side] = "rail";
  }
  return out;
}

/** The Window menu's layout presets. */
export type LayoutPreset = "wide" | "compact" | "touch";
export const LAYOUT_PRESETS: readonly LayoutPreset[] = ["wide", "compact", "touch"];

/** Tab groups of the compact and touch presets; the first member is the tab shown (V12, L10). */
export const PRESET_GROUPS: readonly (readonly string[])[] = [
  ["cuts", "properties"],
  ["library", "artLibrary", "varText", "docking"],
  ["laser", "machine"],
];

/**
 * Window > Layout: Wide (the default layout, both sides pinned), Compact (fewer frames: {Cuts/Layers, Properties},
 * {Library, Art, Variable text, Docking} and, in the Laser workspace, {Laser, Machine} as tab groups, the Browser on a
 * rail) or Touch (the same groups, both sides on rails). Sizes are kept.
 */
export function applyPreset(l: Layout, preset: LayoutPreset): Layout {
  const base: Layout = { ...DEFAULT_LAYOUT, sizes: l.sizes };
  if (preset === "wide") return { ...base, areaMode: { left: "pinned", right: "pinned" } };
  let next = base;
  for (const [first, ...rest] of PRESET_GROUPS) {
    for (const id of rest) next = tabWith(next, id, first);
    const group = placeOf(next, first).group;
    if (group) next = setActiveTab(next, group, first);
  }
  return { ...next, areaMode: preset === "compact" ? { left: "rail", right: "pinned" } : { left: "rail", right: "rail" } };
}

// ---- the store ----------------------------------------------------------------------------------------------------------

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadLayout(): Layout {
  try {
    const raw = storage()?.getItem(LAYOUT_KEY);
    return sanitizeLayout(raw ? JSON.parse(raw) : null);
  } catch {
    return DEFAULT_LAYOUT;
  }
}

let current: Layout = loadLayout();
const listeners = new Set<() => void>();

export function getLayout(): Layout {
  return current;
}

export function updateLayout(fn: (l: Layout) => Layout): void {
  current = sanitizeLayout(fn(current));
  try {
    storage()?.setItem(LAYOUT_KEY, JSON.stringify(current));
  } catch {
    // Storage unavailable: the layout lasts for this visit.
  }
  for (const l of listeners) l();
}

/** LightBurn's Window > Reset to Default Layout. */
export function resetLayout(): void {
  updateLayout(() => DEFAULT_LAYOUT);
}

export function subscribeLayout(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Another tab changed the stored layout: adopt it. */
export function reloadLayout(): void {
  current = loadLayout();
  for (const l of listeners) l();
}
