// LightBurn's hotkeys.json (Edit hotkeys > Export, and the "Hotkeys" item of a LightBurn bundle) to and from
// OpenEtch's key map (hotkeys.ts). LightBurn 2.2 writes only the actions whose keys were changed, as an object of its
// Qt action names and Qt key texts:
//
//   { "actionDistribute_Horizontal_Centered": "Ctrl+Alt+Shift+D" }
//
// An empty text is an action with no key. Only actions with a clear OpenEtch counterpart are mapped (LIGHTBURN_ACTIONS,
// names as LightBurn 2.2's executable lists them); the rest are reported as skipped. STOP is never changed by an
// import: its keys are set in Edit hotkeys only.

import { ACTION_BY_ID, bindChord, chordsFor, normalizeChord, parseChord, RESERVED } from "./hotkeys.ts";

/** LightBurn action name -> OpenEtch action id. */
export const LIGHTBURN_ACTIONS: Readonly<Record<string, string>> = {
  actionImport: "file.import",
  actionSave_As: "file.saveAs",
  actionExport: "file.export",
  actionPrint: "file.print",
  actionPrint_Color: "file.printColors",
  actionDevice_Management: "window.devices",
  actionTrace_Image: "image.trace",
  actionAdjust_Image: "image.adjust",
  actionConvert_to_Bitmap: "image.toBitmap",
  actionUndo: "edit.undo",
  actionRedo: "edit.redo",
  actionSelect_All: "edit.selectAll",
  actionInvert_Selection: "edit.invert",
  actionCut: "edit.cut",
  actionCopy: "edit.copy",
  actionPaste: "edit.paste",
  actionPaste_in_place: "edit.pasteInPlace",
  actionDuplicate: "edit.duplicate",
  actionDelete: "edit.delete",
  actionConvert_to_Path: "edit.convertToPath",
  actionSelect_open_vectors: "edit.selectOpen",
  actionRectangle: "tool.rect",
  actionEllipse: "tool.ellipse",
  actionEdit_Text: "tool.text",
  actionCreate_BarCode: "tool.barcode",
  actionDraw_Lines: "tool.line",
  actionEditNodes: "tool.nodes",
  actionWeld_Shapes: "tool.weld",
  actionUnion: "tool.union",
  actionSubtract: "tool.subtract",
  actionIntersection: "tool.intersect",
  actionCut_Shapes: "tool.cutShapes",
  actionScissors: "tool.trim",
  actionMeasure: "tool.measure",
  actionMirror_Across_Line: "tool.mirrorLine",
  actionOffset: "tool.offset",
  actionBreak_apart: "tool.breakApart",
  actionAuto_Join_selected_shapes: "tool.autoJoin",
  actionDelete_Duplicate_Shapes: "tool.deleteDuplicates",
  actionOptimize_selected_shapes: "tool.optimize",
  actionStart_Points: "tool.startPoint",
  actionWarp_Selection: "tools.warp",
  actionDeform_Selection: "tools.deform",
  actionGenerate_Tangent_Circle: "tools.tangentCircle",
  actionGroup: "arrange.group",
  actionUngroup: "arrange.ungroup",
  actionFlip_Horizontal: "arrange.flipH",
  actionFlip_Vertical: "arrange.flipV",
  actionAlign_Left: "arrange.alignLeft",
  actionAlign_Right: "arrange.alignRight",
  actionAlign_Top: "arrange.alignTop",
  actionAlign_Bottom: "arrange.alignBottom",
  actionAlign_Horizontal_Centers: "arrange.alignHCenter",
  actionAlign_Vertical_Centers: "arrange.alignVCenter",
  actionAlign_Centers: "arrange.alignCenters",
  actionMove_to_Page_Center: "arrange.centerField",
  actionDistribute_Horizontal_Spaced: "arrange.distHSpaced",
  actionDistribute_Horizontal_Centered: "arrange.distHCentered",
  actionDistribute_Vertical_Spaced: "arrange.distVSpaced",
  actionDistribute_Vertical_Centered: "arrange.distVCentered",
  actionMove_H_Together: "arrange.moveHTogether",
  actionMove_V_Together: "arrange.moveVTogether",
  actionDock_Left: "arrange.dockLeft",
  actionDock_Right: "arrange.dockRight",
  actionDock_Up: "arrange.dockUp",
  actionDock_Down: "arrange.dockDown",
  actionPush_forward_in_draw_order: "arrange.forward",
  actionPush_backward_in_draw_order: "arrange.backward",
  actionPush_to_front: "arrange.front",
  actionPush_to_back: "arrange.back",
  actionToggle_Wireframe_Filled: "view.wireframe",
  actionPreview: "view.preview",
  actionZoom_In: "view.zoomIn",
  actionZoom_Out: "view.zoomOut",
  actionZoom_to_Page: "view.zoomPage",
  actionToggle_Side_Panels: "window.sidePanels",
  actionFrame_Laser: "laser.frameSquare",
  actionRubber_Band_Frame_Laser: "laser.frameOutline",
  actionStartLaser: "laser.requestRun",
  actionPause_Laser: "laser.pauseResume",
  actionPosition_Laser: "laser.position",
  actionRotary_Setup: "laser.rotary",
  actionMaterial_Test: "laser.materialTest",
  actionInterval_Test: "laser.intervalTest",
  actionFocus_Test: "laser.focusTest",
  actionPrint_and_Cut: "laser.printCut",
};

/** LightBurn's STOP: an import never moves STOP's keys (they are set in Edit hotkeys only). */
const LIGHTBURN_STOP = "actionStop_Laser";

const BY_ID: ReadonlyMap<string, string> = new Map(Object.entries(LIGHTBURN_ACTIONS).map(([lb, id]) => [id, lb]));

/** Qt key names that differ from OpenEtch's chord notation (Qt -> OpenEtch). */
const QT_KEYS: Readonly<Record<string, string>> = { PgDown: "PgDn", Return: "Enter", Escape: "Esc", Delete: "Del" };
const OE_KEYS: Readonly<Record<string, string>> = { PgDn: "PgDown", Enter: "Return" };

const join = (p: { ctrl: boolean; alt: boolean; shift: boolean }, key: string) =>
  [p.ctrl ? "Ctrl" : "", p.alt ? "Alt" : "", p.shift ? "Shift" : "", key].filter(Boolean).join("+");

/** A Qt key text as an OpenEtch chord, or null (a sequence of several chords, "Ctrl+K, Ctrl+C", or a Meta key). */
export function chordFromQt(text: string): string | null {
  const t = text.trim();
  if (!t || t.includes(", ")) return null;
  const p = parseChord(t);
  return p ? normalizeChord(join(p, QT_KEYS[p.key] ?? p.key)) : null;
}

/** An OpenEtch chord as Qt key text. */
export function chordToQt(chord: string): string {
  const p = parseChord(chord);
  return p ? join(p, OE_KEYS[p.key] ?? p.key) : chord;
}

/** What importing a LightBurn hotkeys.json does: the new overrides, the actions set, and what was skipped (why). */
export interface HotkeyImport {
  overrides: Record<string, string[]>;
  applied: string[];
  skipped: string[];
}

/** Most entries read from one file (LightBurn has fewer than 400 actions). */
const MAX_ENTRIES = 500;

/** Apply LightBurn's hotkeys (`{actionName: keyText}`) to `overrides`. Each mapped action gets exactly that key. */
export function importLightBurnHotkeys(raw: unknown, overrides: Readonly<Record<string, string[]>>): HotkeyImport {
  let next: Record<string, string[]> = { ...overrides };
  const applied: string[] = [];
  const skipped: string[] = [];
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return { overrides: next, applied, skipped };
  for (const [name, value] of Object.entries(raw as Record<string, unknown>).slice(0, MAX_ENTRIES)) {
    const id = LIGHTBURN_ACTIONS[name];
    const text = typeof value === "string" ? value : null;
    if (name === LIGHTBURN_STOP) {
      skipped.push(`${name}: STOP keeps its keys (set them in Edit hotkeys)`);
      continue;
    }
    if (!id) {
      skipped.push(`${name.slice(0, 80)}: no matching OpenEtch action`);
      continue;
    }
    const a = ACTION_BY_ID.get(id);
    if (!a || a.fixed) {
      skipped.push(`${name}: ${a?.label ?? id} keeps its keys (set them in Edit hotkeys)`);
      continue;
    }
    if (text === null) {
      skipped.push(`${name}: not a key`);
      continue;
    }
    if (text.trim() === "") {
      next = { ...next, [id]: [] };
      applied.push(id);
      continue;
    }
    const chord = chordFromQt(text);
    if (!chord || RESERVED.has(chord)) {
      skipped.push(`${name}: "${text.slice(0, 40)}" ${chord ? "is kept by the browser" : "is not a key OpenEtch can use"}`);
      continue;
    }
    const bound = bindChord({ ...next, [id]: [] }, id, chord);
    if (!bound) {
      skipped.push(`${name}: "${text.slice(0, 40)}" cannot be bound`);
      continue;
    }
    next = bound;
    applied.push(id);
  }
  return { overrides: next, applied, skipped };
}

/** OpenEtch's overrides as LightBurn's hotkeys.json object (only the actions LightBurn has; the first key of each). */
export function exportLightBurnHotkeys(overrides: Readonly<Record<string, string[]>>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const id of Object.keys(overrides).sort()) {
    const lb = BY_ID.get(id);
    const a = ACTION_BY_ID.get(id);
    if (!lb || !a) continue;
    const first = chordsFor(a, overrides)[0];
    out[lb] = first ? chordToQt(first) : "";
  }
  return out;
}
