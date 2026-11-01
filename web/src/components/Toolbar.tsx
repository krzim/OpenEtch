// Per-workspace toolbar, Fusion-style: icon over label, grouped. Design edits the document (draw, import, place,
// array, the Arrange tools for align/distribute/group/flip, and the snap and view toggles); Laser holds the toolpath
// preview toggle and the machine actions (Frame uses the red pointer only and Stop frame ends it without touching
// pending runs; Request run never fires by itself). The emergency STOP lives in the top bar.

import type { ReactNode } from "react";
import type { Workspace } from "./TopBar.tsx";
import type { SnapSettings, Tool, ViewStyle } from "./Viewport.tsx";
import { Icon } from "./ui.tsx";
import { usePrefs } from "../workspace/usePrefs.ts";
import { formatLength } from "../workspace/units.ts";

export type ArrangeAction =
  | "left"
  | "hcenter"
  | "right"
  | "top"
  | "vcenter"
  | "bottom"
  | "centers"
  | "distH"
  | "distV"
  | "distHc"
  | "distVc"
  | "stock"
  | "field"
  | "group"
  | "ungroup"
  | "flipH"
  | "flipV";

export interface ToolbarProps {
  workspace: Workspace;
  tool: Tool;
  hasSelection: boolean;
  /** Selected units (a group counts once). */
  units: number;
  /** The selection contains a group. */
  grouped: boolean;
  hasStock: boolean;
  showPreview: boolean;
  leftOpen: boolean;
  rightOpen: boolean;
  snap: SnapSettings;
  viewStyle: ViewStyle;
  canGroup: boolean;
  onTool: (t: Tool) => void;
  onStock: () => void;
  onImport: () => void;
  onPlace: () => void;
  onArray: () => void;
  onDelete: () => void;
  onArrange: (a: ArrangeAction) => void;
  onSnap: (s: SnapSettings) => void;
  onViewStyle: (v: ViewStyle) => void;
  onCommands: () => void;
  onTogglePreview: () => void;
  onFrame: () => void;
  onFrameStop: () => void;
  onRequestRun: () => void;
  /** Why the active device can't frame or run (design-only device), or null. */
  designOnly?: string | null;
  onToggleLeft: () => void;
  onToggleRight: () => void;
  /** Design-only groups after the drawing tools: line/pen/polygon, node editing, Modify and Arrange (EditMenus.tsx). */
  extra?: ReactNode;
}

function Btn(props: { icon: string; label: string; onClick: () => void; active?: boolean; disabled?: boolean; title?: string; cls?: string }) {
  return (
    <button
      className={`tool-btn${props.active ? " active" : ""}${props.cls ? ` ${props.cls}` : ""}`}
      onClick={props.onClick}
      disabled={props.disabled}
      title={props.title ?? props.label}
      aria-pressed={props.active}
    >
      <Icon name={props.icon} size={22} />
      <span>{props.label}</span>
    </button>
  );
}

/** Small icon-only button for the Arrange grid. */
function Mini(props: { icon: string; title: string; onClick: () => void; disabled?: boolean; active?: boolean }) {
  return (
    <button
      className={`mini-btn${props.active ? " active" : ""}`}
      onClick={props.onClick}
      disabled={props.disabled}
      title={props.title}
      aria-label={props.title}
      aria-pressed={props.active}
    >
      <Icon name={props.icon} size={18} />
    </button>
  );
}

export function Toolbar(p: ToolbarProps) {
  const lengthUnit = usePrefs().units;
  const two = p.units >= 2;
  const three = p.units >= 3;
  const a = (x: ArrangeAction) => () => p.onArrange(x);
  return (
    <div className="toolbar">
      <button className={`tool-btn panel-toggle${p.leftOpen ? " active" : ""}`} onClick={p.onToggleLeft} title="Browser panel" aria-pressed={p.leftOpen}>
        <Icon name="panelLeft" size={22} />
        <span>Browser</span>
      </button>
      <div className="tool-sep" />
      {p.workspace === "design" ? (
        <>
          <div className="tool-group">
            <Btn icon="stock" label="Stock" onClick={p.onStock} title="Define the stock (material piece)" />
          </div>
          <div className="tool-group">
            <Btn icon="rect" label="Rectangle" active={p.tool === "rect"} onClick={() => p.onTool(p.tool === "rect" ? "select" : "rect")} title="Rectangle (R)" />
            <Btn icon="ellipse" label="Ellipse" active={p.tool === "ellipse"} onClick={() => p.onTool(p.tool === "ellipse" ? "select" : "ellipse")} title="Ellipse (E). Shift draws a circle." />
            <Btn icon="circle" label="Circle" active={p.tool === "circle"} onClick={() => p.onTool(p.tool === "circle" ? "select" : "circle")} title="Circle from the centre (C)" />
            <Btn icon="import" label="Import" onClick={p.onImport} title="Import SVG, DXF, PNG/JPG, LightBurn or OpenEtch files (Ctrl+I). You can also drop files on the window." />
          </div>
          {p.extra}
          <div className="tool-group">
            <Btn icon="place" label="Place" onClick={p.onPlace} disabled={!p.hasSelection || !p.hasStock} title={p.hasStock ? "Place on stock" : "Define a stock first"} />
            <Btn icon="array" label="Array" onClick={p.onArray} disabled={!p.hasSelection} />
            <Btn icon="delete" label="Delete" onClick={p.onDelete} disabled={!p.hasSelection} title="Delete (Del)" />
          </div>
          <div className="tool-group arrange" role="group" aria-label="Arrange">
            <div className="mini-grid">
              <Mini icon="alignLeft" title="Align left (L). Aligns to the last object selected." onClick={a("left")} disabled={!two} />
              <Mini icon="alignHCenter" title="Align horizontal centres (C)" onClick={a("hcenter")} disabled={!two} />
              <Mini icon="alignRight" title="Align right (R)" onClick={a("right")} disabled={!two} />
              <Mini icon="distH" title="Distribute horizontally: equal gaps (3+ objects)" onClick={a("distH")} disabled={!three} />
              <Mini icon="centerStock" title="Centre on stock" onClick={a("stock")} disabled={!p.hasSelection || !p.hasStock} />
              <Mini icon="group" title="Group (Ctrl+G)" onClick={a("group")} disabled={!two || !p.canGroup} />
              <Mini icon="flipH" title="Flip horizontal (Ctrl+Shift+H)" onClick={a("flipH")} disabled={!p.hasSelection} />
              <Mini icon="alignTop" title="Align top (T)" onClick={a("top")} disabled={!two} />
              <Mini icon="alignVCenter" title="Align vertical centres (E)" onClick={a("vcenter")} disabled={!two} />
              <Mini icon="alignBottom" title="Align bottom (B)" onClick={a("bottom")} disabled={!two} />
              <Mini icon="distV" title="Distribute vertically: equal gaps (3+ objects)" onClick={a("distV")} disabled={!three} />
              <Mini icon="centerField" title="Centre on field (P)" onClick={a("field")} disabled={!p.hasSelection} />
              <Mini icon="ungroup" title="Ungroup (Ctrl+U)" onClick={a("ungroup")} disabled={!p.grouped || !p.canGroup} />
              <Mini icon="flipV" title="Flip vertical (Ctrl+Shift+V)" onClick={a("flipV")} disabled={!p.hasSelection} />
            </div>
            <span className="mini-label">Arrange</span>
          </div>
          <div className="tool-group arrange" role="group" aria-label="Snap and view">
            <div className="mini-grid two">
              <Mini
                icon="magnet"
                title={`Snap to objects ${p.snap.objects ? "on" : "off"}: centres, quadrants, vertices, midpoints, intersections and outlines of objects, stock and field, with alignment guides. Hold Ctrl while dragging to toggle, Alt to snap to guides only.`}
                active={p.snap.objects}
                onClick={() => p.onSnap({ ...p.snap, objects: !p.snap.objects })}
              />
              <Mini
                icon="gridSnap"
                title={`Snap to grid ${p.snap.grid ? "on" : "off"} (${formatLength(p.snap.gridMm, lengthUnit, 3)})`}
                active={p.snap.grid}
                onClick={() => p.onSnap({ ...p.snap, grid: !p.snap.grid })}
              />
              <Mini
                icon="filled"
                title={`Filled view ${p.viewStyle === "filled" ? "on" : "off"}: fill layers drawn as shaded regions (Alt+Shift+W)`}
                active={p.viewStyle === "filled"}
                onClick={() => p.onViewStyle(p.viewStyle === "filled" ? "wireframe" : "filled")}
              />
              <select
                className="mini-select"
                title="Grid snap distance"
                aria-label="Grid snap distance"
                value={p.snap.gridMm}
                onChange={(e) => p.onSnap({ ...p.snap, gridMm: Number(e.target.value) })}
              >
                {[0.1, 0.5, 1, 2, 5, 10].map((g) => (
                  <option key={g} value={g}>
                    {g} mm
                  </option>
                ))}
              </select>
            </div>
            <span className="mini-label">Snap · view</span>
          </div>
        </>
      ) : (
        <>
          <div className="tool-group">
            <Btn icon="preview" label="Preview" active={p.showPreview} onClick={p.onTogglePreview} title="Show simulated toolpath" />
          </div>
          <div className="tool-group">
            <Btn icon="frame" label="Frame" onClick={p.onFrame} disabled={!!p.designOnly} title={p.designOnly ?? "Trace the job bounds with the red pointer (no emission)"} />
            <Btn icon="frameStop" label="Stop frame" onClick={p.onFrameStop} title="End red-pointer framing (pending runs are kept)" />
            <Btn icon="run" label="Request run" onClick={p.onRequestRun} disabled={!!p.designOnly} title={p.designOnly ?? "Create a pending run; it fires only after approval"} cls="tool-run" />
          </div>
        </>
      )}
      <div className="tool-group">
        <Btn icon="terminal" label="Commands" onClick={p.onCommands} title="Run any registered command" />
      </div>
      <div className="tool-spacer" />
      <button className={`tool-btn panel-toggle${p.rightOpen ? " active" : ""}`} onClick={p.onToggleRight} title="Properties panel" aria-pressed={p.rightOpen}>
        <Icon name="panelRight" size={22} />
        <span>{p.workspace === "laser" ? "Machine" : "Properties"}</span>
      </button>
    </div>
  );
}
