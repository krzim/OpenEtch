// LightBurn device settings on the web side: the engine's GET /api/lightburn answer and the `lightburn` part of GET
// /api/machine, the DeviceName a .lbrn2 file was saved for, the field size a LightBurn import uses when nobody types
// one, and the device list taken out of LightBurn's prefs.ini before it is uploaded. No React and no fetch (apart from
// reading a picked file's first bytes), so `npm test` covers it.
//
// LightBurn files do not record the galvo field size, but LightBurn 2.x writes the device they were made for as the root
// attribute `DeviceName` (the device's DisplayName, e.g. "JCZFiber (Unknown)"). The engine's rule for an import with no
// `field_mm` (ARCHITECTURE.md, "Field size"): the device named by the file, else the engine's LightBurn device
// (`--lightburn-device`, else LightBurn's default device), else the document's field, else the machine profile's. The
// UI shows the size that rule picks before the import, and sends no `field_mm` unless the user edits it.
//
// prefs.ini (JSON) holds much more than devices, including secrets (e.g. RestPendantSecret, API auths, inside each
// device entry). `devicePrefsBody` keeps only the device fields the engine maps, so nothing else leaves this browser.

import type { MachineStatus } from "./types.ts";

/** One device of GET /api/lightburn (only these fields are sent). */
export interface LightBurnDevice {
  name: string;
  display_name: string;
  /** Field size OpenEtch uses for this device (its width). */
  field_mm: number | null;
  width_mm: number | null;
  height_mm: number | null;
}

/** GET /api/lightburn. `source` is where the engine got the devices: LightBurn on its own computer, or an upload. */
export interface LightBurnInfo {
  available: boolean;
  source: "file" | "uploaded" | null;
  default_device: string | null;
  devices: LightBurnDevice[];
}

/** One difference between the active machine profile and the matched LightBurn device. */
export interface ProfileDiff {
  field: string;
  base_value: unknown;
  lightburn_value: unknown;
  note: string | null;
}

/** GET /api/machine `lightburn`: the matched LightBurn device and how the machine profile differs from it. */
export interface MachineLightBurn {
  device: string | null;
  field_mm: number | null;
  diffs: ProfileDiff[];
}

function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) && v > 0 ? v : null;
}

function str(v: unknown): string | null {
  return typeof v === "string" && v.trim() !== "" ? v : null;
}

/** GET /api/lightburn as a `LightBurnInfo`; anything malformed becomes "no devices". */
export function lightburnInfo(res: unknown): LightBurnInfo {
  const r = res && typeof res === "object" && !Array.isArray(res) ? (res as Record<string, unknown>) : {};
  const devices: LightBurnDevice[] = [];
  if (Array.isArray(r.devices)) {
    for (const d of r.devices) {
      if (!d || typeof d !== "object") continue;
      const o = d as Record<string, unknown>;
      const name = str(o.name) ?? str(o.display_name);
      if (name === null) continue;
      const width = num(o.width_mm);
      devices.push({
        name,
        display_name: str(o.display_name) ?? name,
        field_mm: num(o.field_mm) ?? width,
        width_mm: width,
        height_mm: num(o.height_mm),
      });
    }
  }
  const source = r.source === "file" || r.source === "uploaded" ? r.source : null;
  return { available: r.available === true && devices.length > 0, source, default_device: str(r.default_device), devices };
}

/** GET /api/machine `lightburn`, or null when the engine reports none. */
export function machineLightBurn(machine: MachineStatus | null | undefined): MachineLightBurn | null {
  const raw = machine?.lightburn;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const r = raw as Record<string, unknown>;
  const diffs: ProfileDiff[] = [];
  if (Array.isArray(r.diffs)) {
    for (const d of r.diffs) {
      if (!d || typeof d !== "object") continue;
      const o = d as Record<string, unknown>;
      if (typeof o.field !== "string" || !o.field) continue;
      diffs.push({ field: o.field, base_value: o.base_value, lightburn_value: o.lightburn_value, note: str(o.note) });
    }
  }
  return { device: str(r.device), field_mm: num(r.field_mm), diffs };
}

/** A diff value for display: numbers to 4 significant decimals, booleans as Yes/No, null as "–". */
export function formatDiffValue(v: unknown): string {
  if (v === null || v === undefined) return "–";
  if (typeof v === "number") return Number.isFinite(v) ? String(Number(v.toFixed(4))) : String(v);
  if (typeof v === "boolean") return v ? "Yes" : "No";
  if (typeof v === "string") return v;
  return JSON.stringify(v);
}

/** A profile field name for display: "max_mark_speed_mm_s" -> "max mark speed mm s". */
export function diffLabel(field: string): string {
  return field.replace(/[._]+/g, " ").trim();
}

/** Case-insensitive comparison key for device names. */
function key(s: string): string {
  return s.trim().toLowerCase();
}

/** The device whose name or display name is `nameOrDisplay` (case-insensitive), as the engine's `find_device`. */
export function findDevice(devices: readonly LightBurnDevice[], nameOrDisplay: string | null | undefined): LightBurnDevice | null {
  if (!nameOrDisplay || !nameOrDisplay.trim()) return null;
  const k = key(nameOrDisplay);
  return devices.find((d) => key(d.name) === k) ?? devices.find((d) => key(d.display_name) === k) ?? null;
}

// ---- DeviceName of a .lbrn2 file ------------------------------------------------------------------------------------

/** How much of a .lbrn2 file is read to find its root element (the root comes right after the XML declaration). */
export const LBRN_HEAD_BYTES = 64 * 1024;

const ENTITIES: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };

/** Decode XML character and predefined entity references in an attribute value. */
export function decodeXmlAttr(s: string): string {
  return s.replace(/&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);/g, (m, ref: string) => {
    if (ref[0] === "#") {
      const code = ref[1] === "x" || ref[1] === "X" ? parseInt(ref.slice(2), 16) : parseInt(ref.slice(1), 10);
      return Number.isFinite(code) && code >= 0 && code <= 0x10ffff ? String.fromCodePoint(code) : m;
    }
    return ENTITIES[ref] ?? m;
  });
}

/**
 * The `DeviceName` attribute of a LightBurn project's root element (LightBurn 2.x writes the device's DisplayName,
 * e.g. "JCZFiber (Unknown)"), or null when the file has none (LightBurn 1.x) or is not a LightBurn project. Only the
 * root start tag counts: an element further down named the same way is ignored. `head` is the start of the file.
 */
export function lbrnDeviceName(head: string): string | null {
  let i = head.charCodeAt(0) === 0xfeff ? 1 : 0;
  // Skip the prolog: whitespace, the XML declaration and other processing instructions, comments and a DOCTYPE.
  for (;;) {
    while (i < head.length && /\s/.test(head[i])) i++;
    if (head.startsWith("<?", i)) {
      const end = head.indexOf("?>", i + 2);
      if (end < 0) return null;
      i = end + 2;
    } else if (head.startsWith("<!--", i)) {
      const end = head.indexOf("-->", i + 4);
      if (end < 0) return null;
      i = end + 3;
    } else if (head.startsWith("<!", i)) {
      const end = head.indexOf(">", i + 2);
      if (end < 0) return null;
      i = end + 1;
    } else break;
  }
  const open = /^<([A-Za-z_][\w.:-]*)/.exec(head.slice(i, i + 256));
  if (!open || open[1] !== "LightBurnProject") return null;
  // Attributes run to the first '>' outside a quoted value.
  let j = i + open[0].length;
  let quote: string | null = null;
  for (; j < head.length; j++) {
    const c = head[j];
    if (quote) {
      if (c === quote) quote = null;
    } else if (c === '"' || c === "'") quote = c;
    else if (c === ">") break;
  }
  if (j >= head.length) return null;
  const attrs = head.slice(i + open[0].length, j);
  const re = /([A-Za-z_][\w.:-]*)\s*=\s*("([^"]*)"|'([^']*)')/g;
  for (let m = re.exec(attrs); m; m = re.exec(attrs)) {
    if (m[1] === "DeviceName") {
      const v = decodeXmlAttr(m[3] ?? m[4] ?? "").trim();
      return v || null;
    }
  }
  return null;
}

/** Read a picked .lbrn2 file's DeviceName from its first bytes; null when it has none or cannot be read. */
export async function readLbrnDeviceName(file: Blob): Promise<string | null> {
  try {
    return lbrnDeviceName(await file.slice(0, LBRN_HEAD_BYTES).text());
  } catch {
    return null;
  }
}

// ---- the import field size ------------------------------------------------------------------------------------------

/** Where a LightBurn import's field size comes from (the engine's `field_source`, without the device name). */
export type FieldSourceKind = "file-device" | "default-device" | "document" | "machine" | "caller";

export interface FieldSource {
  kind: FieldSourceKind;
  /** The LightBurn device's name, for the two device kinds. */
  device: string | null;
}

/** The field size a LightBurn import will use, and why. */
export interface ImportFieldDefault extends FieldSource {
  fieldMm: number;
  /**
   * The file's DeviceName when no known LightBurn device has that name (the file was made on another computer, or
   * the engine has no LightBurn settings): the size then comes from further down the rule.
   */
  unknownFileDevice: string | null;
}

/**
 * The field size the engine uses for a LightBurn import when the caller sends none: the device the file was saved
 * for, else `engineDevice` (the engine's `--lightburn-device` or LightBurn's default device), else the document's
 * field, else the machine profile's, else `fallback` (the UI's default before the engine answers).
 */
export function lbrnImportField(o: {
  fileDevice: string | null;
  devices: readonly LightBurnDevice[];
  engineDevice: string | null;
  docField: number | null;
  machineField: number | null;
  fallback: number;
}): ImportFieldDefault {
  const fromFile = findDevice(o.devices, o.fileDevice);
  const unknownFileDevice = o.fileDevice && !fromFile ? o.fileDevice : null;
  if (fromFile && fromFile.field_mm !== null) return { kind: "file-device", device: fromFile.name, fieldMm: fromFile.field_mm, unknownFileDevice };
  const dflt = findDevice(o.devices, o.engineDevice);
  if (dflt && dflt.field_mm !== null) return { kind: "default-device", device: dflt.name, fieldMm: dflt.field_mm, unknownFileDevice };
  const doc = num(o.docField);
  if (doc !== null) return { kind: "document", device: null, fieldMm: doc, unknownFileDevice };
  const machine = num(o.machineField);
  if (machine !== null) return { kind: "machine", device: null, fieldMm: machine, unknownFileDevice };
  return { kind: "machine", device: null, fieldMm: o.fallback, unknownFileDevice };
}

/** The LightBurn device the engine falls back to: the one it matched for the machine card, else LightBurn's default. */
export function engineLightBurnDevice(machine: MachineStatus | null | undefined, info: LightBurnInfo | null): string | null {
  return machineLightBurn(machine)?.device ?? info?.default_device ?? null;
}

/** Parse the engine's `field_source` ("file device JCZFiber", "LightBurn default device JCZFiber", "document", ...). */
export function parseFieldSource(v: unknown): FieldSource | null {
  if (typeof v !== "string") return null;
  const s = v.trim();
  let m = /^file device\s+(.+)$/i.exec(s);
  if (m) return { kind: "file-device", device: m[1] };
  m = /^LightBurn default device\s+(.+)$/i.exec(s);
  if (m) return { kind: "default-device", device: m[1] };
  const k = s.toLowerCase();
  if (k === "document" || k === "machine" || k === "caller") return { kind: k, device: null };
  return null;
}

/** Where a field size comes from, in plain words, to follow "Field size 153.8 mm — ". */
export function fieldSourceText(s: FieldSource): string {
  switch (s.kind) {
    case "file-device":
      return `from your LightBurn device ${s.device}`;
    case "default-device":
      return `from your default LightBurn device ${s.device}`;
    case "document":
      return "the field size of your current design";
    case "machine":
      return "the field size of this laser";
    case "caller":
      return "the size you entered";
  }
}

/**
 * One line for several files: the shared size and source when every file resolves to the same one, else null (each
 * file then uses its own, and the dialog lists them).
 */
export function sharedField(list: readonly ImportFieldDefault[]): ImportFieldDefault | null {
  if (list.length === 0) return null;
  const [first] = list;
  const same = list.every((f) => Math.abs(f.fieldMm - first.fieldMm) < 1e-6 && f.kind === first.kind && f.device === first.device);
  return same ? first : null;
}

/** A one-tap field size in the LightBurn dialogs, with what it is ("JCZFiber", "document", "machine"...). */
export interface FieldChoice {
  mm: number;
  label: string | null;
}

/** Field-size chips in order, without sizes that repeat an earlier one (the first label wins) or are unusable. */
export function labelledFieldChoices(entries: readonly { mm: number | null | undefined; label: string | null }[]): FieldChoice[] {
  const out: FieldChoice[] = [];
  for (const e of entries) {
    const mm = num(e.mm);
    if (mm !== null && !out.some((c) => Math.abs(c.mm - mm) < 1e-3)) out.push({ mm, label: e.label });
  }
  return out;
}

// ---- prefs.ini upload ---------------------------------------------------------------------------------------------

/** The engine's limit for POST /api/lightburn/prefs. */
export const MAX_PREFS_BYTES = 2 * 1024 * 1024;

/** Larger picked files are refused before they are read (LightBurn's prefs.ini is tens of KB). */
export const MAX_PREFS_FILE_BYTES = 32 * 1024 * 1024;

/** Device entry fields kept for the engine. Everything else (REST secrets, API auths, macros, cut lists...) is dropped. */
const DEVICE_KEYS = ["Name", "DisplayName", "Type", "Width", "Height", "MirrorX", "MirrorY"] as const;

/** Device `Settings` the engine maps to a machine profile (ARCHITECTURE.md, "LightBurn devices"). */
export const DEVICE_SETTINGS_KEYS: readonly string[] = [
  "CorrectionFile",
  "CorScaleOverride",
  "Default_LaserOn_TC",
  "Default_LaserOff_TC",
  "Default_Polygon_TC",
  "Default_End_TC",
  "Default_JumpSpeed",
  "Default_MinJumpDelay",
  "Default_MaxJumpDelay",
  "Default_MaxJumpDistance",
  "FieldAngle",
  "FieldOffsetX",
  "FieldOffsetY",
  "FrameSpeed",
  "Galvo_1_is_X",
  ...[1, 2].flatMap((n) => ["Sign", "Scale", "Skew", "Trapezoid", "Bulge"].map((k) => `Galvo_${n}_${k}`)),
  "Laser_FiberType",
  "Laser_Type",
  "Laser_MinFreq",
  "Laser_MaxFreq",
  "Laser_QPulseWidthEnabled",
  "Laser_OpenMODelay",
  "Laser_PWMEnabled",
  "MaxSpeed",
  "RedLightPortBit",
  "RedLightJumpDelay",
  "StartMarkPortBit",
  "StartMarkBitHigh",
  "StartMarkPulseMode",
];

/** Keep only JSON scalars (a crafted file must not smuggle nested data under an allowed key). */
function scalar(v: unknown): v is string | number | boolean | null {
  return v === null || typeof v === "string" || typeof v === "boolean" || (typeof v === "number" && Number.isFinite(v));
}

/** A path's last component ("C:\\Users\\me\\EzCad\\jcz150.cor" -> "jcz150.cor"): the engine only uses the file name. */
function baseName(p: string): string {
  return p.split(/[\\/]/).pop() ?? p;
}

/**
 * The upload body for POST /api/lightburn/prefs, built from the text of LightBurn's prefs.ini: a prefs.ini-shaped
 * JSON object with `DefaultDevice` and `DeviceList` only, each device cut down to the fields above (the correction
 * file reduced to its name). Throws an Error with a plain message when the text is not LightBurn's prefs.ini or has no
 * devices. `devices` is how many devices the body carries.
 */
export function devicePrefsBody(text: string): { body: string; devices: number } {
  let data: unknown;
  try {
    data = JSON.parse(text.charCodeAt(0) === 0xfeff ? text.slice(1) : text);
  } catch {
    throw new Error("This is not LightBurn's prefs.ini (it is not a JSON settings file).");
  }
  if (!data || typeof data !== "object" || Array.isArray(data)) throw new Error("This is not LightBurn's prefs.ini.");
  const list = (data as { DeviceList?: unknown }).DeviceList;
  if (!Array.isArray(list)) throw new Error("This file has no LightBurn device list. Pick prefs.ini from LightBurn's settings folder.");
  const devices: Record<string, unknown>[] = [];
  for (const d of list) {
    if (!d || typeof d !== "object" || Array.isArray(d)) continue;
    const src = d as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    for (const k of DEVICE_KEYS) if (k in src && scalar(src[k])) out[k] = src[k];
    const s = src.Settings;
    if (s && typeof s === "object" && !Array.isArray(s)) {
      const settings: Record<string, unknown> = {};
      for (const k of DEVICE_SETTINGS_KEYS) {
        const v = (s as Record<string, unknown>)[k];
        if (v === undefined || !scalar(v)) continue;
        settings[k] = k === "CorrectionFile" && typeof v === "string" ? baseName(v) : v;
      }
      out.Settings = settings;
    }
    devices.push(out);
  }
  if (devices.length === 0) throw new Error("LightBurn's settings list no devices.");
  const dflt = (data as { DefaultDevice?: unknown }).DefaultDevice;
  const body = JSON.stringify(typeof dflt === "number" && Number.isInteger(dflt) ? { DefaultDevice: dflt, DeviceList: devices } : { DeviceList: devices });
  if (new TextEncoder().encode(body).length > MAX_PREFS_BYTES) throw new Error("LightBurn's device list is larger than 2 MB.");
  return { body, devices: devices.length };
}

/** Where LightBurn keeps prefs.ini, for the upload hint (Linux path as the engine documents it). */
export const PREFS_LOCATIONS = [
  { os: "Windows", path: "%LOCALAPPDATA%\\LightBurn\\prefs.ini" },
  { os: "Mac", path: "~/Library/Preferences/LightBurn/prefs.ini" },
] as const;
