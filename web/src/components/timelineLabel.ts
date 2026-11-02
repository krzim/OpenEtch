// Friendly timeline chip labels (docs/ux/ui-plan-2026-11.md 1.4 "Copy"): "Rectangle", "Move", Text "Hello" instead of
// the command id ("rect", "transform +1"), with an icon per kind of step. Built from the step's command ids and the
// engine's summary ("added text t3 \"Hello\"", "rotate 15° on o1"); an unknown command keeps its short label. Pure, so
// `npm test` covers it.

import type { TimelineStep } from "../api/history.ts";
import type { T } from "../workspace/i18n.ts";

/** An icon name of the registry (components/ui.tsx ICONS). */
export type StepIcon = string;

export interface StepLabel {
  /** What the chip says. */
  text: string;
  icon: StepIcon;
}


/** The first quoted string in a summary ("added text t3 \"Hello\"" -> Hello), shortened. */
function quoted(summary: string): string | null {
  const m = /"([^"]*)"/.exec(summary);
  if (!m) return null;
  const s = m[1].trim();
  return s.length > 24 ? `${s.slice(0, 23)}…` : s;
}

/** Label and icon for one command id, given the step's summary (for transforms, ellipses and texts). */
function labelFor(cmd: string, summary: string, t: T): StepLabel | null {
  const sum = summary.toLowerCase();
  switch (cmd) {
    case "shape.rect":
      return { text: t("tlx.rect"), icon: "rect" };
    case "shape.ellipse": {
      const m = /rx ([\d.]+) ry ([\d.]+)/.exec(sum);
      return m && m[1] === m[2] ? { text: t("tlx.circle"), icon: "circle" } : { text: t("tlx.ellipse"), icon: "ellipse" };
    }
    case "shape.path":
      return { text: t("tlx.path"), icon: "path" };
    case "shape.text":
    case "text.add":
    case "text.create": {
      const q = quoted(summary);
      return { text: q !== null ? t("tlx.textNamed", { text: q }) : t("tlx.text"), icon: "type" };
    }
    case "text.update":
    case "text.apply_path":
    case "text.detach_path":
    case "text.refresh":
      return { text: t("tlx.editText"), icon: "type" };
    case "text.convert_to_path":
      return { text: t("tlx.textToPath"), icon: "path" };
    case "object.transform": {
      // The first part of "move [1, 2], rotate 15° on o1" names the step.
      if (/^\s*rotate/.test(sum)) return { text: t("tlx.rotate"), icon: "rotate" };
      if (/^\s*scale/.test(sum)) return { text: t("tlx.scale"), icon: "scale" };
      if (/^\s*mirror/.test(sum)) return { text: t("tlx.flip"), icon: "flipH" };
      if (/^\s*shear/.test(sum)) return { text: t("tlx.shear"), icon: "scale" };
      return { text: t("tlx.move"), icon: "move" };
    }
    case "object.delete":
      return { text: t("tlx.delete"), icon: "delete" };
    case "object.duplicate":
      return { text: t("tlx.duplicate"), icon: "copy" };
    case "object.rename": {
      const q = quoted(summary);
      return { text: q !== null ? t("tlx.renameTo", { name: q }) : t("tlx.rename"), icon: "tag" };
    }
    case "object.set_layer":
      return { text: t("tlx.setLayer"), icon: "layers" };
    case "object.move":
      return { text: t("tlx.reorder"), icon: "layers" };
    case "object.align":
      return { text: t("tlx.align"), icon: "alignLeft" };
    case "object.distribute":
      return { text: t("tlx.distribute"), icon: "distH" };
    case "group.create":
      return { text: t("tlx.group"), icon: "group" };
    case "group.ungroup":
      return { text: t("tlx.ungroup"), icon: "ungroup" };
    case "stock.define":
    case "stock.move":
    case "stock.clear":
      return { text: t("tlx.stock"), icon: "stock" };
    case "art.import_svg":
    case "art.import_dxf":
    case "art.import_image":
      return { text: t("tlx.import"), icon: "import" };
    case "art.place":
      return { text: t("tlx.place"), icon: "place" };
    case "art.array":
      return { text: t("tlx.array"), icon: "array" };
    // The Tools menu extras merged from main (tools/toolsApi.ts TOOLS_CMD).
    case "tabs.add":
      return { text: t("tlx.addTab"), icon: "plus" };
    case "tabs.move":
      return { text: t("tlx.moveTab"), icon: "move" };
    case "tabs.remove":
    case "tabs.clear":
      return { text: t("tlx.removeTabs"), icon: "minus" };
    case "path.dogbone":
      return { text: t("tlx.dogbone"), icon: "path" };
    case "path.resize_slots":
      return { text: t("tlx.resizeSlots"), icon: "path" };
    case "image.trace_many":
    case "image.trace_files":
      return { text: t("tlx.trace"), icon: "image" };
    case "doc.new":
      return { text: t("tlx.newDoc"), icon: "file" };
    case "doc.rename":
      return { text: t("tlx.renameDoc"), icon: "file" };
  }
  // Every importer (SVG, DXF, images, and the formats merged from main: .rd, G-code, PDF...) is an Import step; the chip
  // read "import_rd" after an .rd import.
  if (cmd.startsWith("art.import_") || cmd.startsWith("doc.import")) return { text: t("tlx.import"), icon: "import" };
  if (cmd.startsWith("layer.")) {
    if (cmd === "layer.add" || cmd === "layer.tool" || cmd === "layer.sub_add" || cmd === "layer.sub_duplicate") return { text: t("tlx.addLayer"), icon: "layers" };
    if (cmd === "layer.delete" || cmd === "layer.sub_delete") return { text: t("tlx.deleteLayer"), icon: "layers" };
    return { text: t("tlx.layerSettings"), icon: "layers" };
  }
  if (cmd.startsWith("node.") || cmd.startsWith("path.") || cmd.startsWith("boolean.")) return { text: t("tlx.editPath"), icon: "path" };
  return null;
}

/** The chip's label and icon for a timeline step: friendly when the command is known, else its short label. */
export function timelineLabel(step: Pick<TimelineStep, "label" | "commands" | "summary">, t: T): StepLabel {
  const cmds = step.commands
    .split(",")
    .map((c) => c.trim())
    .filter(Boolean);
  const first = cmds[0] ?? "";
  const known = labelFor(first, step.summary.split(";")[0] ?? "", t);
  if (!known) return { text: step.label || first || "?", icon: "pencil" };
  return cmds.length > 1 ? { ...known, text: `${known.text} +${cmds.length - 1}` } : known;
}
