// Left "Browser" panel, after Fusion's browser tree: Document > Stock, Layers, Objects (grouped by layer).
// Clicking selects; the eye and output toggles issue `layer.update`.

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import type { GDocument } from "../api/types.ts";
import type { Selection } from "../selection.ts";
import { Icon } from "./ui.tsx";

export interface BrowserPanelProps {
  doc: GDocument | null;
  selection: Selection;
  onSelectObject: (id: string, additive: boolean) => void;
  onSelectLayer: (id: string) => void;
  onSelectStock: () => void;
  run: (inv: Invocation) => Promise<unknown>;
  report: (e: unknown) => void;
}

export function BrowserPanel(props: BrowserPanelProps) {
  const { doc, selection, run, report } = props;
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const toggle = (k: string) => setCollapsed((c) => ({ ...c, [k]: !c[k] }));
  const open = (k: string) => !collapsed[k];

  if (!doc) return <div className="panel-empty">Waiting for the engine…</div>;

  const addLayer = () => {
    // The engine picks the next palette colour.
    run(C.layerAdd(`Layer ${doc.layers.length + 1}`)).catch(report);
  };

  return (
    <div className="tree" role="tree">
      <TreeHeader label={doc.name || "Untitled"} open={open("doc")} onToggle={() => toggle("doc")} strong />
      {open("doc") && (
        <div className="tree-children">
          <div
            role="treeitem"
            className={`tree-row${selection.stock ? " selected" : ""}`}
            onClick={() => doc.stock && props.onSelectStock()}
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
                  className={`tree-row${selection.layer === l.id && selection.objects.length === 0 ? " selected" : ""}`}
                  onClick={() => props.onSelectLayer(l.id)}
                >
                  <span className="swatch" style={{ background: l.color }} />
                  <span className={`tree-label${l.visible ? "" : " muted"}`}>{l.name}</span>
                  <button
                    className={`icon-btn small${l.visible ? "" : " off"}`}
                    title={l.visible ? "Hide layer" : "Show layer"}
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
                    <TreeHeader
                      label={l.name}
                      swatch={l.color}
                      count={objs.length}
                      open={open(key)}
                      onToggle={() => toggle(key)}
                    />
                    {open(key) && (
                      <div className="tree-children">
                        {objs.map((o) => (
                          <div
                            key={o.id}
                            role="treeitem"
                            className={`tree-row${selection.objects.includes(o.id) ? " selected" : ""}`}
                            onClick={(e) => props.onSelectObject(o.id, e.shiftKey || e.metaKey || e.ctrlKey)}
                          >
                            <Icon name={o.geometry.kind === "ellipse" ? "ellipse" : o.geometry.kind === "rect" ? "rect" : "import"} size={16} />
                            <span className="tree-label">{o.name || o.id}</span>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                );
              })}
              {/* Objects whose layer id is unknown (should not happen; shown so nothing is invisible). */}
              {doc.objects
                .filter((o) => !doc.layers.some((l) => l.id === o.layer))
                .map((o) => (
                  <div key={o.id} className={`tree-row${selection.objects.includes(o.id) ? " selected" : ""}`} onClick={() => props.onSelectObject(o.id, false)}>
                    <span className="tree-label muted">{o.name || o.id} (no layer)</span>
                  </div>
                ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function TreeHeader(props: {
  label: string;
  open: boolean;
  onToggle: () => void;
  strong?: boolean;
  swatch?: string;
  count?: number;
  action?: React.ReactNode;
}) {
  return (
    <div className={`tree-head${props.strong ? " strong" : ""}`} onClick={props.onToggle} role="treeitem" aria-expanded={props.open}>
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
