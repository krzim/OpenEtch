// English strings of touch and canvas input: the touch HUD, long-press, row buttons and the trackpad (owner: P3,
// docs/ux/ui-plan-2026-11.md). Spread into EN (en.ts). `{name}` marks a placeholder.

export const EN_TOUCH = {
  // Canvas tool bars (components/EditTools.tsx).
  "touch.undoPoint": "Undo point",
  "touch.insertNode": "Insert node",
  "touch.tipInsertNode": "Add a node in the middle of the selected segment",
  "touch.selectSegmentToInsert": "Select two neighbouring nodes, or one node, to insert a node after it.",

  // The modifier bar (touch/TouchHud.tsx): a tap latches a toggle for the next gesture, a double-tap locks it.
  "touch.modBar": "Modifier keys",
  "touch.modHint": "Tap: next gesture only · double-tap: keep on",
  "touch.modAdd": "Add",
  "touch.modAddSub": "constrain",
  "touch.modToggle": "Toggle",
  "touch.modToggleSub": "no snap",
  "touch.modGuides": "Guides",
  "touch.modSquare": "Square",
  "touch.modFromCentre": "From centre",
  "touch.modNoSnap": "No snap",
  "touch.mod15": "15° steps",
  "touch.mod5": "5° steps",
  "touch.modAddNode": "Add",
  "touch.tipShift": "Shift: add to the selection; while dragging, keep to horizontal, vertical or 45°",
  "touch.tipCtrl": "Ctrl: toggle the selection; while dragging, snapping off",
  "touch.tipAlt": "Alt: snap to guides only",
  "touch.tipSquare": "Shift: a square or circle",
  "touch.tipFromCentre": "Ctrl: draw from the centre",
  "touch.tipNoSnap": "Ctrl: snapping off",
  "touch.tip15": "Shift: 15° steps",
  "touch.tip5": "Ctrl: 5° steps",
  "touch.tipAddNode": "Shift: add nodes to the node selection",
  "touch.locked": "{name} (locked)",

  // The selection action bar.
  "touch.actionBar": "Selection actions",
  "touch.copy": "Copy",
  "touch.paste": "Paste",
  "touch.duplicate": "Duplicate",
  "touch.delete": "Delete",
  "touch.group": "Group",
  "touch.ungroup": "Ungroup",
  "touch.rotate90": "Rotate 90°",
  "touch.flipH": "Flip horizontal",
  "touch.flipV": "Flip vertical",
  "touch.more": "More…",
  "touch.allActions": "All actions…",

  // The nudge pad (components/PropertiesPanel.tsx, coarse pointers).
  "touch.nudge": "Nudge",
  "touch.nudgeLeft": "Nudge left",
  "touch.nudgeRight": "Nudge right",
  "touch.nudgeUp": "Nudge up",
  "touch.nudgeDown": "Nudge down",
  "touch.nudgeStep": "Step",
  "touch.stepNormal": "Normal",
  "touch.stepLarge": "Large",
  "touch.stepSmall": "Small",
  "touch.stepTiny": "Tiny",

  // Rows and lists.
  "touch.rowMenu": "More actions",
  "touch.rename": "Rename",
  "touch.insert": "Insert",
  "touch.assign": "Assign",
  "touch.reorder": "Drag to reorder",
  "touch.moveUp": "Move up",
  "touch.moveDown": "Move down",
  "touch.expand": "Expand",
  "touch.collapse": "Collapse",
  "touch.open": "Open",

  // File pickers without `accept` (iOS): the type is checked after the pick.
  "touch.wrongFileType": "{name} is not a file this can open (expected {types}).",
};
