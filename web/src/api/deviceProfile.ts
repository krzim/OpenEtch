// The device settings editor's model (LightBurn's Device Settings for a galvo, ARCHITECTURE.md "Devices" and
// `PUT /api/devices/{id}`): the profile fields grouped like LightBurn's dialog, their current values gathered from what
// the engine reports (`GET /api/devices/{id}` `profile`, the device's `.lbdev` export and, for the active device,
// `GET /api/machine/info`), client-side checks, the JSON merge patch of the edited fields only, and the engine's
// validation errors mapped back to fields. Pure, so `npm test` covers it.
//
// A value the engine does not report stays unknown (`undefined`): the editor shows it empty and never sends it unless
// the user types one, so a save can never overwrite a setting it could not read.

export type FieldKind = "number" | "bool" | "text" | "select" | "bit";

export interface FieldSpec {
  /** Profile path, dot-separated ("calibration.scale_x"). */
  path: string;
  label: string;
  kind: FieldKind;
  /** Engine unit for NumberField ("mm", "mm/s", "µs", "°", "kHz", "%"); lengths and speeds follow the display units. */
  unit?: string;
  min?: number;
  max?: number;
  digits?: number;
  options?: { value: string; label: string }[];
  /** The LightBurn key, shown as a hint. */
  lb?: string;
  hint?: string;
  readOnly?: boolean;
  /** A hard limit: it can be lowered but never raised above the device's base profile. */
  limit?: boolean;
  /** For bits: the lowest bit allowed. */
  minBit?: number;
}

export interface FieldGroup {
  id: string;
  title: string;
  fields: FieldSpec[];
}

/** The profile values by path; `undefined` = not reported by the engine. */
export type ProfileValues = Record<string, unknown>;

/** Which calibration axis LightBurn's galvo `n` (1 or 2) drives: galvo 2 is X unless the axes are swapped. */
export function galvoAxis(n: 1 | 2, swapXY: boolean): "x" | "y" {
  return (n === 2) !== swapXY ? "x" : "y";
}

function galvoFields(n: 1 | 2, swapXY: boolean): FieldSpec[] {
  const a = galvoAxis(n, swapXY);
  const AX = a.toUpperCase();
  return [
    { path: `calibration.scale_${a}`, label: `Scale (${AX})`, kind: "number", min: 0.5, max: 2, digits: 5, lb: `Galvo_${n}_Scale` },
    { path: `calibration.bulge_${a}`, label: `Bulge (${AX})`, kind: "number", digits: 5, lb: `Galvo_${n}_Bulge`, hint: "Distortion near the centre; 1 = none" },
    { path: `calibration.skew_${a}`, label: `Skew (${AX})`, kind: "number", digits: 5, lb: `Galvo_${n}_Skew`, hint: "Distortion near the edges; 1 = none" },
    { path: `calibration.trapezoid_${a}`, label: `Trapezoid (${AX})`, kind: "number", digits: 5, lb: `Galvo_${n}_Trapezoid`, hint: "Corner tapering (keystone); 1 = none" },
  ];
}

/** The editor's groups, in LightBurn's order (Galvo and Basic Settings, then Ports and Laser Settings, then axes). */
export function profileGroups(swapXY: boolean): FieldGroup[] {
  return [
    {
      id: "general",
      title: "General",
      fields: [
        { path: "name", label: "Profile name", kind: "text" },
        { path: "laser_source", label: "Laser source", kind: "text", hint: "Informational, e.g. JPT M7 MOPA 60W" },
      ],
    },
    {
      id: "field",
      title: "Field",
      fields: [
        { path: "field_mm", label: "Field size", kind: "number", unit: "mm", min: 1, max: 1000, digits: 3, lb: "Width / Height", hint: "The lens's field; sets the workspace" },
        { path: "calibration.offset_x_mm", label: "Offset X", kind: "number", unit: "mm", digits: 3, lb: "FieldOffsetX" },
        { path: "calibration.offset_y_mm", label: "Offset Y", kind: "number", unit: "mm", digits: 3, lb: "FieldOffsetY" },
        { path: "calibration.angle_deg", label: "Angle", kind: "number", unit: "°", min: -45, max: 45, digits: 3, lb: "FieldAngle", hint: "Counter-clockwise" },
      ],
    },
    {
      id: "reddot",
      title: "Red dot",
      fields: [
        { path: "red_dot.offset_x_mm", label: "Offset X", kind: "number", unit: "mm", digits: 3, hint: "Framing only; the burn never moves" },
        { path: "red_dot.offset_y_mm", label: "Offset Y", kind: "number", unit: "mm", digits: 3 },
        { path: "red_dot.scale_x", label: "Scale X", kind: "number", min: 0.1, max: 10, digits: 4 },
        { path: "red_dot.scale_y", label: "Scale Y", kind: "number", min: 0.1, max: 10, digits: 4 },
        { path: "red_dot.rest_x_mm", label: "Rest pos X", kind: "number", unit: "mm", digits: 3, hint: "Where the dot parks when framing stops" },
        { path: "red_dot.rest_y_mm", label: "Rest pos Y", kind: "number", unit: "mm", digits: 3 },
      ],
    },
    {
      id: "galvo",
      title: "Galvo configuration",
      fields: [
        { path: "swap_xy", label: "Galvo 1 is X axis", kind: "bool", lb: "Galvo_1_is_X" },
        { path: "negate_x", label: "Reverse X (negate)", kind: "bool", lb: "Galvo_n_Sign" },
        { path: "negate_y", label: "Reverse Y (negate)", kind: "bool", lb: "Galvo_n_Sign" },
        { path: "red_light_speed_mm_s", label: "Frame speed", kind: "number", unit: "mm/s", min: 1, digits: 1, lb: "FrameSpeed" },
        { path: "mark_speed.max_mm_s", label: "Max speed", kind: "number", unit: "mm/s", min: 1, digits: 1, lb: "MaxSpeed", limit: true },
        { path: "mark_speed.min_mm_s", label: "Min speed", kind: "number", unit: "mm/s", min: 0.001, digits: 3 },
      ],
    },
    { id: "galvo1", title: "Galvo 1", fields: galvoFields(1, swapXY) },
    { id: "galvo2", title: "Galvo 2", fields: galvoFields(2, swapXY) },
    {
      id: "jump",
      title: "Jump setting defaults",
      fields: [
        { path: "timing.jump_speed_mm_s", label: "Jump speed", kind: "number", unit: "mm/s", min: 1, digits: 1, lb: "Default_JumpSpeed" },
        { path: "timing.jump_delay_min_us", label: "Min jump delay", kind: "number", unit: "µs", min: 0, digits: 1, lb: "Default_MinJumpDelay" },
        { path: "timing.jump_delay_max_us", label: "Max jump delay", kind: "number", unit: "µs", min: 0, digits: 1, lb: "Default_MaxJumpDelay" },
        { path: "timing.jump_distance_limit_mm", label: "Jump distance limit", kind: "number", unit: "mm", min: 0, digits: 3, lb: "Default_MaxJumpDistance" },
        { path: "max_jump_speed_mm_s", label: "Jump speed limit", kind: "number", unit: "mm/s", min: 1, digits: 1, limit: true, hint: "Hard cap for every job and frame" },
      ],
    },
    {
      id: "delay",
      title: "Delay defaults",
      fields: [
        { path: "timing.laser_on_delay_us", label: "Laser On TC", kind: "number", unit: "µs", digits: 1, lb: "Default_LaserOn_TC" },
        { path: "timing.laser_off_delay_us", label: "Laser Off TC", kind: "number", unit: "µs", digits: 1, lb: "Default_LaserOff_TC" },
        { path: "timing.end_delay_us", label: "End TC", kind: "number", unit: "µs", digits: 1, lb: "Default_End_TC" },
        { path: "timing.polygon_delay_us", label: "Polygon TC", kind: "number", unit: "µs", digits: 1, lb: "Default_Polygon_TC" },
        { path: "max_delay_us", label: "Delay limit", kind: "number", unit: "µs", min: 0, digits: 0, limit: true, hint: "Hard cap on any delay a job may send" },
      ],
    },
    {
      id: "laser",
      title: "Laser source",
      fields: [
        {
          path: "laser_kind",
          label: "Laser type",
          kind: "select",
          lb: "Laser_Type",
          options: [
            { value: "fiber", label: "Fiber" },
            { value: "co2", label: "CO2" },
            { value: "uv", label: "UV" },
          ],
          hint: "Only fiber sources are driven",
        },
        {
          path: "fiber_kind",
          label: "Fiber type",
          kind: "select",
          lb: "Laser_FiberType",
          options: [
            { value: "mopa", label: "MOPA" },
            { value: "q_switched", label: "Q-switched" },
          ],
        },
        { path: "pulse_width_enabled", label: "Enable Q-pulse width setting", kind: "bool", lb: "Laser_QPulseWidthEnabled" },
        { path: "open_mo_delay_us", label: "Open MO delay", kind: "number", unit: "µs", min: 0, digits: 0, lb: "Laser_OpenMODelay" },
        { path: "frequency_khz_min", label: "Min frequency", kind: "number", unit: "kHz", min: 0.001, digits: 3, lb: "Laser_MinFreq", limit: true },
        { path: "frequency_khz_max", label: "Max frequency", kind: "number", unit: "kHz", min: 0.001, digits: 3, lb: "Laser_MaxFreq", limit: true },
        { path: "max_power_pct", label: "Power limit", kind: "number", unit: "%", min: 0, max: 100, digits: 1, limit: true },
      ],
    },
    {
      id: "ports",
      title: "IO ports",
      fields: [
        { path: "start_mark_input_bit", label: "Start marking input", kind: "bit", lb: "StartMarkPortBit", hint: "Foot pedal; none when empty" },
        { path: "stop_mark_input_bit", label: "Stop marking input", kind: "bit", hint: "Aborts a running job when set" },
        { path: "focus_light_port_bit", label: "Focus light output", kind: "bit", minBit: 1, hint: "Second pointer; never the laser-enable (0) or red-dot (8) bit" },
        { path: "blanking", label: "Enable blanking", kind: "bool", hint: "Pointer off between shapes while framing" },
        { path: "red_light_port_bit", label: "Red dot output", kind: "bit", readOnly: true, hint: "Fixed at 8 by the LMC light list" },
        { path: "laser_enable_port_bit", label: "Laser enable output", kind: "bit", readOnly: true, hint: "Fixed at 0" },
      ],
    },
    {
      id: "axis",
      title: "Z axis / rotary",
      fields: [
        { path: "axis.enabled", label: "Extension axis", kind: "bool", hint: "A motor on the board's extension axis (LMC MoveAxisTo); the B6's lift is not on the board" },
        {
          path: "axis.role",
          label: "Drives",
          kind: "select",
          options: [
            { value: "z", label: "Z lift" },
            { value: "rotary", label: "Rotary" },
          ],
        },
        { path: "axis.steps_per_mm", label: "Steps per mm", kind: "number", min: 0, digits: 4, hint: "Z lift" },
        { path: "axis.min_mm", label: "Lowest Z", kind: "number", unit: "mm", digits: 3 },
        { path: "axis.max_mm", label: "Highest Z", kind: "number", unit: "mm", digits: 3 },
        { path: "axis.invert", label: "Reverse direction", kind: "bool" },
      ],
    },
  ];
}

/** Every field path the editor knows (both galvo mappings give the same paths). */
export function allPaths(): string[] {
  return profileGroups(false).flatMap((g) => g.fields.map((f) => f.path));
}

// ---- gathering the current values -----------------------------------------------------------------------------------

function obj(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function finite(v: unknown): number | undefined {
  return typeof v === "number" && Number.isFinite(v) ? v : undefined;
}

function flatten(o: Record<string, unknown>, prefix: string, out: ProfileValues): void {
  for (const [k, v] of Object.entries(o)) {
    const p = prefix ? `${prefix}.${k}` : k;
    const sub = obj(v);
    if (sub) flatten(sub, p, out);
    else out[p] = v;
  }
}

/** The `.lbdev` text's first device's `Settings`, or null. */
export function lbdevSettings(text: string | null): Record<string, unknown> | null {
  if (!text) return null;
  try {
    const d = JSON.parse(text) as { DeviceList?: unknown[] };
    const dev = obj(Array.isArray(d.DeviceList) ? d.DeviceList[0] : null);
    return obj(dev?.Settings) ?? null;
  } catch {
    return null;
  }
}

/**
 * The current values: the profile JSON of `GET /api/devices/{id}` (every key it has, nested objects flattened; the
 * summary's `frequency_khz` and `mark_speed_mm_s` pairs split), then the `.lbdev` export's LightBurn keys for what the
 * summary leaves out, then `GET /api/machine/info` (the active device only) for the laser and fiber type and the axis.
 */
export function profileValues(profileJson: unknown, lbdevText: string | null, info: unknown | null): ProfileValues {
  const v: ProfileValues = {};
  const p = obj(profileJson);
  if (p) {
    const { frequency_khz, mark_speed_mm_s, ...rest } = p;
    flatten(rest, "", v);
    if (Array.isArray(frequency_khz) && frequency_khz.length === 2) {
      v.frequency_khz_min ??= finite(frequency_khz[0]);
      v.frequency_khz_max ??= finite(frequency_khz[1]);
    }
    if (Array.isArray(mark_speed_mm_s) && mark_speed_mm_s.length === 2) {
      v["mark_speed.min_mm_s"] ??= finite(mark_speed_mm_s[0]);
      v["mark_speed.max_mm_s"] ??= finite(mark_speed_mm_s[1]);
    }
    if ("axis" in p) {
      const ax = obj(p.axis);
      v["axis.enabled"] = ax !== null;
    }
  }
  const s = lbdevSettings(lbdevText);
  if (s) {
    const set = (path: string, val: unknown) => {
      if (v[path] === undefined && val !== undefined) v[path] = val;
    };
    set("calibration.offset_x_mm", finite(s.FieldOffsetX));
    set("calibration.offset_y_mm", finite(s.FieldOffsetY));
    set("calibration.angle_deg", finite(s.FieldAngle));
    set("open_mo_delay_us", finite(s.Laser_OpenMODelay));
    set("red_light_speed_mm_s", finite(s.FrameSpeed));
    set("mark_speed.max_mm_s", finite(s.MaxSpeed));
    set("frequency_khz_min", finite(s.Laser_MinFreq));
    set("frequency_khz_max", finite(s.Laser_MaxFreq));
    if (typeof s.Laser_QPulseWidthEnabled === "boolean") set("pulse_width_enabled", s.Laser_QPulseWidthEnabled);
    const smp = finite(s.StartMarkPortBit);
    if (smp !== undefined) set("start_mark_input_bit", smp < 0 ? null : smp);
    // `to_lbdev` writes galvo 1 as Y and galvo 2 as X unless swapped (Galvo_1_is_X).
    const swap = typeof v.swap_xy === "boolean" ? v.swap_xy : s.Galvo_1_is_X === true;
    for (const n of [1, 2] as const) {
      const a = galvoAxis(n, swap);
      for (const k of ["Scale", "Bulge", "Skew", "Trapezoid"]) set(`calibration.${k.toLowerCase()}_${a}`, finite(s[`Galvo_${n}_${k}`]));
    }
    set("timing.laser_on_delay_us", finite(s.Default_LaserOn_TC));
    set("timing.laser_off_delay_us", finite(s.Default_LaserOff_TC));
    set("timing.polygon_delay_us", finite(s.Default_Polygon_TC));
    set("timing.end_delay_us", finite(s.Default_End_TC));
    set("timing.jump_speed_mm_s", finite(s.Default_JumpSpeed));
    set("timing.jump_delay_min_us", finite(s.Default_MinJumpDelay));
    set("timing.jump_delay_max_us", finite(s.Default_MaxJumpDelay));
    set("timing.jump_distance_limit_mm", finite(s.Default_MaxJumpDistance));
  }
  const i = obj(info);
  if (i) {
    if (v.laser_kind === undefined && typeof i.laser_kind === "string") v.laser_kind = i.laser_kind;
    if (v.fiber_kind === undefined && typeof i.fiber_kind === "string") v.fiber_kind = i.fiber_kind;
    if (v["axis.enabled"] === undefined && "axis" in i) {
      const ax = obj(i.axis);
      v["axis.enabled"] = ax !== null;
      if (ax) for (const [k, x] of Object.entries(ax)) v[`axis.${k}`] ??= x;
    }
  }
  // The port bits are fixed by lmc whatever the profile says.
  v.red_light_port_bit ??= 8;
  v.laser_enable_port_bit ??= 0;
  return v;
}

// ---- checks and the patch -------------------------------------------------------------------------------------------

/** Defaults for a newly enabled axis. */
export const AXIS_DEFAULTS = { role: "z", steps_per_mm: 100, min_mm: 0, max_mm: 50, invert: false };

/** The values shown: the edits over the current values. */
export function merged(values: ProfileValues, edits: ProfileValues): ProfileValues {
  return { ...values, ...edits };
}

/** Client-side checks the engine also makes, so the obvious ones show at the field before saving. */
export function checkValues(v: ProfileValues, limits: ProfileValues = {}): Record<string, string> {
  const err: Record<string, string> = {};
  const n = (p: string) => finite(v[p]);
  const pos = (p: string) => {
    const x = n(p);
    if (x !== undefined && !(x > 0)) err[p] = "Must be more than 0";
  };
  pos("field_mm");
  pos("red_light_speed_mm_s");
  pos("timing.jump_speed_mm_s");
  pos("max_jump_speed_mm_s");
  const fmin = n("frequency_khz_min");
  const fmax = n("frequency_khz_max");
  if (fmin !== undefined && fmax !== undefined && fmin >= fmax) err.frequency_khz_max = "Must be above the minimum frequency";
  const smin = n("mark_speed.min_mm_s");
  const smax = n("mark_speed.max_mm_s");
  if (smin !== undefined && smax !== undefined && smin >= smax) err["mark_speed.max_mm_s"] = "Must be above the minimum speed";
  const pw = n("max_power_pct");
  if (pw !== undefined && (pw < 0 || pw > 100)) err.max_power_pct = "0 to 100 %";
  const js = n("timing.jump_speed_mm_s");
  const jl = n("max_jump_speed_mm_s");
  if (js !== undefined && jl !== undefined && js > jl) err["timing.jump_speed_mm_s"] = `Above the jump speed limit (${jl} mm/s)`;
  const fs = n("red_light_speed_mm_s");
  if (fs !== undefined && jl !== undefined && fs > jl) err.red_light_speed_mm_s = `Above the jump speed limit (${jl} mm/s)`;
  const ang = n("calibration.angle_deg");
  if (ang !== undefined && Math.abs(ang) > 45) err["calibration.angle_deg"] = "Within ±45°";
  for (const a of ["x", "y"]) {
    const s = n(`calibration.scale_${a}`);
    if (s !== undefined && (s < 0.5 || s > 2)) err[`calibration.scale_${a}`] = "0.5 to 2";
  }
  for (const p of ["start_mark_input_bit", "stop_mark_input_bit"]) {
    const b = v[p];
    if (b !== null && b !== undefined && !(Number.isInteger(b) && (b as number) >= 0 && (b as number) <= 15)) err[p] = "An input bit 0-15";
  }
  const st = v.start_mark_input_bit;
  if (typeof st === "number" && st === v.stop_mark_input_bit) err.stop_mark_input_bit = "The same bit as the start input";
  const f = v.focus_light_port_bit;
  if (f !== null && f !== undefined && !(Number.isInteger(f) && (f as number) >= 1 && (f as number) <= 15 && f !== 8)) {
    err.focus_light_port_bit = "An output bit 1-15 other than 8";
  }
  if (v["axis.enabled"] === true && (v["axis.role"] ?? "z") === "z") {
    const spm = n("axis.steps_per_mm");
    if (spm !== undefined && !(spm > 0)) err["axis.steps_per_mm"] = "Must be more than 0";
    const lo = n("axis.min_mm");
    const hi = n("axis.max_mm");
    if (lo !== undefined && hi !== undefined && lo >= hi) err["axis.max_mm"] = "Must be above the lowest Z";
    if (spm !== undefined && lo !== undefined && hi !== undefined && (hi - lo) * spm > 65535) err["axis.max_mm"] = "The travel is more than 65535 steps";
  }
  // Limits only go down.
  for (const [p, cap] of Object.entries(limits)) {
    const x = n(p);
    if (typeof cap === "number" && x !== undefined) {
      if (p === "frequency_khz_min" ? x < cap : x > cap) err[p] = p === "frequency_khz_min" ? `Not below ${cap} (limits are never widened)` : `Not above ${cap} (limits are never raised)`;
    }
  }
  return err;
}

function setPath(out: Record<string, unknown>, path: string, value: unknown): void {
  const parts = path.split(".");
  let o = out;
  for (const k of parts.slice(0, -1)) {
    const next = obj(o[k]) ?? {};
    o[k] = next;
    o = next;
  }
  o[parts[parts.length - 1]] = value;
}

function same(a: unknown, b: unknown): boolean {
  if (typeof a === "number" && typeof b === "number") return Math.abs(a - b) <= 1e-9 * Math.max(1, Math.abs(a));
  return a === b;
}

/**
 * The JSON merge patch for `PUT /api/devices/{id}`: only fields whose edited value differs from the current one.
 * A cleared bit sends null (no input); the axis is sent whole (or null when turned off), since a new axis needs every
 * field. Returns null when nothing changed.
 */
export function profilePatch(values: ProfileValues, edits: ProfileValues): Record<string, unknown> | null {
  const patch: Record<string, unknown> = {};
  let axisTouched = false;
  for (const [path, val] of Object.entries(edits)) {
    if (val === undefined || same(val, values[path])) continue;
    if (path.startsWith("axis.")) {
      axisTouched = true;
      continue;
    }
    if (path === "red_light_port_bit" || path === "laser_enable_port_bit") continue;
    setPath(patch, path, val);
  }
  if (axisTouched) {
    const m = merged(values, edits);
    if (m["axis.enabled"] !== true) patch.axis = null;
    else {
      const pick = (k: keyof typeof AXIS_DEFAULTS) => (m[`axis.${k}`] ?? AXIS_DEFAULTS[k]) as never;
      patch.axis = { role: pick("role"), steps_per_mm: pick("steps_per_mm"), min_mm: pick("min_mm"), max_mm: pick("max_mm"), invert: pick("invert") };
    }
  }
  return Object.keys(patch).length ? patch : null;
}

/**
 * The field an engine refusal names (`machine::edit_profile` and `MachineProfile::validate` start their messages with
 * the field, e.g. "max_power_pct: 60 is above the limit", "calibration: offset_y_mm = 30 mm is beyond ±..."), or null.
 */
export function errorField(message: string): string | null {
  const words = new Set(message.split(/[^A-Za-z0-9_.]+/).flatMap((w) => [w, ...w.split(".")]));
  let best: { path: string; score: number } | null = null;
  for (const path of allPaths()) {
    const parts = path.split(".");
    if (!words.has(parts[parts.length - 1]) && !words.has(path)) continue;
    const score = (words.has(path) ? 10 : 0) + parts.filter((p) => words.has(p)).length;
    if (!best || score > best.score) best = { path, score };
  }
  return best?.path ?? null;
}

/** Display text for a value in the read-only rows. */
export function valueText(v: unknown): string {
  if (v === undefined) return "not reported";
  if (v === null) return "none";
  if (typeof v === "boolean") return v ? "on" : "off";
  if (typeof v === "number") return String(Number(v.toFixed(6)));
  return String(v);
}
