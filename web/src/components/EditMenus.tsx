// Design toolbar groups for the editing tools, Fusion-style (icon over label): Line, Pen and Polygon; Edit nodes, Trim
// and Measure; and the Modify and Arrange dropdowns, which open a menu under the button with LightBurn's hotkeys.

import { useRef, useState } from "react";
import type { CanvasToolId } from "../canvasTool.ts";
import { ContextMenu, type MenuItem } from "./ContextMenu.tsx";
import type { Tool } from "./Viewport.tsx";

const ICONS: Record<string, string> = {
  line: "M4 18L10 8l5 7 5-10 M4 18h.01 M10 8h.01 M15 15h.01 M20 5h.01",
  pen: "M12 19l7-7 2 2-7 7z M18 13l-1.5-7.5L3 2l3.5 13.5L14 17z M3 2l7.6 7.6 M11 11a1 1 0 100 .01",
  polygon: "M12 3l8 5.5-3 9.5H7L4 8.5z",
  node: "M5 19C5 10 12 5 19 5 M3 17h4v4H3z M17 3h4v4h-4z M5 19l6-11 M11 6h2v2h-2z",
  trim: "M6 4a2 2 0 100 4 2 2 0 000-4z M6 16a2 2 0 100 4 2 2 0 000-4z M7.6 7.2L20 18 M7.6 16.8L20 6",
  measure: "M3 17L17 3l4 4L7 21z M7 13l2 2 M10 10l2 2 M13 7l2 2",
  modify: "M4 12h6 M14 12h6 M12 4v6 M12 14v6 M7 7h10v10H7z",
  arrange: "M4 4h10v10H4z M10 10h10v10H10z",
  chevronDown: "M8 10l4 4 4-4",
};

function EIcon({ name, size = 22 }: { name: string; size?: number }) {
  return (
    <svg className="icon" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={ICONS[name] ?? ""} />
    </svg>
  );
}

function ToolBtn(props: { icon: string; label: string; active?: boolean; disabled?: boolean; title: string; onClick: () => void }) {
  return (
    <button className={`tool-btn${props.active ? " active" : ""}`} onClick={props.onClick} disabled={props.disabled} title={props.title} aria-pressed={props.active}>
      <EIcon name={props.icon} />
      <span>{props.label}</span>
    </button>
  );
}

/** A toolbar button that opens a menu under itself (Fusion's dropdown panels). */
function Dropdown(props: { icon: string; label: string; title: string; items: MenuItem[] }) {
  const ref = useRef<HTMLButtonElement>(null);
  const [at, setAt] = useState<{ x: number; y: number } | null>(null);
  const justClosed = useRef(0);
  return (
    <>
      <button
        ref={ref}
        className={`tool-btn et-dropdown${at ? " active" : ""}`}
        title={props.title}
        aria-haspopup="menu"
        aria-expanded={!!at}
        onClick={() => {
          // A press on the button while open closes the menu (outside press) and must not reopen it.
          if (Date.now() - justClosed.current < 250) return;
          const r = ref.current?.getBoundingClientRect();
          if (r) setAt({ x: r.left, y: r.bottom + 2 });
        }}
      >
        <EIcon name={props.icon} />
        <span>
          {props.label}
          <svg className="et-caret" width={10} height={10} viewBox="0 0 24 24" aria-hidden="true">
            <path d={ICONS.chevronDown} fill="none" stroke="currentColor" strokeWidth="2.5" />
          </svg>
        </span>
      </button>
      {at && (
        <ContextMenu
          x={at.x}
          y={at.y}
          items={props.items}
          onClose={() => {
            justClosed.current = Date.now();
            setAt(null);
          }}
        />
      )}
    </>
  );
}

export function EditToolbar(props: {
  tool: Tool;
  onTool: (t: CanvasToolId) => void;
  can: { line: boolean; pen: boolean; polygon: boolean; node: boolean; trim: boolean };
  modify: MenuItem[];
  arrange: MenuItem[];
}) {
  const t = props.tool;
  const missing = " (this engine does not have it yet)";
  return (
    <>
      <div className="tool-group" role="group" aria-label="Draw">
        <ToolBtn icon="line" label="Line" active={t === "line"} disabled={!props.can.line} onClick={() => props.onTool("line")} title={`Line / polyline (Ctrl+L): click points, double-click or Enter to finish, C closes, Shift 15°${props.can.line ? "" : missing}`} />
        <ToolBtn icon="pen" label="Pen" active={t === "pen"} disabled={!props.can.pen} onClick={() => props.onTool("pen")} title={`Bézier pen: click for corners, drag for smooth curves${props.can.pen ? "" : missing}`} />
        <ToolBtn icon="polygon" label="Polygon" active={t === "polygon"} disabled={!props.can.polygon} onClick={() => props.onTool("polygon")} title={`Polygon or star: drag from the centre; sides and star in the options bar${props.can.polygon ? "" : missing}`} />
      </div>
      <div className="tool-group" role="group" aria-label="Edit">
        <ToolBtn icon="node" label="Edit nodes" active={t === "node"} disabled={!props.can.node} onClick={() => props.onTool("node")} title={`Edit nodes (Ctrl+\`): drag nodes and handles, click a segment to add a node${props.can.node ? "" : missing}`} />
        <ToolBtn icon="trim" label="Trim" active={t === "trim"} disabled={!props.can.trim} onClick={() => props.onTool("trim")} title={`Trim: click a piece of a shape to remove it up to the nearest crossings${props.can.trim ? "" : missing}`} />
        <ToolBtn icon="measure" label="Measure" active={t === "measure"} onClick={() => props.onTool("measure")} title="Measure: click two points or drag; distance, ΔX, ΔY and angle (snaps)" />
      </div>
      <div className="tool-group" role="group" aria-label="Modify and arrange">
        <Dropdown icon="modify" label="Modify" title="Booleans, weld, offset, fillet and path tools" items={props.modify} />
        <Dropdown icon="arrange" label="Arrange" title="Draw order, lock, make same size, transform, arrays and nesting" items={props.arrange} />
      </div>
    </>
  );
}
