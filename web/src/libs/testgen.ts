// LightBurn's Material Test, Interval Test and Focus Test generators (engine commands `generate.material_test`,
// `generate.interval_test`, `generate.focus_test`, crates/commands/src/builtin/testgen.rs): the dialog values, their
// checks (the engine's ranges), the command parameters and the preview the dialogs draw (the same layout the engine
// builds: boxes, swatches and lines with their labels, numbered in LightBurn's safe burn order). Pure; tested.

export type TestParam = "power" | "speed" | "frequency" | "pulse_width" | "interval" | "passes";

export const PARAMS: { value: TestParam; label: string; unit: string }[] = [
  { value: "power", label: "Power", unit: "%" },
  { value: "speed", label: "Speed", unit: "mm/s" },
  { value: "frequency", label: "Frequency", unit: "kHz" },
  { value: "pulse_width", label: "Q-Pulse width", unit: "ns" },
  { value: "interval", label: "Interval", unit: "mm" },
  { value: "passes", label: "Passes", unit: "" },
];

/** Engine limits. */
export const MAX_COUNT = 20;
export const MAX_STEPS = 50;
export const MAX_PASSES = 1000;

export interface Axis {
  param: TestParam;
  min: number;
  max: number;
  count: number;
}

/** Base laser settings of the test (LightBurn's Edit Material Setting / Edit Text Setting), partial. */
export interface BaseSettings {
  mode: "fill" | "line" | "fill_then_line" | "offset_fill";
  power_pct: number;
  speed_mm_s: number;
  frequency_khz: number;
  pulse_width_ns: number | null;
  passes: number;
  interval_mm: number;
}

export interface MaterialTest {
  rows: Axis;
  columns: Axis;
  cell_width_mm: number;
  cell_height_mm: number;
  spacing_mm: number;
  center: [number, number];
  base: BaseSettings;
  labels: boolean;
  label_height_mm: number;
  label_power_pct: number;
  label_speed_mm_s: number;
  border: boolean;
  border_power_pct: number;
  border_speed_mm_s: number;
  title: string;
}

/** LightBurn's defaults: 10 x 10 boxes, speed down the rows, power across. */
export const DEFAULT_MATERIAL_TEST: MaterialTest = {
  rows: { param: "speed", min: 500, max: 5000, count: 10 },
  columns: { param: "power", min: 10, max: 100, count: 10 },
  cell_width_mm: 5,
  cell_height_mm: 5,
  spacing_mm: 1,
  center: [0, 0],
  base: { mode: "fill", power_pct: 30, speed_mm_s: 1000, frequency_khz: 30, pulse_width_ns: null, passes: 1, interval_mm: 0.05 },
  labels: true,
  label_height_mm: 1.5,
  label_power_pct: 30,
  label_speed_mm_s: 1000,
  border: false,
  border_power_pct: 30,
  border_speed_mm_s: 1000,
  title: "",
};

/** Sensible ranges when a parameter is picked for an axis. */
export const PARAM_DEFAULT_RANGE: Record<TestParam, [number, number]> = {
  power: [10, 100],
  speed: [500, 5000],
  frequency: [20, 200],
  pulse_width: [2, 200],
  interval: [0.02, 0.2],
  passes: [1, 10],
};

/** The values along an axis (the engine's `TestAxis::values`). */
export function axisValues(a: Axis): number[] {
  const n = Math.round(a.count);
  if (!(n >= 1)) return [];
  return Array.from({ length: n }, (_, i) => {
    const v = n === 1 ? a.min : a.min + ((a.max - a.min) * i) / (n - 1);
    return a.param === "passes" ? Math.round(v) : v;
  });
}

/** A label like the engine marks: at most 3 decimals, no trailing zeros. */
export function show(v: number): string {
  const r = Math.round(v * 1000) / 1000;
  return Object.is(r, -0) ? "0" : String(r);
}

function axisProblem(a: Axis, which: string): string | null {
  if (!Number.isFinite(a.min) || !Number.isFinite(a.max)) return `${which}: give a minimum and a maximum.`;
  if (!(Number.isInteger(a.count) && a.count >= 1 && a.count <= MAX_COUNT)) return `${which}: count is 1 to ${MAX_COUNT}.`;
  const lo = Math.min(a.min, a.max);
  const hi = Math.max(a.min, a.max);
  switch (a.param) {
    case "power":
      if (lo < 0 || hi > 100) return `${which}: power is 0 to 100 %.`;
      break;
    case "speed":
    case "frequency":
    case "pulse_width":
    case "interval":
      if (!(lo > 0)) return `${which}: values must be more than 0.`;
      break;
    case "passes":
      if (axisValues(a).some((v) => v < 1 || v > MAX_PASSES)) return `${which}: passes are 1 to ${MAX_PASSES}.`;
      break;
  }
  return null;
}

/**
 * Q-pulse values the source cannot take: a MOPA source only accepts the widths of its table, so a box between them
 * would be refused when the job is planned.
 */
export function offTablePulseWidths(a: Axis, widths: readonly number[]): number[] {
  if (a.param !== "pulse_width" || widths.length === 0) return [];
  return axisValues(a).filter((v) => !widths.some((w) => Math.abs(w - v) < 1e-6));
}

/** Why the engine would refuse this test (or the job planned from it), or null. */
export function materialTestProblem(t: MaterialTest, pulseWidths: readonly number[] = []): string | null {
  if (t.rows.param === t.columns.param) return "Rows and columns must test different settings.";
  const a = axisProblem(t.rows, "Rows") ?? axisProblem(t.columns, "Columns");
  if (a) return a;
  const off = [...offTablePulseWidths(t.rows, pulseWidths), ...offTablePulseWidths(t.columns, pulseWidths)];
  if (off.length) return `The source has no ${off.slice(0, 3).map(show).join(", ")} ns pulse width: pick a range and count that land on its table (${pulseWidths.join(", ")} ns).`;
  if (!(t.cell_width_mm > 0 && t.cell_height_mm > 0)) return "Boxes need a width and height above 0.";
  if (!(t.spacing_mm >= 0)) return "Spacing cannot be negative.";
  if (t.labels && !(t.label_height_mm > 0)) return "Label height must be more than 0.";
  if (!(t.base.power_pct >= 0 && t.base.power_pct <= 100)) return "Base power is 0 to 100 %.";
  if (!(t.base.speed_mm_s > 0 && t.base.frequency_khz > 0)) return "Base speed and frequency must be more than 0.";
  if (t.title.length > 200) return "The title is at most 200 characters.";
  return null;
}

function basePatch(b: BaseSettings): Record<string, unknown> {
  const s: Record<string, unknown> = {
    mode: b.mode,
    power_pct: b.power_pct,
    speed_mm_s: b.speed_mm_s,
    frequency_khz: b.frequency_khz,
    passes: Math.max(1, Math.round(b.passes)),
    hatch: { interval_mm: b.interval_mm },
  };
  if (b.pulse_width_ns !== null) s.pulse_width_ns = b.pulse_width_ns;
  return s;
}

const lineSettings = (power: number, speed: number) => ({ mode: "line", power_pct: power, speed_mm_s: speed });

/** `generate.material_test` parameters. */
export function materialTestParams(t: MaterialTest): Record<string, unknown> {
  const p: Record<string, unknown> = {
    rows: { ...t.rows, count: Math.round(t.rows.count) },
    columns: { ...t.columns, count: Math.round(t.columns.count) },
    cell_width_mm: t.cell_width_mm,
    cell_height_mm: t.cell_height_mm,
    spacing_mm: t.spacing_mm,
    center: t.center,
    base: basePatch(t.base),
    labels: t.labels,
    label_height_mm: t.label_height_mm,
    label_settings: lineSettings(t.label_power_pct, t.label_speed_mm_s),
    border: t.border,
  };
  if (t.border) p.border_settings = lineSettings(t.border_power_pct, t.border_speed_mm_s);
  if (t.title.trim()) p.title = t.title.trim();
  return p;
}

export interface PreviewBox {
  x: number;
  y: number;
  w: number;
  h: number;
  /** Burn order, 1 first. */
  order: number;
  /** Shade 0 (light) .. 1 (dark): how hard the box is likely to burn. */
  shade: number;
  label: string;
}

export interface PreviewText {
  x: number;
  y: number;
  text: string;
  anchor: "start" | "middle" | "end";
  size: number;
  rotate?: number;
}

export interface TestPreview {
  boxes: PreviewBox[];
  texts: PreviewText[];
  lines: { x1: number; y1: number; x2: number; y2: number; label: string }[];
  /** Field mm, +Y up: [x0, y0, x1, y1]. */
  bounds: [number, number, number, number];
  border: PreviewBox | null;
}

/** LightBurn's safe order: highest speed, lowest power, lowest interval, fewest passes first (the engine's `rank`/`risk`). */
const RANK: Record<TestParam, number> = { speed: 0, power: 1, interval: 2, passes: 3, frequency: 4, pulse_width: 5 };
const risk = (p: TestParam, v: number) => (p === "speed" ? -v : v);

/** How strongly a value burns, 0..1 along its axis (speed and interval burn less when larger). */
function burn(p: TestParam, v: number, lo: number, hi: number): number {
  if (hi === lo) return 0.5;
  const f = (v - lo) / (hi - lo);
  return p === "speed" || p === "interval" ? 1 - f : f;
}

export function materialTestPreview(t: MaterialTest): TestPreview {
  const rv = axisValues(t.rows);
  const cv = axisValues(t.columns);
  const { cell_width_mm: w, cell_height_mm: h, spacing_mm: gap } = t;
  const [cx, cy] = t.center;
  const gw = cv.length * w + Math.max(0, cv.length - 1) * gap;
  const gh = rv.length * h + Math.max(0, rv.length - 1) * gap;
  const left = cx - gw / 2;
  const top = cy + gh / 2;
  const cells: { r: number; c: number; key: [number, number] }[] = [];
  rv.forEach((a, r) =>
    cv.forEach((b, c) => {
      const k = [
        [RANK[t.rows.param], risk(t.rows.param, a)],
        [RANK[t.columns.param], risk(t.columns.param, b)],
      ].sort((x, y) => x[0] - y[0]);
      cells.push({ r, c, key: [k[0][1], k[1][1]] });
    }),
  );
  const sorted = [...cells].sort((a, b) => a.key[0] - b.key[0] || a.key[1] - b.key[1]);
  const order = new Map(sorted.map((x, i) => [`${x.r},${x.c}`, i + 1]));
  const [rlo, rhi] = [Math.min(...rv), Math.max(...rv)];
  const [clo, chi] = [Math.min(...cv), Math.max(...cv)];
  const boxes: PreviewBox[] = cells.map(({ r, c }) => ({
    x: left + c * (w + gap),
    y: top - r * (h + gap) - h,
    w,
    h,
    order: order.get(`${r},${c}`) ?? 0,
    shade: (burn(t.rows.param, rv[r], rlo, rhi) + burn(t.columns.param, cv[c], clo, chi)) / 2,
    label: `${paramLabel(t.rows.param)} ${show(rv[r])} / ${paramLabel(t.columns.param)} ${show(cv[c])}`,
  }));
  const texts: PreviewText[] = [];
  const lh = t.label_height_mm;
  if (t.labels) {
    cv.forEach((v, c) => texts.push({ x: left + c * (w + gap) + w / 2, y: top + gap + lh / 2, text: show(v), anchor: "middle", size: lh }));
    texts.push({ x: cx, y: top + gap + 2.6 * lh, text: axisTitle(t.columns.param), anchor: "middle", size: lh });
    rv.forEach((v, r) => texts.push({ x: left - gap - 0.2 * lh, y: top - r * (h + gap) - h / 2 - lh / 2, text: show(v), anchor: "end", size: lh }));
    const widest = Math.max(1, ...rv.map((v) => show(v).length));
    texts.push({ x: left - gap - (widest * 0.7 + 1.2) * lh, y: cy, text: axisTitle(t.rows.param), anchor: "middle", size: lh, rotate: 90 });
    if (t.title.trim()) texts.push({ x: cx, y: top + gap + 4.4 * lh, text: t.title.trim(), anchor: "middle", size: lh * 1.4 });
  }
  const m = Math.max(gap, 1);
  const border = t.border ? { x: cx - gw / 2 - m, y: cy - gh / 2 - m, w: gw + 2 * m, h: gh + 2 * m, order: 0, shade: 0, label: "Border" } : null;
  const ext = t.labels ? (Math.max(1, ...rv.map((v) => show(v).length)) * 0.7 + 2) * lh + gap : 0;
  const x0 = Math.min(left - ext, border ? border.x : left);
  const x1 = Math.max(left + gw, border ? border.x + border.w : left + gw);
  const y0 = Math.min(cy - gh / 2, border ? border.y : cy - gh / 2);
  const y1 = Math.max(top + (t.labels ? gap + (t.title.trim() ? 5.4 : 3.4) * lh : 0), border ? border.y + border.h : top);
  return { boxes, texts, lines: [], bounds: [x0, y0, x1, y1], border };
}

export function paramLabel(p: TestParam): string {
  return PARAMS.find((x) => x.value === p)?.label ?? p;
}

/** The engine's axis titles ("Power (%)"). */
export function axisTitle(p: TestParam): string {
  return { power: "Power (%)", speed: "Speed (mm/s)", frequency: "Frequency (kHz)", pulse_width: "Q-pulse (ns)", interval: "Interval (mm)", passes: "Passes" }[p];
}

// ---- interval test --------------------------------------------------------------------------------------------------

export interface IntervalTest {
  speed_mm_s: number;
  power_pct: number;
  frequency_khz: number;
  pulse_width_ns: number | null;
  steps: number;
  min_interval_mm: number;
  max_interval_mm: number;
  size_mm: number;
  fill: "simple" | "dithered";
  center: [number, number];
  labels: boolean;
}

export const DEFAULT_INTERVAL_TEST: IntervalTest = {
  speed_mm_s: 1000,
  power_pct: 30,
  frequency_khz: 30,
  pulse_width_ns: null,
  steps: 5,
  min_interval_mm: 0.08,
  max_interval_mm: 0.16,
  size_mm: 10,
  fill: "simple",
  center: [0, 0],
  labels: true,
};

export function intervalValues(t: IntervalTest): number[] {
  const n = Math.round(t.steps);
  if (!(n >= 2)) return [];
  return Array.from({ length: n }, (_, k) => Math.round((t.min_interval_mm + ((t.max_interval_mm - t.min_interval_mm) * k) / (n - 1)) * 1e6) / 1e6);
}

export function intervalTestProblem(t: IntervalTest): string | null {
  if (!(Number.isInteger(t.steps) && t.steps >= 2 && t.steps <= MAX_STEPS)) return `Steps are 2 to ${MAX_STEPS}.`;
  if (!(t.min_interval_mm > 0 && t.max_interval_mm > 0)) return "Intervals must be more than 0.";
  if (!(t.size_mm > 0)) return "The swatch size must be more than 0.";
  if (!(t.power_pct >= 0 && t.power_pct <= 100)) return "Power is 0 to 100 %.";
  if (!(t.speed_mm_s > 0 && t.frequency_khz > 0)) return "Speed and frequency must be more than 0.";
  return null;
}

export function intervalTestParams(t: IntervalTest): Record<string, unknown> {
  const base: Record<string, unknown> = { frequency_khz: t.frequency_khz };
  if (t.pulse_width_ns !== null) base.pulse_width_ns = t.pulse_width_ns;
  return {
    speed_mm_s: t.speed_mm_s,
    power_pct: t.power_pct,
    steps: Math.round(t.steps),
    min_interval_mm: t.min_interval_mm,
    max_interval_mm: t.max_interval_mm,
    size_mm: t.size_mm,
    fill: t.fill,
    center: t.center,
    base,
    labels: t.labels,
  };
}

export function intervalTestPreview(t: IntervalTest): TestPreview {
  const vals = intervalValues(t);
  const size = t.size_mm;
  const gap = Math.max(size * 0.3, 1);
  const lh = Math.min(Math.max(size * 0.25, 1), 4);
  const total = vals.length * size + Math.max(0, vals.length - 1) * gap;
  const top = t.center[1] + total / 2;
  const x = t.center[0] - (t.labels ? 2 * lh : 0);
  const boxes = vals.map((v, k) => ({
    x: x - size / 2,
    y: top - k * (size + gap) - size,
    w: size,
    h: size,
    order: k + 1,
    shade: vals.length > 1 ? 1 - k / (vals.length - 1) : 0.5,
    label: `Interval ${show(v)} mm`,
  }));
  const texts: PreviewText[] = t.labels ? vals.map((v, k) => ({ x: x + size / 2 + gap / 2, y: top - k * (size + gap) - size / 2 - lh / 2, text: show(v), anchor: "start" as const, size: lh })) : [];
  const right = x + size / 2 + (t.labels ? gap / 2 + lh * 4 : 0);
  return { boxes, texts, lines: [], bounds: [x - size / 2, top - total, right, top], border: null };
}

// ---- focus test -----------------------------------------------------------------------------------------------------

export interface FocusTest {
  start_z_mm: number;
  end_z_mm: number;
  steps: number;
  speed_mm_s: number;
  power_pct: number;
  frequency_khz: number;
  line_length_mm: number;
  spacing_mm: number;
  center: [number, number];
  labels: boolean;
}

export const DEFAULT_FOCUS_TEST: FocusTest = {
  start_z_mm: -2,
  end_z_mm: 2,
  steps: 8,
  speed_mm_s: 500,
  power_pct: 30,
  frequency_khz: 30,
  line_length_mm: 15,
  spacing_mm: 2.5,
  center: [0, 0],
  labels: true,
};

export function focusHeights(t: FocusTest): number[] {
  const n = Math.round(t.steps);
  if (!(n >= 1)) return [];
  return Array.from({ length: n + 1 }, (_, k) => t.start_z_mm + ((t.end_z_mm - t.start_z_mm) * k) / n);
}

export function focusTestProblem(t: FocusTest): string | null {
  if (!(Number.isInteger(t.steps) && t.steps >= 1 && t.steps <= MAX_STEPS)) return `Steps are 1 to ${MAX_STEPS}.`;
  if (!Number.isFinite(t.start_z_mm) || !Number.isFinite(t.end_z_mm)) return "Give a start and end Z.";
  if (!(t.line_length_mm > 0 && t.spacing_mm > 0)) return "Line length and spacing must be more than 0.";
  if (!(t.power_pct >= 0 && t.power_pct <= 100)) return "Power is 0 to 100 %.";
  if (!(t.speed_mm_s > 0 && t.frequency_khz > 0)) return "Speed and frequency must be more than 0.";
  return null;
}

export function focusTestParams(t: FocusTest): Record<string, unknown> {
  return {
    start_z_mm: t.start_z_mm,
    end_z_mm: t.end_z_mm,
    steps: Math.round(t.steps),
    speed_mm_s: t.speed_mm_s,
    power_pct: t.power_pct,
    line_length_mm: t.line_length_mm,
    spacing_mm: t.spacing_mm,
    center: t.center,
    base: { frequency_khz: t.frequency_khz },
    labels: t.labels,
  };
}

export function focusTestPreview(t: FocusTest): TestPreview {
  const zs = focusHeights(t);
  const pitch = t.spacing_mm;
  const lh = Math.min(Math.max(pitch * 0.5, 0.8), 3);
  const width = Math.max(0, zs.length - 1) * pitch;
  const left = t.center[0] - width / 2;
  const cy = t.center[1] + lh;
  const len = t.line_length_mm;
  const lines = zs.map((z, k) => ({ x1: left + k * pitch, y1: cy - len / 2, x2: left + k * pitch, y2: cy + len / 2, label: `Z ${show(z)} mm` }));
  const texts: PreviewText[] = t.labels ? zs.map((z, k) => ({ x: left + k * pitch, y: t.center[1] - len / 2 + lh / 2 - 0.5 * lh, text: show(z), anchor: "end" as const, size: lh, rotate: 90 })) : [];
  const labelW = t.labels ? lh * 5 : 0;
  return { boxes: [], texts, lines, bounds: [left - lh, cy - len / 2 - labelW, left + width + lh, cy + len / 2], border: null };
}

/** An SVG viewBox for a preview (field mm, +Y up drawn with a y-flip), with a margin. */
export function previewViewBox(p: TestPreview, margin = 2): string {
  const [x0, y0, x1, y1] = p.bounds;
  const w = Math.max(1, x1 - x0) + 2 * margin;
  const h = Math.max(1, y1 - y0) + 2 * margin;
  return `${x0 - margin} ${-(y1 + margin)} ${w} ${h}`;
}
