// English strings of the Fill Region tool (src/fill/): the ribbon button, the options bar in the context strip, the
// status and error lines, and the timeline label. Spread into EN (en.ts).

export const EN_FILL = {
  "fill.tool": "Fill Region",
  "fill.toolTitle": "Fill Region: click inside an area enclosed by lines to make a closed shape of exactly that area on the chosen layer (Shift+click collects several areas)",
  "fill.unsupported": "The engine does not have Fill Region (update it)",
  "fill.layer": "Layer",
  "fill.layerTitle": "The layer the new shapes go on (the current layer until you pick another)",
  "fill.gap": "Gap",
  "fill.gapTitle": "Close openings narrower than this between lines (0: the lines must meet)",
  "fill.holes": "Islands as holes",
  "fill.holesTitle": "Shapes inside the area (a pupil in an eye) stay holes in the new shape; off fills over them",
  "fill.hintMouse": "Click inside an area to fill it · Shift+click collects areas, Enter fills them · Esc",
  "fill.hintTouch": "Tap inside an area to show it, tap it again (or Add) to fill it",
  "fill.area": "Area {area}",
  "fill.pending": "Collected: {n}",
  "fill.add": "Add",
  "fill.addN": "Add ({n})",
  "fill.clear": "Clear",
  "fill.done": "Done",
  "fill.looking": "Finding the area…",
  "fill.filled": "Filled {n} area(s) on {layer}",
  "fill.notEnclosed": "No closed area here: the lines around this point leave an opening (try a gap tolerance)",
  "fill.onLine": "That is on a line: click inside an area",
  "tlx.fillRegion": "Fill region",
} as const;
