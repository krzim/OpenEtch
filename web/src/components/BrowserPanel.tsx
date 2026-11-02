// Left "Browser" panel, after Fusion's browser tree: Document > Stock, Layers, Objects (grouped by layer, with their
// groups). Clicking selects (Shift adds, Ctrl toggles; Shift+click a layer selects every shape on it, as in
// LightBurn); the eye and output toggles issue `layer.update`; right-click a layer for "show only" / "output only".
// Layers are listed in cut order: drag one by its grip above or below another, or use Move up / Move down in its menu
// (`layer.move`, on engines that have it). Double-click the document name, or its pencil, to rename it.
// Touch (docs/ux/ui-plan-2026-11.md 1.2): a long-press on a row is its right-click, rows have a ⋯ button, the grip
// reorders with pointer events (not HTML5 drag-and-drop), and the touch modifier bar's Add / Toggle work for taps.

import { useRef, useState } from "react";
import { getVirtualMods } from "../touch/virtualMods.ts";
import { useLongPress } from "../touch/useLongPress.ts";
import { useReorder } from "../touch/reorder.ts";
import { TouchIcon } from "../touch/icons.tsx";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import type { GDocument } from "../api/types.ts";
import { selectOpFor, topGroup, type SelectOp, type Selection } from "../selection.ts";
import { objectIconName } from "../snapping.ts";
import { stepIndex } from "../workspace/layersTable.ts";
import { ContextMenu, type MenuItem } from "./ContextMenu.tsx";
import { Icon } from "./ui.tsx";
import { useT } from "../workspace/usePrefs.ts";

export interface BrowserPanelProps {
  doc: GDocument | null;
  selection: Selection;
  has: (cmd: string) => boolean;
  onSelectObject: (id: string, op: SelectOp) => void;
  /** `all`: select every shape on the layer (Shift+click). */
  onSelectLayer: (id: string, all: boolean) => void;
  onSelectStock: () => void;
  /** Right-click on an object row: the app selects it (unless selected) and shows its object menu at (x, y). */
  onObjectMenu?: (x: number, y: number, id: string) => void;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  report: (e: unknown) => void;
}

export function BrowserPanel(props: BrowserPanelProps) {
  const t = useT();
  const { doc, selection, run, report } = props;
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [renaming, setRenaming] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; layer: string } | null>(null);
  const toggle = (k: string) => setCollapsed((c) => ({ ...c, [k]: !c[k] }));
  const open = (k: string) => !collapsed[k];
  const layersRef = useRef<HTMLDivElement>(null);
  const layerCount = doc?.layers.length ?? 0;
  const reorder = useReorder(layersRef, layerCount, (from, to) => {
    const id = doc?.layers[from]?.id;
    if (id) run(C.layerMove(id, to)).catch(report);
  });
  // Long-press (touch) or right-click on a layer or object row: its menu. The row says which it is.
  const longPress = useLongPress((x, y, el) => {
    const id = el.getAttribute("data-id");
    if (!id) return;
    if (el.getAttribute("data-kind") === "layer") setMenu({ x, y, layer: id });
    else props.onObjectMenu?.(x, y, id);
  });
  /** The ⋯ button of a row: the row's menu under the button. */
  const moreAt = (e: React.MouseEvent<HTMLElement>) => {
    e.stopPropagation();
    const r = e.currentTarget.getBoundingClientRect();
    return { x: r.left, y: r.bottom };
  };

  if (!doc) return <div className="panel-empty">{t("br.waitingForTheEngine")}</div>;

  const addLayer = () => {
    // The engine picks the next palette colour.
    run(C.layerAdd(`Layer ${doc.layers.length + 1}`)).catch(report);
  };
  const canRename = props.has(C.OPT.docRename);
  const commitRename = () => {
    const n = (renaming ?? "").trim();
    setRenaming(null);
    if (n && n !== doc.name) run(C.docRename(n)).catch(report);
  };
  const canMove = props.has(C.OPT.layerMove);
  const moveLayer = (id: string, to: number | null) => {
    if (to !== null) run(C.layerMove(id, to)).catch(report);
  };
  const groupName = (id: string) => {
    const g = topGroup(doc, id);
    return g ? (doc.groups?.find((x) => x.id === g)?.name ?? g) : null;
  };

  const layerMenu = (layer: string): MenuItem[] => {
    const others = doc.layers.filter((l) => l.id !== layer);
    const batch = (invs: Invocation[]) => invs.length && run(invs.length === 1 ? invs[0] : invs).catch(report);
    const index = doc.layers.findIndex((l) => l.id === layer);
    const up = stepIndex(index, -1, doc.layers.length);
    const down = stepIndex(index, 1, doc.layers.length);
    const order: MenuItem[] = canMove
      ? [
          { sep: true },
          { label: t("br.moveUpBurnsEarlier"), disabled: up === null, action: () => moveLayer(layer, up) },
          { label: t("br.moveDownBurnsLater"), disabled: down === null, action: () => moveLayer(layer, down) },
        ]
      : [];
    return [
      { label: t("br.selectAllShapesInThisLayer"), action: () => props.onSelectLayer(layer, true) },
      { sep: true },
      {
        label: t("br.hideAllLayersButThisOne"),
        action: () => batch([C.layerUpdate(layer, { visible: true }), ...others.filter((l) => l.visible).map((l) => C.layerUpdate(l.id, { visible: false }))]),
      },
      { label: t("br.showAllLayers"), action: () => batch(doc.layers.filter((l) => !l.visible).map((l) => C.layerUpdate(l.id, { visible: true }))) },
      {
        label: t("br.disableOutputForAllLayersBut"),
        action: () => batch([C.layerUpdate(layer, { output: true }), ...others.filter((l) => l.output).map((l) => C.layerUpdate(l.id, { output: false }))]),
      },
      { label: t("br.enableOutputForAllLayers"), action: () => batch(doc.layers.filter((l) => !l.output).map((l) => C.layerUpdate(l.id, { output: true }))) },
      ...order,
    ];
  };

  return (
    <div className="tree" role="tree">
      {renaming !== null ? (
        <div className="tree-head strong">
          <input
            className="tree-rename"
            autoFocus
            value={renaming}
            maxLength={200}
            onChange={(e) => setRenaming(e.target.value)}
            onBlur={commitRename}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitRename();
              if (e.key === "Escape") setRenaming(null);
            }}
          />
        </div>
      ) : (
        <TreeHeader
          label={doc.name || "Untitled"}
          open={open("doc")}
          onToggle={() => toggle("doc")}
          onDoubleClick={canRename ? () => setRenaming(doc.name) : undefined}
          title={canRename ? t("br.doubleClickToRenameTheDocument") : undefined}
          strong
          action={
            canRename ? (
              <button className="icon-btn small row-rename" title={t("touch.rename")} aria-label={t("touch.rename")} onClick={() => setRenaming(doc.name)}>
                <TouchIcon name="pencil" size={16} />
              </button>
            ) : undefined
          }
        />
      )}
      {open("doc") && (
        <div className="tree-children">
          <div
            role="treeitem"
            className={`tree-row${selection.stock ? " selected" : ""}${doc.stock ? "" : " muted"}`}
            // Always forwarded: right after the Stock dialog the tree may still show the old document.
            onClick={() => props.onSelectStock()}
          >
            <Icon name="stock" size={18} />
            <span className="tree-label">{doc.stock ? doc.stock.name || "Stock" : t("br.stockNone")}</span>
            {doc.stock?.material && <span className="tree-meta">{doc.stock.material}</span>}
          </div>

          <TreeHeader
            label={`Layers (${doc.layers.length})`}
            open={open("layers")}
            onToggle={() => toggle("layers")}
            action={
              <button className="icon-btn small" title={t("br.addLayer")} aria-label={t("br.addLayer")} onClick={addLayer}>
                <Icon name="plus" size={18} />
              </button>
            }
          />
          {open("layers") && (
            <div className="tree-children" ref={layersRef}>
              {doc.layers.map((l, index) => (
                <div
                  key={l.id}
                  role="treeitem"
                  data-reorder-row=""
                  data-kind="layer"
                  data-id={l.id}
                  className={`tree-row${selection.layer === l.id && selection.objects.length === 0 ? " selected" : ""}${l.output ? "" : " no-output"}${reorder.dropClass(index)}${
                    reorder.dragging === index ? " dragging" : ""
                  }`}
                  title={`Click to edit its settings; Shift+click selects every shape on it; ${canMove ? "drag its grip to change the cut order; " : ""}right-click for more`}
                  {...longPress}
                  onClick={(e) => props.onSelectLayer(l.id, e.shiftKey || getVirtualMods().shift)}
                >
                  {canMove && (
                    <button type="button" className="row-grip" title={t("touch.reorder")} aria-label={t("touch.reorder")} onClick={(e) => e.stopPropagation()} {...reorder.grip(index)}>
                      <TouchIcon name="grip" size={16} />
                    </button>
                  )}
                  <span className="swatch" style={{ background: l.color }} />
                  <span className={`tree-label${l.visible ? "" : " muted"}`}>{l.name}</span>
                  <span className="tree-meta">{l.settings.mode === "line" ? "Line" : l.settings.mode === "fill" ? "Fill" : "Fill+Line"}</span>
                  <button
                    className={`icon-btn small${l.visible ? "" : " off"}`}
                    title={l.visible ? t("br.hideLayerDisplayOnly") : t("br.showLayer")}
                    aria-label={l.visible ? t("br.hideLayer") : t("br.showLayer")}
                    onClick={(e) => {
                      e.stopPropagation();
                      run(C.layerUpdate(l.id, { visible: !l.visible })).catch(report);
                    }}
                  >
                    <Icon name={l.visible ? "eye" : "eyeOff"} size={18} />
                  </button>
                  <button
                    className={`icon-btn small${l.output ? " on-laser" : " off"}`}
                    title={l.output ? t("br.outputOnIncludedInJobs") : t("br.outputOffExcludedFromJobs")}
                    aria-label={l.output ? t("br.disableOutput") : t("br.enableOutput")}
                    onClick={(e) => {
                      e.stopPropagation();
                      run(C.layerUpdate(l.id, { output: !l.output })).catch(report);
                    }}
                  >
                    <Icon name={l.output ? "laser" : "noLaser"} size={18} />
                  </button>
                  <button
                    type="button"
                    className="icon-btn small row-more"
                    title={t("touch.rowMenu")}
                    aria-label={t("touch.rowMenu")}
                    onClick={(e) => setMenu({ ...moreAt(e), layer: l.id })}
                  >
                    <TouchIcon name="more" size={18} />
                  </button>
                </div>
              ))}
            </div>
          )}

          <TreeHeader label={`Objects (${doc.objects.length})`} open={open("objects")} onToggle={() => toggle("objects")} />
          {open("objects") && (
            <div className="tree-children">
              {doc.layers.map((l) => {
                const objs = doc.objects.filter((o) => o.layer === l.id);
                if (objs.length === 0) return null;
                const key = `objs:${l.id}`;
                return (
                  <div key={l.id}>
                    <TreeHeader label={l.name} swatch={l.color} count={objs.length} open={open(key)} onToggle={() => toggle(key)} />
                    {open(key) && (
                      <div className="tree-children">
                        {objs.map((o) => {
                          const g = groupName(o.id);
                          return (
                            <div
                              key={o.id}
                              role="treeitem"
                              data-kind="object"
                              data-id={o.id}
                              className={`tree-row${selection.objects.includes(o.id) ? " selected" : ""}`}
                              onClick={(e) => {
                                const v = getVirtualMods();
                                props.onSelectObject(o.id, selectOpFor({ shift: e.shiftKey || v.shift, ctrl: e.ctrlKey || e.metaKey || v.ctrl }));
                              }}
                              {...(props.onObjectMenu ? longPress : {})}
                            >
                              <Icon name={objectIconName(o)} size={16} />
                              <span className="tree-label">{o.name || o.id}</span>
                              {g && (
                                <span className="tree-meta" title={t("br.group")}>
                                  {g}
                                </span>
                              )}
                              {props.onObjectMenu && (
                                <button
                                  type="button"
                                  className="icon-btn small row-more"
                                  title={t("touch.rowMenu")}
                                  aria-label={t("touch.rowMenu")}
                                  onClick={(e) => {
                                    const at = moreAt(e);
                                    props.onObjectMenu?.(at.x, at.y, o.id);
                                  }}
                                >
                                  <TouchIcon name="more" size={18} />
                                </button>
                              )}
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
              {/* Objects whose layer id is unknown (should not happen; shown so nothing is invisible). */}
              {doc.objects
                .filter((o) => !doc.layers.some((l) => l.id === o.layer))
                .map((o) => (
                  <div key={o.id} className={`tree-row${selection.objects.includes(o.id) ? " selected" : ""}`} onClick={() => props.onSelectObject(o.id, "replace")}>
                    <span className="tree-label muted">{o.name || o.id} (no layer)</span>
                  </div>
                ))}
            </div>
          )}
        </div>
      )}
      {menu && <ContextMenu x={menu.x} y={menu.y} items={layerMenu(menu.layer)} onClose={() => setMenu(null)} />}
    </div>
  );
}

function TreeHeader(props: {
  label: string;
  open: boolean;
  onToggle: () => void;
  onDoubleClick?: () => void;
  title?: string;
  strong?: boolean;
  swatch?: string;
  count?: number;
  action?: React.ReactNode;
}) {
  return (
    <div
      className={`tree-head${props.strong ? " strong" : ""}`}
      onClick={props.onToggle}
      onDoubleClick={props.onDoubleClick}
      title={props.title}
      role="treeitem"
      aria-expanded={props.open}
    >
      <span className={`chevron${props.open ? " open" : ""}`}>
        <Icon name="chevron" size={14} />
      </span>
      {props.swatch && <span className="swatch small" style={{ background: props.swatch }} />}
      <span className="tree-label">{props.label}</span>
      {props.count !== undefined && <span className="tree-meta">{props.count}</span>}
      {props.action && (
        <span className="tree-action" onClick={(e) => e.stopPropagation()}>
          {props.action}
        </span>
      )}
    </div>
  );
}
