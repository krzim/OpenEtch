// LightBurn's colour palette (Reference > UI > Color Palette): 30 colour layers C00..C29 plus the tool layers T1 and T2.
// In LightBurn the colour is the layer; here layers have free colours, so a palette swatch stands for the layer whose
// colour is that swatch's (preferring one also named C05 etc.), and clicking a swatch with no such layer creates it.
// Colours from crates/lbrn/src/palette.rs, which keeps LightBurn files round-tripping with the same indices.

import type { Layer } from "../api/types.ts";

export interface PaletteEntry {
  /** LightBurn layer index: 0..29 colours, 30 = T1, 31 = T2. */
  index: number;
  /** "C00".."C29", "T1", "T2". */
  name: string;
  color: string;
  /** Tool layers never output (LightBurn). */
  tool: boolean;
}

export const LB_COLORS = [
  "#000000", "#0000ff", "#ff0000", "#00e000", "#d0d000", "#ff8000", "#00e0e0", "#ff00ff", "#b4b4b4", "#0000a0",
  "#a00000", "#00a000", "#a0a000", "#c08000", "#00a0ff", "#a000a0", "#808080", "#7d87b9", "#bb7784", "#4a6fe3",
  "#d33f6a", "#8cd78c", "#f0b98d", "#f6c4e1", "#fa9ed4", "#500a78", "#b45a00", "#004754", "#86fa88", "#ffdb66",
];
export const LB_TOOL_COLORS = ["#f36926", "#0c96d9"];

export const PALETTE: PaletteEntry[] = [
  ...LB_COLORS.map((color, index) => ({ index, name: `C${String(index).padStart(2, "0")}`, color, tool: false })),
  { index: 30, name: "T1", color: LB_TOOL_COLORS[0], tool: true },
  { index: 31, name: "T2", color: LB_TOOL_COLORS[1], tool: true },
];

/** "#ABC" / "#aabbcc" -> "#aabbcc"; other CSS colours lower-cased as they are. */
export function normColor(c: string): string {
  const t = c.trim().toLowerCase();
  const m = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/.exec(t);
  return m ? `#${m[1]}${m[1]}${m[2]}${m[2]}${m[3]}${m[3]}` : t;
}

/**
 * The layer a swatch stands for: T1 / T2 the document's tool layer of that number (engines with `Layer.tool`); else the
 * same colour, preferring the one also named like the swatch (and, for a colour swatch, not a tool layer); else null.
 */
export function layerForEntry(layers: readonly Layer[], e: PaletteEntry): Layer | null {
  if (e.tool) {
    const tool = layers.find((l) => l.tool === e.name);
    if (tool) return tool;
    // A document from an engine with tool layers: the T1 / T2 swatch only stands for the real tool layer.
    if (layers.some((l) => l.tool)) return null;
  }
  const same = layers.filter((l) => normColor(l.color) === e.color && (e.tool || !l.tool));
  return same.find((l) => l.name.trim().toUpperCase() === e.name) ?? same[0] ?? null;
}

/** The palette entry of a layer's colour, or null when it is not a LightBurn colour. */
export function entryForLayer(l: Pick<Layer, "color">): PaletteEntry | null {
  const c = normColor(l.color);
  return PALETTE.find((e) => e.color === c) ?? null;
}

/** What clicking swatch `e` does with the selection: move it to an existing layer, or create the layer first. */
export type AssignPlan =
  | { kind: "none"; reason: "no-selection" | "already" }
  | { kind: "existing"; layer: string }
  | { kind: "create"; name: string; color: string; output: boolean }
  /** Add tool layer T1 / T2 with `layer.tool` (engines that have it), then move the selection to it. */
  | { kind: "tool"; tool: "T1" | "T2" };

/** `toolCmd`: the engine has `layer.tool`, so a missing T1 / T2 is made a real tool layer. */
export function assignPlan(
  layers: readonly Layer[],
  objects: readonly { id: string; layer: string }[],
  ids: readonly string[],
  e: PaletteEntry,
  toolCmd = false,
): AssignPlan {
  if (ids.length === 0) return { kind: "none", reason: "no-selection" };
  const target = layerForEntry(layers, e);
  if (!target && e.tool && toolCmd) return { kind: "tool", tool: e.name as "T1" | "T2" };
  if (!target) return { kind: "create", name: e.name, color: e.color, output: !e.tool };
  const set = new Set(ids);
  const all = objects.filter((o) => set.has(o.id)).every((o) => o.layer === target.id);
  return all ? { kind: "none", reason: "already" } : { kind: "existing", layer: target.id };
}
