// Per-workspace toolbar, Fusion-style: icon over label, grouped. Design edits the document; Laser holds the
// toolpath preview toggle and the machine actions (Frame uses the red pointer only and Stop frame ends it without
// touching pending runs; Request run never fires by itself). The emergency STOP lives in the top bar.

import type { Workspace } from "./TopBar.tsx";
import type { Tool } from "./Viewport.tsx";
import { Icon } from "./ui.tsx";

export interface ToolbarProps {
  workspace: Workspace;
  tool: Tool;
  hasSelection: boolean;
  hasStock: boolean;
  showPreview: boolean;
  leftOpen: boolean;
  rightOpen: boolean;
  onTool: (t: Tool) => void;
  onStock: () => void;
  onImport: () => void;
  onPlace: () => void;
  onArray: () => void;
  onDelete: () => void;
  onCommands: () => void;
  onTogglePreview: () => void;
  onFrame: () => void;
  onFrameStop: () => void;
  onRequestRun: () => void;
  onToggleLeft: () => void;
  onToggleRight: () => void;
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

export function Toolbar(p: ToolbarProps) {
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
            <Btn icon="ellipse" label="Ellipse" active={p.tool === "ellipse"} onClick={() => p.onTool(p.tool === "ellipse" ? "select" : "ellipse")} title="Ellipse (E)" />
            <Btn icon="import" label="Import SVG" onClick={p.onImport} />
          </div>
          <div className="tool-group">
            <Btn icon="place" label="Place" onClick={p.onPlace} disabled={!p.hasSelection || !p.hasStock} title={p.hasStock ? "Place on stock" : "Define a stock first"} />
            <Btn icon="array" label="Array" onClick={p.onArray} disabled={!p.hasSelection} />
            <Btn icon="delete" label="Delete" onClick={p.onDelete} disabled={!p.hasSelection} title="Delete (Del)" />
          </div>
        </>
      ) : (
        <>
          <div className="tool-group">
            <Btn icon="preview" label="Preview" active={p.showPreview} onClick={p.onTogglePreview} title="Show simulated toolpath" />
          </div>
          <div className="tool-group">
            <Btn icon="frame" label="Frame" onClick={p.onFrame} title="Trace the job bounds with the red pointer (no emission)" />
            <Btn icon="frameStop" label="Stop frame" onClick={p.onFrameStop} title="End red-pointer framing (pending runs are kept)" />
            <Btn icon="run" label="Request run" onClick={p.onRequestRun} title="Create a pending run; it fires only after approval" cls="tool-run" />
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
