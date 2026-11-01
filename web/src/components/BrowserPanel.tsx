// Left "Browser" panel, after Fusion's browser tree: Document > Stock, Layers, Objects (grouped by layer, with their
// groups). Clicking selects (Shift adds, Ctrl toggles; Shift+click a layer selects every shape on it, as in
// LightBurn); the eye and output toggles issue `layer.update`; right-click a layer for "show only" / "output only".
// Double-click the document name to rename it.

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import type { GDocument } from "../api/types.ts";
import { selectOpFor, topGroup, type SelectOp, type Selection } from "../selection.ts";
import { ContextMenu, type MenuItem } from "./ContextMenu.tsx";
import { Icon } from "./ui.tsx";

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
  const { doc, selection, run, report } = props;
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [renaming, setRenaming] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; layer: string } | null>(null);
  const toggle = (k: string) => setCollapsed((c) => ({ ...c, [k]: !c[k] }));
  const open = (k: string) => !collapsed[k];

  if (!doc) return <div className="panel-empty">Waiting for the engine…</div>;

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
  const groupName = (id: string) => {
    const g = topGroup(doc, id);
    return g ? (doc.groups?.find((x) => x.id === g)?.name ?? g) : null;
  };

  const layerMenu = (layer: string): MenuItem[] => {
    const others = doc.layers.filter((l) => l.id !== layer);
    const batch = (invs: Invocation[]) => invs.length && run(invs.length === 1 ? invs[0] : invs).catch(report);
    return [
      { label: "Select all shapes in this layer", action: () => props.onSelectLayer(layer, true) },
      { sep: true },
      {
        label: "Hide all layers but this one",
        action: () => batch([C.layerUpdate(layer, { visible: true }), ...others.filter((l) => l.visible).map((l) => C.layerUpdate(l.id, { visible: false }))]),
      },
      { label: "Show all layers", action: () => batch(doc.layers.filter((l) => !l.visible).map((l) => C.layerUpdate(l.id, { visible: true }))) },
      {
        label: "Disable output for all layers but this one",
        action: () => batch([C.layerUpdate(layer, { output: true }), ...others.filter((l) => l.output).map((l) => C.layerUpdate(l.id, { output: false }))]),
      },
      { label: "Enable output for all layers", action: () => batch(doc.layers.filter((l) => !l.output).map((l) => C.layerUpdate(l.id, { output: true }))) },
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
          title={canRename ? "Double-click to rename the document" : undefined}
          strong
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
            <span className="tree-label">{doc.stock ? doc.stock.name || "Stock" : "Stock (none)"}</span>
            {doc.stock?.material && <span className="tree-meta">{doc.stock.material}</span>}
          </div>

          <TreeHeader
            label={`Layers (${doc.layers.length})`}
            open={open("layers")}
            onToggle={() => toggle("layers")}
            action={
              <button className="icon-btn small" title="Add layer" aria-label="Add layer" onClick={addLayer}>
                <Icon name="plus" size={18} />
              </button>
            }
          />
          {open("layers") && (
            <div className="tree-children">
              {doc.layers.map((l) => (
                <div
                  key={l.id}
                  role="treeitem"
                  className={`tree-row${selection.layer === l.id && selection.objects.length === 0 ? " selected" : ""}${l.output ? "" : " no-output"}`}
                  title="Click to edit its settings; Shift+click selects every shape on it; right-click for more"
                  onClick={(e) => props.onSelectLayer(l.id, e.shiftKey)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setMenu({ x: e.clientX, y: e.clientY, layer: l.id });
                  }}
                >
                  <span className="swatch" style={{ background: l.color }} />
                  <span className={`tree-label${l.visible ? "" : " muted"}`}>{l.name}</span>
                  <span className="tree-meta">{l.settings.mode === "line" ? "Line" : l.settings.mode === "fill" ? "Fill" : "Fill+Line"}</span>
                  <button
                    className={`icon-btn small${l.visible ? "" : " off"}`}
                    title={l.visible ? "Hide layer (display only)" : "Show layer"}
                    aria-label={l.visible ? "Hide layer" : "Show layer"}
                    onClick={(e) => {
                      e.stopPropagation();
                      run(C.layerUpdate(l.id, { visible: !l.visible })).catch(report);
                    }}
                  >
                    <Icon name={l.visible ? "eye" : "eyeOff"} size={18} />
                  </button>
                  <button
                    className={`icon-btn small${l.output ? " on-laser" : " off"}`}
                    title={l.output ? "Output on (included in jobs)" : "Output off (excluded from jobs)"}
                    aria-label={l.output ? "Disable output" : "Enable output"}
                    onClick={(e) => {
                      e.stopPropagation();
                      run(C.layerUpdate(l.id, { output: !l.output })).catch(report);
                    }}
                  >
                    <Icon name={l.output ? "laser" : "noLaser"} size={18} />
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
                              className={`tree-row${selection.objects.includes(o.id) ? " selected" : ""}`}
                              onClick={(e) => props.onSelectObject(o.id, selectOpFor({ shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey }))}
                              onContextMenu={(e) => {
                                if (!props.onObjectMenu) return;
                                e.preventDefault();
                                props.onObjectMenu(e.clientX, e.clientY, o.id);
                              }}
                            >
                              <Icon
                                name={o.geometry.kind === "ellipse" ? "ellipse" : o.geometry.kind === "rect" ? "rect" : o.geometry.kind === "image" ? "preview" : "import"}
                                size={16}
                              />
                              <span className="tree-label">{o.name || o.id}</span>
                              {g && (
                                <span className="tree-meta" title="Group">
                                  {g}
                                </span>
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
