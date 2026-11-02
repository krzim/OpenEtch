// Per-workspace toolbar, Fusion-style: icon over label, grouped. Design edits the document (draw, import, place,
// array, the Arrange tools for align/distribute/group/flip, and the snap and view toggles); Laser holds the toolpath
// preview toggle and the machine actions (Frame uses the red pointer only and Stop frame ends it without touching
// pending runs; Request run never fires by itself). The emergency STOP lives in the top bar.

import type { ReactNode } from "react";
import type { Workspace } from "./TopBar.tsx";
import type { SnapSettings, Tool, ViewStyle } from "./Viewport.tsx";
import { Icon } from "./ui.tsx";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
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
  /** No PIN on the engine: the run button is Start and burns at once. */
  startMode?: boolean;
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
  const t = useT();
  const { units: lengthUnit, beginnerMode } = usePrefs();
  const two = p.units >= 2;
  const three = p.units >= 3;
  const a = (x: ArrangeAction) => () => p.onArrange(x);
  return (
    <div className="toolbar">
      <button className={`tool-btn panel-toggle${p.leftOpen ? " active" : ""}`} onClick={p.onToggleLeft} title={t("tb.browserPanel")} aria-pressed={p.leftOpen}>
        <Icon name="panelLeft" size={22} />
        <span>{t("tb.browser")}</span>
      </button>
      <div className="tool-sep" />
      {p.workspace === "design" ? (
        <>
          <div className="tool-group">
            <Btn icon="stock" label={t("tb.stock")} onClick={p.onStock} title={t("tb.stockTitle")} />
          </div>
          <div className="tool-group">
            <Btn icon="rect" label={t("tb.rect")} active={p.tool === "rect"} onClick={() => p.onTool(p.tool === "rect" ? "select" : "rect")} title={t("tb.rectTitle")} />
            <Btn icon="ellipse" label={t("tb.ellipse")} active={p.tool === "ellipse"} onClick={() => p.onTool(p.tool === "ellipse" ? "select" : "ellipse")} title={t("tb.ellipseTitle")} />
            <Btn icon="circle" label={t("tb.circle")} active={p.tool === "circle"} onClick={() => p.onTool(p.tool === "circle" ? "select" : "circle")} title={t("tb.circleTitle")} />
            <Btn icon="import" label={t("tb.import")} onClick={p.onImport} title={t("tb.importTitle")} />
          </div>
          {p.extra}
          <div className="tool-group">
            <Btn icon="place" label={t("tb.place")} onClick={p.onPlace} disabled={!p.hasSelection || !p.hasStock} title={p.hasStock ? t("tb.placeTitle") : t("tb.stockFirst")} />
            <Btn icon="array" label={t("tb.array")} onClick={p.onArray} disabled={!p.hasSelection} />
            <Btn icon="delete" label={t("tb.delete")} onClick={p.onDelete} disabled={!p.hasSelection} title={t("tb.deleteTitle")} />
          </div>
          <div className="tool-group arrange" role="group" aria-label={t("tb.arrange")}>
            <div className={`mini-grid${beginnerMode ? "" : " eight"}`}>
              <Mini icon="alignLeft" title={t("tb.alignLeft")} onClick={a("left")} disabled={!two} />
              <Mini icon="alignHCenter" title={t("tb.alignHCenter")} onClick={a("hcenter")} disabled={!two} />
              <Mini icon="alignRight" title={t("tb.alignRight")} onClick={a("right")} disabled={!two} />
              {!beginnerMode && <Mini icon="distH" title={t("tb.distH")} onClick={a("distH")} disabled={!three} />}
              {!beginnerMode && <Mini icon="distHc" title={t("tb.distHc")} onClick={a("distHc")} disabled={!three} />}
              <Mini icon="centerStock" title={t("tb.centreStock")} onClick={a("stock")} disabled={!p.hasSelection || !p.hasStock} />
              <Mini icon="group" title={t("tb.group")} onClick={a("group")} disabled={!two || !p.canGroup} />
              <Mini icon="flipH" title={t("tb.flipH")} onClick={a("flipH")} disabled={!p.hasSelection} />
              <Mini icon="alignTop" title={t("tb.alignTop")} onClick={a("top")} disabled={!two} />
              <Mini icon="alignVCenter" title={t("tb.alignVCenter")} onClick={a("vcenter")} disabled={!two} />
              <Mini icon="alignBottom" title={t("tb.alignBottom")} onClick={a("bottom")} disabled={!two} />
              {!beginnerMode && <Mini icon="distV" title={t("tb.distV")} onClick={a("distV")} disabled={!three} />}
              {!beginnerMode && <Mini icon="distVc" title={t("tb.distVc")} onClick={a("distVc")} disabled={!three} />}
              <Mini icon="centerField" title={t("tb.centreField")} onClick={a("field")} disabled={!p.hasSelection} />
              <Mini icon="ungroup" title={t("tb.ungroup")} onClick={a("ungroup")} disabled={!p.grouped || !p.canGroup} />
              <Mini icon="flipV" title={t("tb.flipV")} onClick={a("flipV")} disabled={!p.hasSelection} />
            </div>
            <span className="mini-label">{t("tb.arrange")}</span>
          </div>
          <div className="tool-group arrange" role="group" aria-label={t("tb.snapAndView")}>
            <div className="mini-grid two">
              <Mini
                icon="magnet"
                title={t("tb.snapObjects", { state: p.snap.objects ? t("tb.on") : t("tb.off") })}
                active={p.snap.objects}
                onClick={() => p.onSnap({ ...p.snap, objects: !p.snap.objects })}
              />
              <Mini
                icon="gridSnap"
                title={t("tb.snapGrid", { state: p.snap.grid ? t("tb.on") : t("tb.off"), step: formatLength(p.snap.gridMm, lengthUnit, 3) })}
                active={p.snap.grid}
                onClick={() => p.onSnap({ ...p.snap, grid: !p.snap.grid })}
              />
              <Mini
                icon="filled"
                title={t("tb.filled", { state: p.viewStyle === "filled" ? t("tb.on") : t("tb.off") })}
                active={p.viewStyle === "filled"}
                onClick={() => p.onViewStyle(p.viewStyle === "filled" ? "wireframe" : "filled")}
              />
              <select
                className="mini-select"
                title={t("tb.gridStep")}
                aria-label={t("tb.gridStep")}
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
            <span className="mini-label">{t("tb.snapView")}</span>
          </div>
        </>
      ) : (
        <>
          <div className="tool-group">
            <Btn icon="preview" label={t("tb.preview")} active={p.showPreview} onClick={p.onTogglePreview} title={t("tb.previewTitle")} />
          </div>
          <div className="tool-group">
            <Btn icon="frame" label={t("tb.frame")} onClick={p.onFrame} disabled={!!p.designOnly} title={p.designOnly ?? t("tb.frameTitle")} />
            <Btn icon="frameStop" label={t("tb.stopFrame")} onClick={p.onFrameStop} title={t("tb.stopFrameTitle")} />
            <Btn
              icon="run"
              label={p.startMode ? t("laser.start") : t("tb.requestRun")}
              onClick={p.onRequestRun}
              disabled={!!p.designOnly}
              title={p.designOnly ?? (p.startMode ? t("laser.startTitle") : t("tb.requestRunTitle"))}
              cls="tool-run"
            />
          </div>
        </>
      )}
      <div className="tool-group">
        <Btn icon="terminal" label={t("tb.commands")} onClick={p.onCommands} title={t("tb.commandsTitle")} />
      </div>
      <div className="tool-spacer" />
      <button className={`tool-btn panel-toggle${p.rightOpen ? " active" : ""}`} onClick={p.onToggleRight} title={t("tb.propertiesPanel")} aria-pressed={p.rightOpen}>
        <Icon name="panelRight" size={22} />
        <span>{p.workspace === "laser" ? t("tb.machine") : t("tb.properties")}</span>
      </button>
    </div>
  );
}
