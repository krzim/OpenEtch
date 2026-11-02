// STAGE A STUB (owner: P5). The empty-document card (docs/ux/ui-plan-2026-11.md P5): Import, Open project, Draw a
// rectangle or text, Templates, and a device status line. P1 mounts it in `.center-stage` while the document is empty.
// The signature below is final; until P5 lands it renders nothing.

import "./laser.css";

export interface EmptyCanvasCardProps {
  onImport: () => void;
  onOpen: () => void;
  /** Pick a drawing tool. */
  onDraw: (tool: "rect" | "text") => void;
  onTemplates: () => void;
}

export function EmptyCanvasCard(_p: EmptyCanvasCardProps) {
  return null;
}
