// OpenEtch devices on the web side, LightBurn-style (ARCHITECTURE.md, "Devices"): the engine's device registry
// (GET /api/devices, GET /api/devices/{id}), what the Devices window and the Laser workspace's device dropdown show,
// when switching or removing a device is allowed, the canvas workspace of the active device, and the import body cut
// out of a LightBurn prefs.ini or .lbdev file. No React and no fetch, so `npm test` covers it.
//
// A device is a LightBurn device (or the built-in B6) with a workspace (W x H mm) and an origin. Only a JCZ/LMC galvo
// ("jcz_galvo") can be driven by OpenEtch; every other kind is design-only: its workspace sets the canvas and the
// LightBurn import/export, and the Laser workspace says why it cannot run.
//
// Privacy: prefs.ini holds unrelated settings and secrets, and .lbdev files hold local paths, macros and network
// addresses. `deviceImportBody` keeps only the typed device fields below (the correction file and library path cut
// to their file names), so nothing else leaves this browser. The engine parses the result with the same whitelist.

import { DEVICE_SETTINGS_KEYS, type LightBurnDevice, type ProfileDiff } from "./lightburn.ts";
import { ACTIVE_STATES } from "./runs.ts";
import type { MachineStatus, Vec2 } from "./types.ts";
import { tr } from "../workspace/tr.ts";
import { DESIGN_ONLY } from "../mode.ts";

/**
 * What kind of controller a device is. OpenEtch drives `jcz_galvo`, `grbl` (a GRBL 1.1 gantry made in OpenEtch) and
 * `ruida` (a Ruida DSP gantry made in OpenEtch; both unverified on hardware); `gantry` is a LightBurn gantry kept
 * design-only.
 */
export type DeviceKind = "jcz_galvo" | "grbl" | "ruida" | "gantry" | "galvo_other" | "unknown";

/** Where a device came from. */
export type DeviceSource = "builtin" | "lightburn-prefs" | "lbdev" | "manual";

/** The workspace origin: the field centre for galvos, a corner for gantry machines. */
export type DeviceOrigin = "center" | "bottom_left" | "top_left" | "bottom_right" | "top_right";

/** One row of GET /api/devices. */
export interface DeviceSummary {
  id: string;
  /** LightBurn's controller name ("JCZFiber", "GRBL", "Custom GCode"), or OpenEtch's name for the built-in device. */
  name: string;
  /** The user's name for the device (LightBurn's DisplayName); `.lbrn2` files carry it as `DeviceName`. */
  display_name: string;
  source: DeviceSource | string;
  kind: DeviceKind;
  /** LightBurn's connection type as stored ("LibUSB", "Serial", "TCP"), or null. */
  lightburn_type: string | null;
  /** Workspace width and height in mm (a galvo's field is square). */
  workspace_mm: Vec2;
  /**
   * Galvos: the field LightBurn files are laid out for (LightBurn's Width, e.g. 153.8 on the B6 whose profile field is
   * EzCad's 153.75); the engine imports and exports `.lbrn2` files with it. Null for gantries or when not sent.
   */
  lightburn_field_mm: number | null;
  origin: DeviceOrigin | string;
  /** False for devices OpenEtch cannot drive (design-only). */
  controllable: boolean;
}

/** GET /api/devices. */
export interface DeviceList {
  /** Id of the active device, or null. */
  active: string | null;
  /** In the engine's order. */
  devices: DeviceSummary[];
}

/** One read-only setting of GET /api/devices/{id}, grouped like LightBurn's Device Settings tabs. */
export interface DeviceSetting {
  group: string;
  label: string;
  value: unknown;
}

/** GET /api/devices/{id}: the summary plus the profile OpenEtch drives it with and how that differs from its base. */
export interface DeviceDetails extends DeviceSummary {
  /** Profile summary (name, field, limits...) as label/value pairs; empty for design-only devices. */
  profile: { label: string; value: unknown }[];
  /** The base profile the device's profile was built over ("ComMarker B6 MOPA 60W", "generic JCZ galvo"), or null. */
  base: string | null;
  diffs: ProfileDiff[];
  settings: DeviceSetting[];
}

/** POST /api/devices/import: what was added, updated (same name, type and size) and skipped (with the reason). */
export interface ImportResult {
  added: string[];
  updated: string[];
  skipped: { name: string; reason: string }[];
}

// ---- parsing the engine's answers -----------------------------------------------------------------------------------

function obj(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function str(v: unknown): string | null {
  return typeof v === "string" && v.trim() !== "" ? v : null;
}

function pos(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) && v > 0 ? v : null;
}

const KINDS: readonly DeviceKind[] = ["jcz_galvo", "grbl", "ruida", "gantry", "galvo_other", "unknown"];

/** Whether a device kind is a galvo (square field, centre origin). */
export function isGalvo(kind: DeviceKind | string): boolean {
  return kind === "jcz_galvo" || kind === "galvo_other";
}

/** A workspace from `[w, h]` or `{width_mm, height_mm}` / `field_mm`, or null. */
function workspaceOf(o: Record<string, unknown>): Vec2 | null {
  const ws = o.workspace_mm;
  if (Array.isArray(ws) && ws.length === 2) {
    const w = pos(ws[0]);
    const h = pos(ws[1]);
    if (w !== null && h !== null) return [w, h];
  }
  const w = pos(o.width_mm) ?? pos(o.field_mm);
  const h = pos(o.height_mm) ?? w;
  return w !== null && h !== null ? [w, h] : null;
}

/** One device of an engine answer, or null when it lacks an id or a usable workspace. */
export function deviceSummary(raw: unknown): DeviceSummary | null {
  const o = obj(raw);
  if (!o) return null;
  const id = str(o.id);
  const workspace = workspaceOf(o);
  if (id === null || workspace === null) return null;
  const name = str(o.name) ?? str(o.display_name) ?? id;
  const kind = KINDS.includes(o.kind as DeviceKind) ? (o.kind as DeviceKind) : "unknown";
  return {
    id,
    name,
    display_name: str(o.display_name) ?? name,
    source: str(o.source) ?? "manual",
    kind,
    lightburn_type: str(o.lightburn_type),
    workspace_mm: workspace,
    lightburn_field_mm: isGalvo(kind) ? pos(o.lightburn_field_mm) : null,
    origin: str(o.origin) ?? (isGalvo(kind) ? "center" : "bottom_left"),
    // Only what the engine says it can drive; a missing flag is design-only.
    controllable: o.controllable === true,
  };
}

/** GET /api/devices as a `DeviceList`; malformed rows are dropped, and an unknown `active` becomes null. */
export function deviceList(res: unknown): DeviceList {
  const r = obj(res) ?? {};
  const devices: DeviceSummary[] = [];
  if (Array.isArray(r.devices)) {
    for (const d of r.devices) {
      const s = deviceSummary(d);
      if (s && !devices.some((x) => x.id === s.id)) devices.push(s);
    }
  }
  const active = str(r.active);
  return { active: active !== null && devices.some((d) => d.id === active) ? active : null, devices };
}

/** A scalar or a list of scalars (a range such as `[20, 4000]`). */
function shown(v: unknown): boolean {
  return v === null || typeof v !== "object" || (Array.isArray(v) && v.every((e) => e === null || typeof e !== "object"));
}

/**
 * Label/value pairs from an object (`{field_mm: 153.8, frequency_khz: [20, 4000], timing: {jump_delay_us: 100}}`;
 * scalars and lists of scalars, nested objects one level deep as "timing.jump_delay_us") or a list of
 * `{label|key|field, value}`.
 */
function pairs(v: unknown): { label: string; value: unknown }[] {
  const out: { label: string; value: unknown }[] = [];
  if (Array.isArray(v)) {
    for (const e of v) {
      const o = obj(e);
      const label = o && (str(o.label) ?? str(o.key) ?? str(o.field) ?? str(o.name));
      if (o && label !== null && label !== undefined) out.push({ label, value: o.value });
    }
  } else {
    const o = obj(v);
    if (o) {
      for (const [k, val] of Object.entries(o)) {
        if (shown(val)) out.push({ label: k, value: val });
        else {
          const inner = obj(val);
          if (inner) for (const [k2, v2] of Object.entries(inner)) if (shown(v2)) out.push({ label: `${k}.${k2}`, value: v2 });
        }
      }
    }
  }
  return out;
}

/** GET /api/devices/{id}, or null when it is not a device. */
export function deviceDetails(res: unknown): DeviceDetails | null {
  const o = obj(res);
  const raw = o && (obj(o.device) ?? o);
  const summary = raw ? deviceSummary(raw) : null;
  if (!raw || !summary) return null;
  const diffs: ProfileDiff[] = [];
  const rawDiffs = Array.isArray(raw.diffs) ? raw.diffs : Array.isArray(o?.diffs) ? (o?.diffs as unknown[]) : [];
  for (const d of rawDiffs) {
    const x = obj(d);
    if (!x || typeof x.field !== "string" || !x.field) continue;
    diffs.push({ field: x.field, base_value: x.base_value, lightburn_value: x.lightburn_value, note: str(x.note) });
  }
  const settings: DeviceSetting[] = [];
  const rawSettings = raw.settings ?? o?.settings;
  if (Array.isArray(rawSettings)) {
    for (const e of rawSettings) {
      const x = obj(e);
      if (x && Array.isArray(x.rows)) {
        // The engine's sections: `{title, rows: [{label, key, value}]}` (LightBurn's Device Settings tabs).
        const group = str(x.title) ?? str(x.group) ?? "General";
        for (const r of x.rows) {
          const y = obj(r);
          const label = y && (str(y.label) ?? str(y.key));
          if (y && label) settings.push({ group, label, value: y.value });
        }
        continue;
      }
      const label = x && (str(x.label) ?? str(x.key));
      if (x && label) settings.push({ group: str(x.group) ?? "General", label, value: x.value });
    }
  } else if (obj(rawSettings)) {
    // `{group: {label: value}}` or a flat `{label: value}`.
    for (const [k, v] of Object.entries(obj(rawSettings) ?? {})) {
      if (obj(v)) for (const p of pairs(v)) settings.push({ group: k, label: p.label, value: p.value });
      else if (v === null || typeof v !== "object") settings.push({ group: "General", label: k, value: v });
    }
  }
  // The profile summary: an object of scalars, a label/value list, or `{summary, base}`.
  const profile = raw.profile ?? o?.profile;
  const profileObj = obj(profile);
  return {
    ...summary,
    profile: pairs(profileObj?.summary ?? profile),
    base: str(raw.base) ?? str(profileObj?.base) ?? str(o?.base),
    diffs,
    settings,
  };
}

/** Names from a list of names, `{name}` rows or ids; a count becomes that many unnamed entries. */
function names(v: unknown): string[] {
  if (typeof v === "number" && Number.isInteger(v) && v > 0) return Array.from({ length: Math.min(v, 1000) }, () => "");
  if (!Array.isArray(v)) return [];
  const out: string[] = [];
  for (const e of v) {
    if (typeof e === "string") out.push(e);
    else {
      const o = obj(e);
      const n = o && (str(o.display_name) ?? str(o.name) ?? str(o.id));
      if (n) out.push(n);
    }
  }
  return out;
}

/** POST /api/devices/import as an `ImportResult`. */
export function importResult(res: unknown): ImportResult {
  const r = obj(res) ?? {};
  const skipped: { name: string; reason: string }[] = [];
  if (Array.isArray(r.skipped)) {
    for (const e of r.skipped) {
      const o = obj(e);
      if (typeof e === "string") skipped.push({ name: e, reason: "" });
      else if (o) skipped.push({ name: str(o.name) ?? str(o.display_name) ?? "?", reason: str(o.reason) ?? "" });
    }
  }
  return { added: names(r.added), updated: names(r.updated), skipped };
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

/** One-line toast for an import: "Added 2 devices, updated 1." plus the first skip reason. */
export function importMessage(r: ImportResult): { msg: string; kind: "info" | "error" } {
  const parts: string[] = [];
  if (r.added.length) parts.push(`added ${plural(r.added.length, "device")}`);
  if (r.updated.length) parts.push(`updated ${plural(r.updated.length, "device")}`);
  if (r.skipped.length) {
    const first = r.skipped[0];
    parts.push(`skipped ${r.skipped.length}${first.reason ? ` (${first.name}: ${first.reason}${r.skipped.length > 1 ? ", …" : ""})` : ""}`);
  }
  if (parts.length === 0) return { msg: tr("apid.noDevicesWereImported"), kind: "error" };
  const s = parts.join(", ");
  // Importing the same devices again is not an error: the engine skips them as "already imported, unchanged".
  const allUnchanged = r.skipped.length > 0 && r.skipped.every((x) => /unchanged/i.test(x.reason));
  return { msg: `${s.charAt(0).toUpperCase()}${s.slice(1)}.`, kind: r.added.length || r.updated.length || allUnchanged ? "info" : "error" };
}

// ---- the active device and the canvas -------------------------------------------------------------------------------

/** The active device of a list, or null. */
export function activeDevice(list: DeviceList | null | undefined): DeviceSummary | null {
  if (!list || list.active === null) return null;
  return list.devices.find((d) => d.id === list.active) ?? null;
}

/** The active device GET /api/machine reports (`device`, or `active_device`), or null. */
export function machineDevice(machine: MachineStatus | null | undefined): DeviceSummary | null {
  return deviceSummary(machine?.device) ?? deviceSummary(machine?.active_device);
}

/**
 * The canvas workspace in mm: the active device's (a galvo field may be non-square), else the
 * machine's field, else `fallback`. Objects live in the field frame, so the workspace is centred on (0, 0).
 */
export function canvasWorkspace(device: DeviceSummary | null, machineFieldMm: number | null, fallback: number): Vec2 {
  if (device) {
    // A galvo's field is [width, height]: square unless the profile has a separate height (LightBurn's Width and
    // Height), which the engine reports as the device's workspace.
    const [w, h] = device.workspace_mm;
    return [w, h];
  }
  const f = pos(machineFieldMm) ?? fallback;
  return [f, f];
}

/** Whether a workspace is square (a galvo field), to the micrometre. */
export function isSquare(ws: Vec2): boolean {
  return Math.abs(ws[0] - ws[1]) < 1e-3;
}

/** "153.8 mm" for a square field, "600 × 400 mm" otherwise. */
export function workspaceText(ws: Vec2): string {
  const n = (v: number) => String(Number(v.toFixed(2)));
  return isSquare(ws) ? `${n(ws[0])} mm` : `${n(ws[0])} × ${n(ws[1])} mm`;
}

/** A device's size text: a galvo shows its field (square unless the height differs by more than 0.05 mm, the
 * engine's rule for a non-square field), anything else its workspace. */
export function deviceSizeText(d: Pick<DeviceSummary, "kind" | "workspace_mm">): string {
  const [w, h] = d.workspace_mm;
  if (!isGalvo(d.kind)) return `${Number(w.toFixed(2))} × ${Number(h.toFixed(2))} mm`; // a square bed is still W x H
  return workspaceText(Math.abs(w - h) <= 0.05 + 1e-9 ? [w, w] : d.workspace_mm);
}

/**
 * Why switching devices is not allowed now, or null: never while a run is approved, waiting for its confirmation
 * input or running, or while the red pointer frames (the engine refuses with 409 too).
 */
export function switchBlocker(machine: MachineStatus | null | undefined): string | null {
  if (!machine) return null;
  const runs = Array.isArray(machine.runs) ? machine.runs : [];
  if (machine.running === true || runs.some((r) => r && typeof r.state === "string" && ACTIVE_STATES.includes(r.state)))
    return tr("apid.aRunIsUnderWayWait");
  if (machine.framing === true) return tr("apid.theRedPointerIsFramingStop");
  return null;
}

/** Why `device` cannot be made active now, or null. */
export function activateBlocker(device: DeviceSummary | null, list: DeviceList | null, machine: MachineStatus | null | undefined): string | null {
  if (!device) return tr("apid.selectADevice");
  if (list?.active === device.id) return tr("apid.v1IsAlreadyTheActiveDevice", { v1: device.display_name });
  return switchBlocker(machine);
}

/** Why `device` cannot be removed, or null: the built-in device and the active device stay. */
export function removeBlocker(device: DeviceSummary | null, list: DeviceList | null): string | null {
  if (!device) return tr("apid.selectADevice");
  if (device.source === "builtin") return tr("apid.theBuiltInDeviceCannotBe");
  if (list?.active === device.id) return tr("apid.makeAnotherDeviceActiveBeforeRemoving");
  return null;
}

/** The device a click selects after the list changed: the same id if it is still there, else the active, else the first. */
export function keepSelection(list: DeviceList, selected: string | null): string | null {
  if (selected !== null && list.devices.some((d) => d.id === selected)) return selected;
  return list.active ?? list.devices[0]?.id ?? null;
}

/** The Devices window's groups, in this order: galvos, then each kind of gantry. */
const GROUP_ORDER: readonly DeviceKind[] = ["jcz_galvo", "galvo_other", "grbl", "ruida", "gantry", "unknown"];

/**
 * The Devices window's list, grouped by kind (galvos first) and sorted inside each group: the active device first,
 * then the ones that run jobs before design-only ones, then by name (numbers in names in numeric order).
 */
export function deviceGroups(list: DeviceList): { kind: DeviceKind; label: string; devices: DeviceSummary[] }[] {
  const byName = (a: DeviceSummary, b: DeviceSummary) => a.display_name.localeCompare(b.display_name, undefined, { numeric: true, sensitivity: "base" });
  const rank = (d: DeviceSummary) => (d.id === list.active ? 0 : d.controllable ? 1 : 2);
  return GROUP_ORDER.flatMap((kind) => {
    const devices = list.devices.filter((d) => d.kind === kind).sort((a, b) => rank(a) - rank(b) || byName(a, b));
    return devices.length ? [{ kind, label: kindLabel(kind), devices }] : [];
  });
}

/** The registry device whose display name or name is `nameOrDisplay` (case-insensitive; display names first). */
export function findRegistryDevice(devices: readonly DeviceSummary[], nameOrDisplay: string | null | undefined): DeviceSummary | null {
  const k = nameOrDisplay?.trim().toLowerCase();
  if (!k) return null;
  return devices.find((d) => d.display_name.trim().toLowerCase() === k) ?? devices.find((d) => d.name.trim().toLowerCase() === k) ?? null;
}

/**
 * Registry devices in the shape the LightBurn import dialog uses (api/lightburn.ts). `name` is the display name, what
 * the UI shows and `.lbrn2` files carry; `display_name` holds the controller name, so `findDevice` matches a file's
 * DeviceName against display names first and controller names second, as the engine does.
 */
export function asLightBurnDevices(devices: readonly DeviceSummary[]): LightBurnDevice[] {
  return devices.map((d) => ({
    name: d.display_name,
    display_name: d.name,
    // The engine lays a galvo's LightBurn files out for LightBurn's field, not the profile's.
    field_mm: d.lightburn_field_mm ?? d.workspace_mm[0],
    width_mm: d.workspace_mm[0],
    height_mm: d.workspace_mm[1],
    kind: d.kind,
  }));
}

// ---- labels ---------------------------------------------------------------------------------------------------------

export function kindLabel(kind: DeviceKind | string): string {
  switch (kind) {
    case "jcz_galvo":
      return tr("apid.jczGalvoEzcad2Lmc");
    case "galvo_other":
      return "Galvo";
    case "grbl":
      return "GRBL gantry";
    case "ruida":
      return "Ruida gantry";
    case "gantry":
      return "Gantry";
    default:
      return "Unknown";
  }
}

export function originLabel(origin: DeviceOrigin | string): string {
  switch (origin) {
    case "center":
      return tr("apid.centreGalvo");
    case "bottom_left":
      return tr("apid.frontLeft");
    case "top_left":
      return tr("apid.rearLeft");
    case "bottom_right":
      return tr("apid.frontRight");
    case "top_right":
      return tr("apid.rearRight");
    default:
      return origin;
  }
}

export function sourceLabel(source: DeviceSource | string): string {
  switch (source) {
    case "builtin":
      return tr("apid.builtIn");
    case "lightburn-prefs":
      return "LightBurn";
    case "lbdev":
      return ".lbdev file";
    case "manual":
      return tr("apid.addedByHand");
    default:
      return source;
  }
}

/** Why OpenEtch cannot run jobs on `device`, or null when it can. */
export function designOnlyReason(device: DeviceSummary | null): string | null {
  // OpenEtch Design drives no device at all, so none is singled out (the app says so once, mode.ts).
  if (DESIGN_ONLY || !device || device.controllable) return null;
  const what = device.kind === "gantry" ? "a gantry laser" : device.kind === "galvo_other" ? "this galvo controller" : "this controller";
  return tr("apid.openetchCanTDriveV1V2", { v1: what, v2: device.name });
}

/** The dropdown/list text of a device: "JCZFiber (Unknown) · 153.8 mm". */
export function deviceOptionText(d: DeviceSummary): string {
  return `${d.display_name} · ${deviceSizeText(d)}`;
}

/** The confirmation text before switching to `next`. */
export function switchMessage(next: DeviceSummary, current: DeviceSummary | null): string {
  const ws = deviceSizeText(next);
  const parts = [`Switch to ${next.display_name}? The canvas becomes its ${isGalvo(next.kind) ? "field" : "workspace"} (${ws}).`];
  if (next.controllable) parts.push(tr("apid.theEngineReInitialisesTheLaser"));
  else parts.push(tr("apid.openetchCanTRunJobsOn"));
  if (current && current.origin !== next.origin) parts.push(tr("apid.itsOriginDiffersOpenDesignsKeep"));
  return parts.join(" ");
}

// ---- import body ----------------------------------------------------------------------------------------------------

/** The engine's limit for POST /api/devices/import. */
export const MAX_DEVICE_IMPORT_BYTES = 2 * 1024 * 1024;

/** Larger picked files are refused before they are read (prefs.ini is tens of KB, an .lbdev a few). */
export const MAX_DEVICE_FILE_BYTES = 32 * 1024 * 1024;

/** Top-level device keys kept. Everything else (GUID, Info address, macros, checklists, cut lists...) is dropped. */
const TOP_KEYS = ["Name", "DisplayName", "Type", "Width", "Height", "MirrorX", "MirrorY", "HomeOnStartup"] as const;

/** Settings shown read-only for every family (job origin, bounds check, preview speeds, GCode connection basics). */
const GENERIC_SETTINGS_KEYS: readonly string[] = [
  "CutOrigin",
  "EnableBoundsCheck",
  "Units",
  "Sim_MaxSpeedX",
  "Sim_MaxSpeedY",
  "Sim_RapidSpeed",
  "Sim_MaxAccelX",
  "Sim_MaxAccelY",
  "UserFinishX",
  "UserFinishY",
  "BaudRate",
  "NetworkPort",
  "S_Scale",
  "AirAssistM7",
  "EnableZ",
  "EnableU",
  "TransferMode",
  "GCodeFlavor",
];

/** Settings whose value is a local path: only the file name is kept. */
const PATH_KEYS = new Set(["CorrectionFile"]);

const SETTINGS_KEYS: readonly string[] = [...DEVICE_SETTINGS_KEYS, ...GENERIC_SETTINGS_KEYS];

/** JSON scalars only (a crafted file must not smuggle nested data under an allowed key). */
function scalar(v: unknown): v is string | number | boolean | null {
  return v === null || typeof v === "string" || typeof v === "boolean" || (typeof v === "number" && Number.isFinite(v));
}

function baseName(p: string): string {
  return p.split(/[\\/]/).pop() ?? p;
}

/** A device's name for the import picker: its DisplayName, else its controller Name. */
function entryName(d: Record<string, unknown>): string {
  return str(d.DisplayName) ?? str(d.Name) ?? tr("apid.unnamedDevice");
}

/** What a picked prefs.ini or .lbdev holds, after the cut: the body to send and the device names, in file order. */
export interface DeviceImportBody {
  body: string;
  /** "prefs" for LightBurn's prefs.ini (it has a DefaultDevice or other settings), "lbdev" for a device file. */
  kind: "prefs" | "lbdev";
  names: string[];
  /** Index into `names` of LightBurn's default device, when the file says. */
  defaultIndex: number | null;
}

/**
 * The body for POST /api/devices/import from the text of LightBurn's prefs.ini or a .lbdev file (both are JSON with a
 * `DeviceList`): a `{DefaultDevice?, DeviceList}` object, each device cut down to the typed fields above (paths cut to
 * file names, `Settings` scalars only). Throws an Error with a plain message for anything else.
 */
export function deviceImportBody(text: string): DeviceImportBody {
  let data: unknown;
  try {
    data = JSON.parse(text.charCodeAt(0) === 0xfeff ? text.slice(1) : text);
  } catch {
    throw new Error(tr("apid.thisIsNotALightburnDevice"));
  }
  const root = obj(data);
  if (!root) throw new Error(tr("apid.thisIsNotALightburnDevice2"));
  const list = root.DeviceList;
  if (!Array.isArray(list)) throw new Error(tr("apid.thisFileHasNoLightburnDevice"));
  const devices: Record<string, unknown>[] = [];
  const names: string[] = [];
  let defaultIndex: number | null = null;
  const dflt = root.DefaultDevice;
  list.forEach((d, i) => {
    const src = obj(d);
    if (!src) return;
    const out: Record<string, unknown> = {};
    for (const k of TOP_KEYS) if (k in src && scalar(src[k])) out[k] = src[k];
    // The material library linked to the device: its file name only (the engine shows it read-only).
    if (typeof src.LastDevLibraryPath === "string" && src.LastDevLibraryPath) out.LastDevLibraryPath = baseName(src.LastDevLibraryPath);
    const s = obj(src.Settings);
    if (s) {
      const settings: Record<string, unknown> = {};
      for (const k of SETTINGS_KEYS) {
        const v = s[k];
        if (v === undefined || !scalar(v)) continue;
        settings[k] = PATH_KEYS.has(k) && typeof v === "string" ? baseName(v) : v;
      }
      out.Settings = settings;
    }
    if (typeof dflt === "number" && dflt === i) defaultIndex = devices.length;
    devices.push(out);
    names.push(entryName(src));
  });
  if (devices.length === 0) throw new Error(tr("apid.thisFileListsNoLightburnDevices"));
  const kind: "prefs" | "lbdev" = "DefaultDevice" in root || Object.keys(root).some((k) => k !== "DeviceList") ? "prefs" : "lbdev";
  const body = JSON.stringify(defaultIndex !== null ? { DefaultDevice: defaultIndex, DeviceList: devices } : { DeviceList: devices });
  if (new TextEncoder().encode(body).length > MAX_DEVICE_IMPORT_BYTES) throw new Error(tr("apid.theDeviceListIsLargerThan"));
  return { body, kind, names, defaultIndex };
}

/** The `add` query value: every device, or one by name. */
export function importAddParam(choice: string | null): string {
  return choice === null || choice === "" ? "all" : choice;
}

// ---- GRBL devices ---------------------------------------------------------------------------------------------------

/** The New GRBL device form, as typed (numbers as text). */
export interface GrblForm {
  displayName: string;
  /** COM5, /dev/ttyUSB0. */
  port: string;
  baud: string;
  widthMm: string;
  heightMm: string;
  origin: Exclude<DeviceOrigin, "center">;
  homing: boolean;
  /** `$30`. */
  sMax: string;
  maxFeedMmMin: string;
  /** M4 (dynamic power) rather than M3. */
  dynamicPower: boolean;
}

export const GRBL_FORM_DEFAULT: GrblForm = {
  displayName: "GRBL laser",
  port: "",
  baud: "115200",
  widthMm: "400",
  heightMm: "400",
  origin: "bottom_left",
  homing: false,
  sMax: "1000",
  maxFeedMmMin: "6000",
  dynamicPower: true,
};

/**
 * The body of POST /api/devices/grbl from the form, or the first problem as a plain sentence. The engine validates
 * the profile again (and is the one that matters); this only catches typing slips before the request.
 */
export function grblDeviceBody(f: GrblForm): { body: { display_name: string; profile: Record<string, unknown> } } | { error: string } {
  const num = (v: string) => (v.trim() === "" ? NaN : Number(v));
  const name = f.displayName.trim();
  if (!name) return { error: "Give the device a name." };
  const baud = num(f.baud);
  if (!Number.isInteger(baud) || baud < 300 || baud > 4_000_000) return { error: "The baud rate must be a whole number (GRBL 1.1 uses 115200)." };
  const w = num(f.widthMm);
  const h = num(f.heightMm);
  if (!(w > 0 && h > 0 && w <= 5000 && h <= 5000)) return { error: "The work area must be positive, at most 5000 mm each way." };
  const sMax = num(f.sMax);
  if (!(sMax > 0 && sMax <= 100_000)) return { error: "$30 (maximum S) must be positive." };
  const feed = num(f.maxFeedMmMin);
  if (!(feed > 0 && feed <= 100_000)) return { error: "The maximum feed must be positive (mm/min)." };
  return {
    body: {
      display_name: name,
      profile: {
        port: f.port.trim(),
        baud,
        work_area_mm: [w, h],
        origin: f.origin,
        homing: f.homing,
        s_max: sMax,
        max_feed_mm_min: feed,
        frame_feed_mm_min: Math.min(3000, feed),
        dynamic_power: f.dynamicPower,
      },
    },
  };
}
