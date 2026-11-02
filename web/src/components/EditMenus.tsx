// The editing tools' ribbon entries (Toolbar.tsx groups, Fusion-style): Line, Pen and Polygon; Edit nodes, Trim and
// the Modify dropdown; Measure; and the Arrange items for the toolbar's Arrange dropdown, with LightBurn's hotkeys.
// On the phone Pen, Polygon, Edit nodes, Trim and Weld are desktop features (workspace/desktopOnly.ts): left out of
// the strip and the Modify dropdown unless active (then the strip keeps the button, to switch the tool off again).

import type { CanvasToolId } from "../canvasTool.ts";
import { MenuButton, type MenuItem } from "./Menu.tsx";
import { Icon } from "./ui.tsx";
import { RibbonSlot, usePublish } from "./Ribbon.tsx";
import type { Tool } from "./Viewport.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { forLayout, toolOffered } from "../workspace/desktopOnly.ts";
import { useLayoutMode } from "../workspace/layoutMode.ts";

function ToolBtn(props: { icon: string; label: string; active?: boolean; disabled?: boolean; title: string; onClick: () => void }) {
  return (
    <button
      type="button"
      className={`tool-btn${props.active ? " active" : ""}`}
      onClick={props.onClick}
      disabled={props.disabled}
      title={props.title}
      aria-label={props.label}
      aria-pressed={props.active}
    >
      <Icon name={props.icon} size={20} />
      <span className="tool-caption">{props.label}</span>
    </button>
  );
}

/** A toolbar button that opens a menu under itself (Fusion's dropdown panels). Also used by src/design/ and libs/. */
export function Dropdown(props: { icon: string; label: string; title: string; items: MenuItem[]; badge?: boolean }) {
  return (
    <MenuButton className="tool-btn et-dropdown" label={props.label} title={props.title} items={props.items} ariaLabel={props.label}>
      <Icon name={props.icon} size={20} />
      <span className="tool-caption">
        {props.label}
        <span className="et-caret" aria-hidden="true">
          <Icon name="chevronDown" size={10} />
        </span>
      </span>
      {props.badge && <span className="lx-dot" aria-hidden="true" />}
    </MenuButton>
  );
}

/**
 * The edit tools' ribbon entries, placed into the toolbar's groups: Line / Pen / Polygon under CREATE, Edit nodes,
 * Trim and Modify ▾ under MODIFY, Measure under INSPECT; the Arrange items join the toolbar's Arrange ▾ menu.
 */
export function EditToolbar(props: {
  tool: Tool;
  onTool: (t: CanvasToolId) => void;
  can: { line: boolean; pen: boolean; polygon: boolean; node: boolean; trim: boolean };
  modify: MenuItem[];
  arrange: MenuItem[];
}) {
  const tr = useT();
  const t = props.tool;
  const layout = useLayoutMode().layout;
  const offered = (tool: string) => toolOffered(layout, tool, t);
  const modify = forLayout(props.modify, layout);
  const arrange = forLayout(props.arrange, layout);
  const missing = ` ${tr("em.missing")}`;
  usePublish("ribbon.arrange", arrange);
  usePublish("menu.modify", { label: tr("em.modify"), items: modify });
  return (
    <>
      <RibbonSlot group="create" order={10}>
        <ToolBtn icon="line" label={tr("em.line")} active={t === "line"} disabled={!props.can.line} onClick={() => props.onTool("line")} title={`${tr("em.lineTitle")}${props.can.line ? "" : missing}`} />
        {offered("pen") && (
          <ToolBtn icon="pen" label={tr("em.pen")} active={t === "pen"} disabled={!props.can.pen} onClick={() => props.onTool("pen")} title={`${tr("em.penTitle")}${props.can.pen ? "" : missing}`} />
        )}
        {offered("polygon") && (
          <ToolBtn icon="polygon" label={tr("em.polygon")} active={t === "polygon"} disabled={!props.can.polygon} onClick={() => props.onTool("polygon")} title={`${tr("em.polygonTitle")}${props.can.polygon ? "" : missing}`} />
        )}
      </RibbonSlot>
      <RibbonSlot group="modify" order={10}>
        {offered("node") && (
          <ToolBtn icon="node" label={tr("em.editNodes")} active={t === "node"} disabled={!props.can.node} onClick={() => props.onTool("node")} title={`${tr("em.nodesTitle")}${props.can.node ? "" : missing}`} />
        )}
        {offered("trim") && (
          <ToolBtn icon="trim" label={tr("em.trim")} active={t === "trim"} disabled={!props.can.trim} onClick={() => props.onTool("trim")} title={`${tr("em.trimTitle")}${props.can.trim ? "" : missing}`} />
        )}
        <Dropdown icon="modify" label={tr("em.modify")} title={tr("em.modifyTitle")} items={modify} />
      </RibbonSlot>
      <RibbonSlot group="inspect" order={0}>
        <ToolBtn icon="measure" label={tr("em.measure")} active={t === "measure"} onClick={() => props.onTool("measure")} title={tr("em.measureTitle")} />
      </RibbonSlot>
    </>
  );
}
