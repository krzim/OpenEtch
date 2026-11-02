// The per-workspace ribbon (docs/ux/ui-plan-2026-11.md P2 "Ribbon"), Fusion-style groups that never scroll:
// - Design: CREATE (Stock, Rect, Ellipse, Circle, Line/Pen/Polygon, Text, Barcode), MODIFY (Nodes, Trim, Modify ▾,
//   Edit ▾, Tools ▾, Delete), ARRANGE (Arrange ▾ with align, distribute, group, flip and centre, plus an align strip
//   of six buttons while two or more objects are selected), INSERT (Import, Place, Array, Art), INSPECT (Measure,
//   snap and view toggles) and LASER TOOLS. Lower-priority groups fold into `GROUP ▾` buttons when space runs out.
// - Laser: Preview | Frame ▾ | Start | Stop frame, all pinned. Frame uses the red pointer only and Stop frame ends it
//   without touching pending runs; Request run never fires by itself. The emergency STOP lives in the top bar.
// The Browser and Properties toggles and command search are fixed end caps. Wide: icon and caption; compact: icon
// only (caption as tooltip and accessible name); tablet: the same groups as a vertical rail at the canvas's left
// edge; phone: a strip. Other hooks add their tools with <RibbonSlot> (Ribbon.tsx).

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { Workspace } from "./TopBar.tsx";
import type { SnapSettings, Tool, ViewStyle } from "./Viewport.tsx";
import { Icon } from "./ui.tsx";
import { Menu, MenuButton, type MenuItem } from "./Menu.tsx";
import { Ribbon, RibbonGroup, RibbonSlot, usePublish, usePublished, type RibbonOrientation } from "./Ribbon.tsx";
import { getCreationRail, setCreationRail, useCreationRail } from "./ribbonPrefs.ts";
import { onCommandRequest } from "./commandBus.ts";
import { runHotkeyAction } from "./commands.ts";
import { CommandSearch } from "./CommandSearch.tsx";
import { StartButton } from "../laser/StartButton.tsx";
import { useLayoutMode, type LayoutMode } from "../workspace/layoutMode.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { formatLength } from "../workspace/units.ts";
import { formatChord } from "./formatChord.ts";
import { hintFor } from "../workspace/hotkeys.ts";

import { ARRANGE_ACTION_ID, type ArrangeAction } from "./arrangeActions.ts";

export type { ArrangeAction };

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
  /** Opens the engine's JSON command console (now under Help > Developer > Engine commands, and command search). */
  onCommands: () => void;
  onTogglePreview: () => void;
  onFrame: () => void;
  onFrameStop: () => void;
  /** Why the active device can't frame or run (design-only device), or null. */
  designOnly?: string | null;
  onToggleLeft: () => void;
  onToggleRight: () => void;
  /** Tools from the other hooks (text, edit tools, design, libs), placed into groups with <RibbonSlot>. */
  extra?: ReactNode;
  /** The layout mode (default: workspace/layoutMode.ts). */
  layout?: LayoutMode;
}

/** A tool button: icon over caption (the caption hides in the compact layout and stays the accessible name). */
export function ToolButton(props: {
  icon: string;
  label: string;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  title?: string;
  cls?: string;
}) {
  return (
    <button
      type="button"
      className={`tool-btn${props.active ? " active" : ""}${props.cls ? ` ${props.cls}` : ""}`}
      onClick={props.onClick}
      disabled={props.disabled}
      title={props.title ?? props.label}
      aria-label={props.label}
      aria-pressed={props.active}
    >
      <Icon name={props.icon} size={20} />
      <span className="tool-caption">{props.label}</span>
    </button>
  );
}

/** Small icon-only button (the align strip, the snap toggles). */
function Mini(props: { icon: string; title: string; onClick: () => void; disabled?: boolean; active?: boolean }) {
  return (
    <button
      type="button"
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

function orientationOf(layout: LayoutMode): RibbonOrientation {
  return layout === "phone" ? "strip" : layout === "tablet" ? "vertical" : "horizontal";
}

export function Toolbar(p: ToolbarProps) {
  const t = useT();
  const { units: lengthUnit, beginnerMode, hotkeys } = usePrefs();
  const lm = useLayoutMode();
  const layout = p.layout ?? lm.layout;
  const orientation = orientationOf(layout);
  const railPref = useCreationRail();
  const creationRail = railPref && orientation === "horizontal" && p.workspace === "design";
  const two = p.units >= 2;
  const three = p.units >= 3;
  const a = (x: ArrangeAction) => () => p.onArrange(x);
  const editArrange = usePublished<MenuItem[]>("ribbon.arrange") ?? [];
  const [search, setSearch] = useState(false);
  const [ribbonMenu, setRibbonMenu] = useState<{ x: number; y: number } | null>(null);
  const P = useRef(p);
  P.current = p;

  // Command search (S) and Help > Developer > Engine commands come here.
  useEffect(
    () =>
      onCommandRequest((r) => {
        if (r === "search") setSearch(true);
        else P.current.onCommands();
      }),
    [],
  );

  // The vertical rails sit at the canvas's left edge, starting where the toolbar row would be.
  const anchor = useRef<HTMLDivElement>(null);
  const [railTop, setRailTop] = useState<number | null>(null);
  useLayoutEffect(() => {
    if (orientation !== "vertical" && !creationRail) return;
    const measure = () => {
      const el = anchor.current;
      if (el) setRailTop(Math.round(el.getBoundingClientRect().bottom));
    };
    measure();
    window.addEventListener("resize", measure);
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    if (ro && anchor.current?.parentElement) ro.observe(anchor.current.parentElement);
    return () => {
      window.removeEventListener("resize", measure);
      ro?.disconnect();
    };
  }, [orientation, creationRail]);

  const hk = (id: string) => formatChord(hintFor(id, hotkeys));
  const arrangeItems: MenuItem[] = [
    {
      label: t("rb.align"),
      icon: "alignLeft",
      disabled: !two,
      submenu: [
        { label: t("rb.alignLeft"), icon: "alignLeft", hint: "L", actionId: ARRANGE_ACTION_ID.left, disabled: !two, action: a("left") },
        { label: t("rb.alignHCenter"), icon: "alignHCenter", hint: "C", actionId: ARRANGE_ACTION_ID.hcenter, disabled: !two, action: a("hcenter") },
        { label: t("rb.alignRight"), icon: "alignRight", hint: "R", actionId: ARRANGE_ACTION_ID.right, disabled: !two, action: a("right") },
        { sep: true },
        { label: t("rb.alignTop"), icon: "alignTop", hint: "T", actionId: ARRANGE_ACTION_ID.top, disabled: !two, action: a("top") },
        { label: t("rb.alignVCenter"), icon: "alignVCenter", hint: "E", actionId: ARRANGE_ACTION_ID.vcenter, disabled: !two, action: a("vcenter") },
        { label: t("rb.alignBottom"), icon: "alignBottom", hint: "B", actionId: ARRANGE_ACTION_ID.bottom, disabled: !two, action: a("bottom") },
        { sep: true },
        { label: t("rb.alignCenters"), actionId: ARRANGE_ACTION_ID.centers, disabled: !two, action: a("centers") },
      ],
    },
    ...(beginnerMode
      ? []
      : [
          {
            label: t("rb.distribute"),
            icon: "distH",
            disabled: !three,
            submenu: [
              { label: t("rb.distH"), icon: "distH", actionId: ARRANGE_ACTION_ID.distH, disabled: !three, action: a("distH") },
              { label: t("rb.distHc"), icon: "distHc", actionId: ARRANGE_ACTION_ID.distHc, disabled: !three, action: a("distHc") },
              { label: t("rb.distV"), icon: "distV", actionId: ARRANGE_ACTION_ID.distV, disabled: !three, action: a("distV") },
              { label: t("rb.distVc"), icon: "distVc", actionId: ARRANGE_ACTION_ID.distVc, disabled: !three, action: a("distVc") },
            ],
          } as MenuItem,
        ]),
    { sep: true },
    { label: t("rb.group"), icon: "group", actionId: ARRANGE_ACTION_ID.group, disabled: !two || !p.canGroup, action: a("group") },
    { label: t("rb.ungroup"), icon: "ungroup", actionId: ARRANGE_ACTION_ID.ungroup, disabled: !p.grouped || !p.canGroup, action: a("ungroup") },
    { sep: true },
    { label: t("rb.flipH"), icon: "flipH", actionId: ARRANGE_ACTION_ID.flipH, disabled: !p.hasSelection, action: a("flipH") },
    { label: t("rb.flipV"), icon: "flipV", actionId: ARRANGE_ACTION_ID.flipV, disabled: !p.hasSelection, action: a("flipV") },
    { sep: true },
    { label: t("tb.centreStock"), icon: "centerStock", actionId: ARRANGE_ACTION_ID.stock, disabled: !p.hasSelection || !p.hasStock, action: a("stock") },
    { label: t("rb.centreField"), icon: "centerField", actionId: ARRANGE_ACTION_ID.field, disabled: !p.hasSelection, action: a("field") },
    ...(editArrange.length ? [{ sep: true } as MenuItem, ...editArrange] : []),
  ];

  const frameItems: MenuItem[] = [
    { label: t("rb.frameWithOptions"), icon: "frame", disabled: !!p.designOnly, action: p.onFrame },
    { sep: true },
    { label: t("rb.frameSelection"), actionId: "laser.frameSelection", disabled: !!p.designOnly || !p.hasSelection, action: () => runHotkeyAction("laser.frameSelection") },
    { label: t("rb.frameBounds"), actionId: "laser.frameSquare", disabled: !!p.designOnly, action: () => runHotkeyAction("laser.frameSquare") },
    { label: t("rb.frameOutline"), actionId: "laser.frameOutline", disabled: !!p.designOnly, action: () => runHotkeyAction("laser.frameOutline") },
  ];

  const toggleTool = (x: Tool) => () => p.onTool(p.tool === x ? "select" : x);
  const createTools = (
    <RibbonSlot group="create" order={0}>
      <ToolButton icon="stock" label={t("tb.stock")} onClick={p.onStock} title={t("tb.stockTitle")} />
      <ToolButton icon="rect" label={t("tb.rect")} active={p.tool === "rect"} onClick={toggleTool("rect")} title={t("tb.rectTitle")} />
      <ToolButton icon="ellipse" label={t("tb.ellipse")} active={p.tool === "ellipse"} onClick={toggleTool("ellipse")} title={t("tb.ellipseTitle")} />
      <ToolButton icon="circle" label={t("tb.circle")} active={p.tool === "circle"} onClick={toggleTool("circle")} title={t("tb.circleTitle")} />
    </RibbonSlot>
  );

  const startCap = (
    <button
      type="button"
      className={`tool-btn panel-toggle toolbar-start${p.leftOpen ? " active" : ""}`}
      onClick={p.onToggleLeft}
      title={t("tb.browserPanel")}
      aria-label={t("tb.browserPanel")}
      aria-pressed={p.leftOpen}
    >
      <Icon name="panelLeft" size={20} />
      <span className="tool-caption">{t("tb.browser")}</span>
    </button>
  );
  const endCaps = (
    <>
      <div className="tool-spacer" />
      <button
        type="button"
        className="tool-btn toolbar-end rb-search"
        onClick={() => setSearch(true)}
        title={`${t("rb.searchTitle")} (${hk("app.commandSearch") || "S"})`}
        aria-label={t("rb.search")}
      >
        <Icon name="search" size={20} />
        <span className="tool-caption">{t("rb.search")}</span>
      </button>
      <button
        type="button"
        className={`tool-btn panel-toggle toolbar-end${p.rightOpen ? " active" : ""}`}
        onClick={p.onToggleRight}
        title={t("tb.propertiesPanel")}
        aria-label={t("tb.propertiesPanel")}
        aria-pressed={p.rightOpen}
      >
        <Icon name="panelRight" size={20} />
        <span className="tool-caption">{p.workspace === "laser" ? t("tb.machine") : t("tb.properties")}</span>
      </button>
    </>
  );

  const ribbonOptions: MenuItem[] = [
    {
      label: t("rb.creationRail"),
      checked: railPref,
      action: () => setCreationRail(!getCreationRail()),
    },
  ];

  const design = p.workspace === "design";
  // For command search (CommandSearch.tsx): the Arrange menu and the ribbon's own buttons.
  usePublish("ribbon.arrangeMenu", { label: t("rb.arrange"), items: design ? arrangeItems : [] });
  usePublish("menu.ribbon", {
    label: t("rb.toolbar"),
    items: [
      ...(design
        ? [
            { label: t("tb.stock"), action: p.onStock },
            { label: t("tb.place"), disabled: !p.hasSelection || !p.hasStock, action: p.onPlace },
            { label: t("tb.array"), disabled: !p.hasSelection, action: p.onArray },
          ]
        : [
            { label: t("tb.preview"), checked: p.showPreview, action: p.onTogglePreview },
            { label: t("tb.frame"), disabled: !!p.designOnly, action: p.onFrame },
          ]),
      { label: t("tb.browserPanel"), checked: p.leftOpen, action: p.onToggleLeft },
      { label: t("tb.propertiesPanel"), checked: p.rightOpen, action: p.onToggleRight },
      { label: t("rb.creationRail"), checked: railPref, action: () => setCreationRail(!getCreationRail()) },
    ] as MenuItem[],
  });
  const railStyle = railTop === null ? undefined : ({ ["--ribbon-rail-top" as string]: `${railTop}px` } as CSSProperties);

  return (
    <>
      {(orientation === "vertical" || creationRail) && <div ref={anchor} className="ribbon-anchor" aria-hidden="true" />}
      <Ribbon
        orientation={orientation}
        label={t("rb.toolbar")}
        className={`rb-ws-${p.workspace}${creationRail ? " has-creation-rail" : ""}`}
        onContextMenu={orientation === "horizontal" ? (x, y) => setRibbonMenu({ x, y }) : undefined}
      >
        {startCap}
        {design ? (
          <>
            {creationRail ? (
              <div className="ribbon-rail-host" style={railStyle}>
                <RibbonGroup id="create" label={t("rb.create")} icon="create" priority={100} pinned />
              </div>
            ) : (
              <RibbonGroup id="create" label={t("rb.create")} icon="create" priority={100} pinned />
            )}
            <RibbonGroup id="modify" label={t("rb.modify")} icon="modify" priority={60} />
            <RibbonGroup id="arrange" label={t("rb.arrange")} icon="arrange" priority={80} />
            <RibbonGroup id="insert" label={t("rb.insert")} icon="insert" priority={40} />
            <RibbonGroup id="inspect" label={t("rb.inspect")} icon="measure" priority={30} />
            <RibbonGroup id="laserTools" label={t("lx.laserTools")} icon="laserTools" priority={20} />
          </>
        ) : (
          <>
            <RibbonGroup id="preview" label={t("tb.preview")} icon="preview" priority={100} pinned>
              <ToolButton icon="preview" label={t("tb.preview")} active={p.showPreview} onClick={p.onTogglePreview} title={t("tb.previewTitle")} />
            </RibbonGroup>
            <RibbonGroup id="job" label={t("rb.job")} icon="run" priority={100} pinned>
              <span className="rb-split">
                <ToolButton icon="frame" label={t("tb.frame")} onClick={p.onFrame} disabled={!!p.designOnly} title={p.designOnly ?? t("tb.frameTitle")} />
                <MenuButton className="tool-btn rb-split-caret" label={t("rb.frameOptions")} ariaLabel={t("rb.frameOptions")} items={frameItems} disabled={!!p.designOnly}>
                  <Icon name="chevronDown" size={12} />
                </MenuButton>
              </span>
              {/* The one Start (laser/StartButton.tsx): Request run with a PIN, Start without; gated like the dock's. */}
              <StartButton variant="toolbar" />
              <ToolButton icon="frameStop" label={t("tb.stopFrame")} onClick={p.onFrameStop} title={t("tb.stopFrameTitle")} />
            </RibbonGroup>
          </>
        )}
        {endCaps}

        {design && (
          <>
            {createTools}
            <RibbonSlot group="modify" order={90}>
              <ToolButton icon="delete" label={t("tb.delete")} onClick={p.onDelete} disabled={!p.hasSelection} title={t("tb.deleteTitle")} />
            </RibbonSlot>
            <RibbonSlot group="arrange" order={0}>
              <MenuButton className="tool-btn et-dropdown" label={t("rb.arrange")} title={t("rb.arrangeTitle")} items={arrangeItems}>
                <Icon name="arrange" size={20} />
                <span className="tool-caption">
                  {t("rb.arrange")}
                  <span className="et-caret" aria-hidden="true">
                    <Icon name="chevronDown" size={10} />
                  </span>
                </span>
              </MenuButton>
              {two && (
                <span className="rb-align-strip" role="group" aria-label={t("rb.align")}>
                  <Mini icon="alignLeft" title={t("tb.alignLeft")} onClick={a("left")} />
                  <Mini icon="alignHCenter" title={t("tb.alignHCenter")} onClick={a("hcenter")} />
                  <Mini icon="alignRight" title={t("tb.alignRight")} onClick={a("right")} />
                  <Mini icon="alignTop" title={t("tb.alignTop")} onClick={a("top")} />
                  <Mini icon="alignVCenter" title={t("tb.alignVCenter")} onClick={a("vcenter")} />
                  <Mini icon="alignBottom" title={t("tb.alignBottom")} onClick={a("bottom")} />
                </span>
              )}
            </RibbonSlot>
            <RibbonSlot group="insert" order={0}>
              <ToolButton icon="import" label={t("tb.import")} onClick={p.onImport} title={t("tb.importTitle")} />
              <ToolButton icon="place" label={t("tb.place")} onClick={p.onPlace} disabled={!p.hasSelection || !p.hasStock} title={p.hasStock ? t("tb.placeTitle") : t("tb.stockFirst")} />
              <ToolButton icon="array" label={t("tb.array")} onClick={p.onArray} disabled={!p.hasSelection} />
            </RibbonSlot>
            <RibbonSlot group="inspect" order={50}>
              <span className="rb-toggles" role="group" aria-label={t("tb.snapAndView")}>
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
                <Mini
                  icon="filled"
                  title={t("tb.filled", { state: p.viewStyle === "filled" ? t("tb.on") : t("tb.off") })}
                  active={p.viewStyle === "filled"}
                  onClick={() => p.onViewStyle(p.viewStyle === "filled" ? "wireframe" : "filled")}
                />
              </span>
            </RibbonSlot>
            {p.extra}
          </>
        )}
      </Ribbon>
      {ribbonMenu && <Menu items={ribbonOptions} anchor={ribbonMenu} onClose={() => setRibbonMenu(null)} label={t("rb.toolbar")} />}
      {search && <CommandSearch onClose={() => setSearch(false)} onEngineConsole={() => P.current.onCommands()} />}
    </>
  );
}
