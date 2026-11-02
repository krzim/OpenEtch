// Pure helpers of the Tools menu extras (src/tools/): hand-placed tab marks and the tabs.* invocations, dogbone and
// Resize Slots parameters, Multi-File Trace parameters, and the User Tools model (LightBurn's Web Tools menu) with
// the same checks the engine makes when they are saved (`crate::user_tools`). No React, no fetch: tested in
// tools.test.ts.

import type { Invocation } from "../api/client.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import { traceParams, type TraceOptions } from "../files/fileFormats.ts";
import { TOL, pointerClass } from "../pointer.ts";

export const TOOLS_CMD = {
  tabsAdd: "tabs.add",
  tabsMove: "tabs.move",
  tabsRemove: "tabs.remove",
  tabsClear: "tabs.clear",
  dogbone: "path.dogbone",
  resizeSlots: "path.resize_slots",
  traceMany: "image.trace_many",
  traceFiles: "image.trace_files",
  importSvg: "art.import_svg",
} as const;

function obj(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

// ---- hand-placed tabs -----------------------------------------------------------------------------------------------

export interface TabMark {
  id: string;
  /** Index in the object's list (tabs.move / tabs.remove use it). */
  index: number;
  /** Field mm. */
  at: Vec2;
}

/** The tabs placed by hand on the document's objects (`object_options[id].tabs`, local mm), in field mm. */
export function tabMarks(doc: GDocument | null): TabMark[] {
  const opts = obj((doc as { object_options?: unknown } | null)?.object_options);
  if (!doc || !opts) return [];
  const out: TabMark[] = [];
  for (const o of doc.objects) {
    const tabs = obj(opts[o.id])?.tabs;
    if (!Array.isArray(tabs)) continue;
    const [a, b, c, d, e, f] = o.transform;
    tabs.forEach((t, index) => {
      if (!Array.isArray(t) || !isNum(t[0]) || !isNum(t[1])) return;
      out.push({ id: o.id, index, at: [a * t[0] + c * t[1] + e, b * t[0] + d * t[1] + f] });
    });
  }
  return out;
}

/** How near (screen px) a press must be to a tab mark to pick it: 8 px for a mouse, a finger's or a pen's node reach
 *  (pointer.ts TOL) for a touch or a pen. */
export function tabHitPx(pointerType: string | undefined): number {
  return Math.max(8, TOL[pointerClass({ pointerType })].node);
}

/** The tab mark nearest `p` within `tolMm`, or null. */
export function nearestTab(marks: readonly TabMark[], p: Vec2, tolMm: number): TabMark | null {
  let best: { m: TabMark; d: number } | null = null;
  for (const m of marks) {
    const d = Math.hypot(m.at[0] - p[0], m.at[1] - p[1]);
    if (d <= tolMm && (!best || d < best.d)) best = { m, d };
  }
  return best?.m ?? null;
}

const r4 = (v: number) => Math.round(v * 1e4) / 1e4;

export function addTab(id: string, at: Vec2, maxDistanceMm: number): Invocation {
  return { id: TOOLS_CMD.tabsAdd, params: { id, points: [[r4(at[0]), r4(at[1])]], max_distance_mm: r4(maxDistanceMm) } };
}

export function moveTab(m: TabMark, to: Vec2, toId: string | null): Invocation {
  const params: Record<string, unknown> = { id: m.id, index: m.index, to: [r4(to[0]), r4(to[1])] };
  if (toId && toId !== m.id) params.to_id = toId;
  return { id: TOOLS_CMD.tabsMove, params };
}

export function removeTab(m: TabMark): Invocation {
  return { id: TOOLS_CMD.tabsRemove, params: { ids: [m.id], near: m.at, within_mm: 0.001 } };
}

export function removeTabs(ids: readonly string[]): Invocation {
  return { id: TOOLS_CMD.tabsRemove, params: { ids: [...ids] } };
}

export function clearTabs(layer: string): Invocation {
  return { id: TOOLS_CMD.tabsClear, params: { layer } };
}

// ---- dogbone and Resize Slots ---------------------------------------------------------------------------------------

export type DogboneStyle = "dogbone" | "tbone_longer" | "tbone_shorter";

export function dogboneProblem(diameter: number | null): "dog.badDiameter" | null {
  return diameter !== null && diameter > 0 && diameter <= 100 ? null : "dog.badDiameter";
}

export function dogboneInvocation(ids: readonly string[], diameter: number, style: DogboneStyle): Invocation {
  const params: Record<string, unknown> = { ids: [...ids], diameter_mm: diameter };
  if (style !== "dogbone") params.style = style;
  return { id: TOOLS_CMD.dogbone, params };
}

export interface SlotForm {
  oldMm: number | null;
  newMm: number | null;
  toleranceMm: number | null;
  depth: boolean;
  width: boolean;
  tabHeight: boolean;
}

export const DEFAULT_SLOTS: SlotForm = { oldMm: 3, newMm: 3, toleranceMm: 0.1, depth: true, width: false, tabHeight: false };

/** A message key for what is wrong with the form, or null. */
export function slotsProblem(f: SlotForm): "slots.badThickness" | "slots.none" | null {
  const { oldMm, newMm, toleranceMm } = f;
  if (oldMm === null || newMm === null || toleranceMm === null || !(oldMm > 0) || !(newMm > 0) || !(toleranceMm >= 0) || toleranceMm >= oldMm) return "slots.badThickness";
  if (!f.depth && !f.width && !f.tabHeight) return "slots.none";
  return null;
}

export function slotsInvocation(ids: readonly string[], f: SlotForm): Invocation {
  return {
    id: TOOLS_CMD.resizeSlots,
    params: {
      ids: [...ids],
      old_thickness_mm: f.oldMm,
      new_thickness_mm: f.newMm,
      tolerance_mm: f.toleranceMm,
      adjust_slot_depth: f.depth,
      adjust_slot_width: f.width,
      adjust_tab_height: f.tabHeight,
    },
  };
}

// ---- Multi-File Trace -----------------------------------------------------------------------------------------------

/** image.trace's settings of a Trace Image form, without the id, boundary and delete flag. */
export function traceSettings(o: TraceOptions): Record<string, unknown> {
  const p = traceParams("", { ...o, boundary: null, deleteImage: false }, true);
  delete p.id;
  return p;
}

export function traceManyInvocation(ids: readonly string[], o: TraceOptions, deleteImages: boolean, layer: string | null): Invocation {
  const params: Record<string, unknown> = { ids: [...ids], settings: traceSettings(o) };
  if (deleteImages) params.delete_images = true;
  if (layer) params.layer = layer;
  return { id: TOOLS_CMD.traceMany, params };
}

export function traceFilesInvocation(files: readonly { name: string; data_base64: string }[], o: TraceOptions, dpi: number | null, zip: boolean): Invocation {
  const params: Record<string, unknown> = { files: files.map((f) => ({ name: f.name, data_base64: f.data_base64 })), settings: traceSettings(o) };
  if (dpi !== null) params.dpi = dpi;
  if (zip) params.zip = true;
  return { id: TOOLS_CMD.traceFiles, params };
}

export interface TracedFile {
  name: string;
  svgName: string | null;
  svg: string | null;
  error: string | null;
}

/** image.trace_files' answer: each file, and the zip when asked for. */
export function parseTraceFiles(raw: unknown): { files: TracedFile[]; zip: string | null; zipName: string } {
  const r = obj(raw) ?? {};
  const files = (Array.isArray(r.files) ? r.files : []).map((f): TracedFile => {
    const o = obj(f) ?? {};
    return {
      name: typeof o.name === "string" ? o.name : "",
      svgName: typeof o.svg_name === "string" ? o.svg_name : null,
      svg: typeof o.svg === "string" ? o.svg : null,
      error: typeof o.error === "string" ? o.error : null,
    };
  });
  return { files, zip: typeof r.zip_base64 === "string" ? r.zip_base64 : null, zipName: typeof r.zip_name === "string" ? r.zip_name : "traced.zip" };
}

/** art.import_svg of every traced file, one object each, named after its file. */
export function importTracedSvgs(files: readonly TracedFile[], layer: string | null): Invocation[] {
  return files
    .filter((f) => f.svg)
    .map((f) => {
      const params: Record<string, unknown> = { svg: f.svg, name: (f.svgName ?? f.name).replace(/\.svg$/i, "") || "Trace" };
      if (layer) params.layer = layer;
      return { id: TOOLS_CMD.importSvg, params };
    });
}

// ---- User Tools (LightBurn's Web Tools menu) ------------------------------------------------------------------------

export interface ToolStep {
  id: string;
  params: Record<string, unknown>;
}

export type UserTool =
  | { name: string; category: string; kind: "url"; url: string }
  | { name: string; category: string; kind: "commands"; steps: ToolStep[] };

export const MAX_TOOLS = 50;
export const MAX_NAME_CHARS = 40;
export const MAX_STEPS = 50;

/** The `user_tools` of GET /api/prefs (entries that do not parse are dropped). */
export function parseUserTools(raw: unknown): UserTool[] {
  const list = obj(raw)?.user_tools;
  if (!Array.isArray(list)) return [];
  const out: UserTool[] = [];
  for (const v of list) {
    const o = obj(v);
    if (!o || typeof o.name !== "string") continue;
    const category = typeof o.category === "string" ? o.category : "";
    if (o.kind === "url" && typeof o.url === "string") out.push({ name: o.name, category, kind: "url", url: o.url });
    else if (o.kind === "commands" && Array.isArray(o.steps)) {
      const steps = o.steps.map(obj).filter((s): s is Record<string, unknown> => !!s && typeof s.id === "string").map((s) => ({ id: s.id as string, params: obj(s.params) ?? {} }));
      out.push({ name: o.name, category, kind: "commands", steps });
    }
  }
  return out;
}

/** What PUT /api/prefs takes (category left out when empty). */
export function userToolsBody(tools: readonly UserTool[]): Record<string, unknown>[] {
  return tools.map((t) => {
    const base: Record<string, unknown> = { name: t.name.trim(), ...(t.category.trim() ? { category: t.category.trim() } : {}) };
    return t.kind === "url" ? { ...base, kind: "url", url: t.url.trim() } : { ...base, kind: "commands", steps: t.steps.map((s) => ({ id: s.id, params: s.params })) };
  });
}

/** Whether `url` is an http(s) address the menu may open (the engine's rule). */
export function isWebUrl(url: string): boolean {
  const u = url.trim();
  if (!u || /\s/.test(u)) return false;
  const m = /^https?:\/\/(.*)$/i.exec(u);
  return !!m && m[1].length > 0 && !m[1].startsWith("/");
}

/** Command steps from the editor's JSON text: the steps, or why not. */
export function parseSteps(text: string): { steps: ToolStep[] } | { error: string } {
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch (e) {
    return { error: `Not JSON: ${e instanceof Error ? e.message : String(e)}` };
  }
  if (!Array.isArray(v) || v.length === 0 || v.length > MAX_STEPS) return { error: `A list of 1 to ${MAX_STEPS} commands.` };
  const steps: ToolStep[] = [];
  for (const s of v) {
    const o = obj(s);
    if (!o || typeof o.id !== "string" || !o.id.trim()) return { error: "Each command needs an \"id\"." };
    if (o.params !== undefined && !obj(o.params)) return { error: `The params of "${o.id}" must be an object.` };
    steps.push({ id: o.id.trim(), params: obj(o.params) ?? {} });
  }
  return { steps };
}

/** Why a tool cannot be saved (the engine checks the command ids and their risk too), or null. */
export function userToolProblem(t: UserTool): string | null {
  const name = t.name.trim();
  if (!name || [...t.name].length > MAX_NAME_CHARS) return `The name needs 1 to ${MAX_NAME_CHARS} characters.`;
  if ([...t.category].length > MAX_NAME_CHARS) return `The category has at most ${MAX_NAME_CHARS} characters.`;
  if (t.kind === "url") return isWebUrl(t.url) ? null : "The URL must start with http:// or https://.";
  return t.steps.length > 0 && t.steps.length <= MAX_STEPS ? null : `A sequence has 1 to ${MAX_STEPS} commands.`;
}

/** The menu: top-level tools first, then one submenu per category (in first-seen order). */
export function groupUserTools(tools: readonly UserTool[]): { category: string; tools: UserTool[] }[] {
  const out: { category: string; tools: UserTool[] }[] = [];
  for (const t of tools) {
    const c = t.category.trim();
    const g = out.find((x) => x.category === c);
    if (g) g.tools.push(t);
    else out.push({ category: c, tools: [t] });
  }
  return out.sort((a, b) => (a.category === "" ? -1 : b.category === "" ? 1 : 0));
}

/** The address a web-page tool opens, or null when it is not an http(s) address (a preferences.json edited by hand
 * is not checked by the engine, so the menu checks again before opening anything). */
export function userToolUrl(t: UserTool): string | null {
  return t.kind === "url" && isWebUrl(t.url) ? t.url.trim() : null;
}

/** The invocations a command-sequence tool runs (one batch). */
export function userToolInvocations(t: UserTool): Invocation[] {
  return t.kind === "commands" ? t.steps.map((s) => ({ id: s.id, params: s.params })) : [];
}
