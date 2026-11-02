// Design toolbar groups for the editing tools, Fusion-style (icon over label): Line, Pen and Polygon; Edit nodes, Trim
// and Measure; and the Modify and Arrange dropdowns, which open a menu under the button with LightBurn's hotkeys.

import type { CanvasToolId } from "../canvasTool.ts";
import { MenuButton, type MenuItem } from "./Menu.tsx";
import { Icon } from "./ui.tsx";
import type { Tool } from "./Viewport.tsx";
import { useT } from "../workspace/usePrefs.ts";

function ToolBtn(props: { icon: string; label: string; active?: boolean; disabled?: boolean; title: string; onClick: () => void }) {
  return (
    <button className={`tool-btn${props.active ? " active" : ""}`} onClick={props.onClick} disabled={props.disabled} title={props.title} aria-pressed={props.active}>
      <Icon name={props.icon} size={20} />
      <span>{props.label}</span>
    </button>
  );
}

/** A toolbar button that opens a menu under itself (Fusion's dropdown panels). Also used by src/design/. */
export function Dropdown(props: { icon: string; label: string; title: string; items: MenuItem[] }) {
  return (
    <MenuButton className="tool-btn et-dropdown" label={props.label} title={props.title} items={props.items}>
      <Icon name={props.icon} size={20} />
      <span>
        {props.label}
        <span className="et-caret" aria-hidden="true">
          <Icon name="chevronDown" size={10} />
        </span>
      </span>
    </MenuButton>
  );
}

export function EditToolbar(props: {
  tool: Tool;
  onTool: (t: CanvasToolId) => void;
  can: { line: boolean; pen: boolean; polygon: boolean; node: boolean; trim: boolean };
  modify: MenuItem[];
  arrange: MenuItem[];
}) {
  const tr = useT();
  const t = props.tool;
  const missing = ` ${tr("em.missing")}`;
  return (
    <>
      <div className="tool-group" role="group" aria-label={tr("em.draw")}>
        <ToolBtn icon="line" label={tr("em.line")} active={t === "line"} disabled={!props.can.line} onClick={() => props.onTool("line")} title={`${tr("em.lineTitle")}${props.can.line ? "" : missing}`} />
        <ToolBtn icon="pen" label={tr("em.pen")} active={t === "pen"} disabled={!props.can.pen} onClick={() => props.onTool("pen")} title={`${tr("em.penTitle")}${props.can.pen ? "" : missing}`} />
        <ToolBtn icon="polygon" label={tr("em.polygon")} active={t === "polygon"} disabled={!props.can.polygon} onClick={() => props.onTool("polygon")} title={`${tr("em.polygonTitle")}${props.can.polygon ? "" : missing}`} />
      </div>
      <div className="tool-group" role="group" aria-label={tr("em.edit")}>
        <ToolBtn icon="node" label={tr("em.editNodes")} active={t === "node"} disabled={!props.can.node} onClick={() => props.onTool("node")} title={`${tr("em.nodesTitle")}${props.can.node ? "" : missing}`} />
        <ToolBtn icon="trim" label={tr("em.trim")} active={t === "trim"} disabled={!props.can.trim} onClick={() => props.onTool("trim")} title={`${tr("em.trimTitle")}${props.can.trim ? "" : missing}`} />
        <ToolBtn icon="measure" label={tr("em.measure")} active={t === "measure"} onClick={() => props.onTool("measure")} title={tr("em.measureTitle")} />
      </div>
      <div className="tool-group" role="group" aria-label={tr("em.modifyArrange")}>
        <Dropdown icon="modify" label={tr("em.modify")} title={tr("em.modifyTitle")} items={props.modify} />
        <Dropdown icon="arrange" label={tr("em.arrange")} title={tr("em.arrangeTitle")} items={props.arrange} />
      </div>
    </>
  );
}
