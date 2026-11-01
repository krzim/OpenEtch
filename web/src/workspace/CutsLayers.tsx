// Cuts / Layers window (LightBurn's Cuts/Layers): one row per layer with #, colour, name, mode, speed/power, passes,
// output and show toggles. A layer with sub-layers shows "Multi" and expands into one row per pass. Click selects the
// layer (Shift+click selects its shapes), double-click opens its cut settings, right-click has the layer menu (show,
// output, select, copy/paste settings, delete), the Output and Show headers toggle every layer, and the quick-edit row
// under the list changes the selected layer's main speed, power, passes, interval and frequency.
//
// The list order is the cut order (jobs burn layer by layer). Drag a row (above or below another), or use the up/down
// buttons or the right-click menu, to reorder: one `layer.move {id, to_index}` each, shown only on engines that have
// it. Every change is a `layer.*` command.

import { useRef, useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { createdIds } from "../api/drop.ts";
import type { GDocument, Layer, MachineStatus } from "../api/types.ts";
import { ContextMenu, type MenuItem } from "../components/ContextMenu.tsx";
import { LayerSection } from "../components/PropertiesPanel.tsx";
import { Icon, NumberField } from "../components/ui.tsx";
import type { Selection } from "../selection.ts";
import { Dock } from "./Dock.tsx";
import { WsModal } from "./WsModal.tsx";
import {
  copyLayerSettings,
  dropIndex,
  invertAll,
  layerRows,
  modeLabel,
  onlyThis,
  pasteLayerSettings,
  setAll,
  stepIndex,
  type LayerSettingsClip,
} from "./layersTable.ts";
import { formatSpeed, speedToDisplay, trimNumber } from "./units.ts";
import { usePrefs, useT } from "./usePrefs.ts";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export interface CutsLayersProps {
  doc: GDocument | null;
  selection: Selection;
  machine: MachineStatus | null;
  has: (cmd: string) => boolean;
  run: Run;
  report: (e: unknown) => void;
  notify: (msg: string, kind?: "info" | "error") => void;
  /** Select the layer (`all`: select every shape on it). */
  onSelectLayer: (id: string, all: boolean) => void;
}

/** Copied layer settings, shared by every Cuts/Layers window in the page. */
let clipboard: LayerSettingsClip | null = null;

export function CutsLayers(p: CutsLayersProps) {
  const t = useT();
  const prefs = usePrefs();
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [, bump] = useState(0);
  const drag = useRef<number | null>(null);
  const [dropAt, setDropAt] = useState<{ index: number; after: boolean } | null>(null);
  const { doc, run, report } = p;

  const batch = (invs: Invocation[]) => {
    if (invs.length) void run(invs.length === 1 ? invs[0] : invs).catch(report);
  };

  if (!doc) return null;
  const rows = layerRows(doc);
  const current = p.selection.layer;
  const canMove = p.has(C.OPT.layerMove);
  const canSub = p.has(C.OPT.layerSubAdd) && p.has(C.OPT.layerSubDelete);
  const editLayer = editing ? doc.layers.find((l) => l.id === editing) : undefined;
  const delLayer = confirmDelete ? doc.layers.find((l) => l.id === confirmDelete) : undefined;

  const moveLayer = (id: string, to: number | null) => {
    if (to === null || to < 0 || to >= doc.layers.length) return;
    void run(C.layerMove(id, to)).catch(report);
  };
  const endDrag = () => {
    drag.current = null;
    setDropAt(null);
  };
  const dropHalf = (e: React.DragEvent<HTMLElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    return e.clientY > r.top + r.height / 2;
  };

  const copy = (l: Layer) => {
    clipboard = copyLayerSettings(l);
    bump((n) => n + 1);
    p.notify(t("layers.copied", { name: l.name }));
  };
  const paste = (l: Layer) => {
    if (!clipboard) return;
    if (clipboard.subs.length && !canSub) p.notify(t("layers.pasteNoSubs"), "error");
    batch(pasteLayerSettings(l, clipboard, canSub));
  };

  const addLayer = () => {
    run(C.layerAdd(`C${String(doc.layers.length).padStart(2, "0")}`))
      .then((r) => {
        const id = createdIds(r)[0];
        if (id) p.onSelectLayer(id, false);
      })
      .catch(report);
  };

  const headerMenu = (field: "visible" | "output"): MenuItem[] => [
    { label: field === "output" ? t("layers.outputAll") : t("layers.showAll"), action: () => batch(setAll(doc.layers, field, true)) },
    { label: field === "output" ? t("layers.outputNone") : t("layers.hideAll"), action: () => batch(setAll(doc.layers, field, false)) },
    { label: field === "output" ? t("layers.invertOutput") : t("layers.invertShow"), action: () => batch(invertAll(doc.layers, field)) },
  ];

  const layerMenu = (l: Layer, index: number): MenuItem[] => {
    const others = doc.layers.filter((x) => x.id !== l.id);
    const items: MenuItem[] = [
      { label: t("layers.settings"), hint: t("layers.doubleClick"), action: () => setEditing(l.id) },
      { label: t("layers.selectShapes"), hint: t("layers.shiftClick"), action: () => p.onSelectLayer(l.id, true) },
      { sep: true },
      { label: t("layers.showOnly"), action: () => batch(onlyThis(doc.layers, l.id, "visible")) },
      { label: t("layers.showAll"), action: () => batch(setAll(doc.layers, "visible", true)) },
      { label: t("layers.hideAll"), action: () => batch(setAll(doc.layers, "visible", false)) },
      { sep: true },
      { label: t("layers.outputOnly"), action: () => batch(onlyThis(doc.layers, l.id, "output")) },
      { label: t("layers.outputAll"), action: () => batch(setAll(doc.layers, "output", true)) },
      { label: t("layers.outputNone"), action: () => batch(setAll(doc.layers, "output", false)) },
      { label: t("layers.invertOutput"), action: () => batch(invertAll(doc.layers, "output")) },
      { sep: true },
      { label: t("layers.copySettings"), action: () => copy(l) },
      {
        label: clipboard ? t("layers.pasteSettingsFrom", { name: clipboard.from }) : t("layers.pasteSettings"),
        disabled: !clipboard,
        action: () => paste(l),
      },
    ];
    if (canMove) {
      items.push(
        { sep: true },
        { label: t("layers.moveUp"), disabled: stepIndex(index, -1, doc.layers.length) === null, action: () => moveLayer(l.id, stepIndex(index, -1, doc.layers.length)) },
        { label: t("layers.moveDown"), disabled: stepIndex(index, 1, doc.layers.length) === null, action: () => moveLayer(l.id, stepIndex(index, 1, doc.layers.length)) },
      );
    }
    items.push(
      { sep: true },
      { label: t("layers.delete"), disabled: !p.has(C.OPT.layerDelete) || others.length === 0, action: () => setConfirmDelete(l.id) },
    );
    return items;
  };

  const sel = current ? doc.layers.find((l) => l.id === current) : undefined;
  const patch = (l: Layer, settings: C.CutSettingsPatch) => void run(C.layerUpdate(l.id, { settings })).catch(report);

  return (
    <Dock
      id="cuts"
      title={t("layers.title")}
      actions={
        <>
          <button className="icon-btn small" title={t("layers.add")} aria-label={t("layers.add")} onClick={addLayer}>
            <Icon name="plus" size={18} />
          </button>
        </>
      }
    >
      <div className="cuts-wrap">
        <table className="cuts-table">
          <thead>
            <tr>
              <th className="c-num">#</th>
              <th className="c-sw" aria-label={t("layers.colour")} />
              <th className="c-name">{t("layers.name")}</th>
              <th>{t("layers.mode")}</th>
              <th title={t("layers.spdPwrTitle", { unit: prefs.speedUnit })}>{t("layers.spdPwr")}</th>
              <th title={t("layers.passes")}>#P</th>
              <th className="c-tog">
                <button
                  className="th-btn"
                  title={t("layers.outputHeader")}
                  onClick={(e) => setMenu({ x: e.clientX, y: e.clientY, items: headerMenu("output") })}
                >
                  {t("layers.output")}
                </button>
              </th>
              <th className="c-tog">
                <button className="th-btn" title={t("layers.showHeader")} onClick={(e) => setMenu({ x: e.clientX, y: e.clientY, items: headerMenu("visible") })}>
                  {t("layers.show")}
                </button>
              </th>
              {canMove && <th className="c-tog" aria-label={t("layers.order")} />}
            </tr>
          </thead>
          <tbody>
            {rows.map((r, index) => {
              const l = r.layer;
              const open = !!expanded[l.id];
              const main = r.passes[0];
              return [
                <tr
                  key={l.id}
                  className={`cuts-row${current === l.id ? " selected" : ""}${l.output ? "" : " no-output"}${l.visible ? "" : " hidden-layer"}${
                    dropAt?.index === index ? (dropAt.after ? " drop-after" : " drop-before") : ""
                  }`}
                  title={canMove ? t("layers.rowTitleMove", { count: r.objects }) : t("layers.rowTitle", { count: r.objects })}
                  draggable={canMove}
                  onDragStart={(e) => {
                    drag.current = index;
                    e.dataTransfer.effectAllowed = "move";
                    // Firefox starts a drag only with data set.
                    e.dataTransfer.setData("text/plain", l.name);
                  }}
                  onDragOver={(e) => {
                    if (!canMove || drag.current === null) return;
                    e.preventDefault();
                    e.dataTransfer.dropEffect = "move";
                    const after = dropHalf(e);
                    if (dropAt?.index !== index || dropAt.after !== after) setDropAt({ index, after });
                  }}
                  onDrop={(e) => {
                    e.preventDefault();
                    const from = drag.current;
                    if (from !== null) {
                      const id = rows[from]?.layer.id;
                      if (id) moveLayer(id, dropIndex(from, index, dropHalf(e), rows.length));
                    }
                    endDrag();
                  }}
                  onDragEnd={endDrag}
                  onClick={(e) => p.onSelectLayer(l.id, e.shiftKey)}
                  onDoubleClick={() => setEditing(l.id)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setMenu({ x: e.clientX, y: e.clientY, items: layerMenu(l, index) });
                  }}
                >
                  <td className="c-num">{r.number}</td>
                  <td className="c-sw">
                    <span className="swatch small" style={{ background: l.color }} />
                  </td>
                  <td className="c-name">
                    {r.multi && (
                      <button
                        className={`chevron-btn${open ? " open" : ""}`}
                        aria-expanded={open}
                        aria-label={open ? t("layers.collapse") : t("layers.expand")}
                        onClick={(e) => {
                          e.stopPropagation();
                          setExpanded((x) => ({ ...x, [l.id]: !open }));
                        }}
                      >
                        <Icon name="chevron" size={12} />
                      </button>
                    )}
                    <span className="cuts-name">{l.name}</span>
                  </td>
                  <td>{r.multi ? t("layers.multi") : modeLabel(main.mode)}</td>
                  <td className="c-num-r">{r.multi ? t("layers.multi") : `${trimNumber(speedToDisplay(main.speed_mm_s, prefs.speedUnit), 1)} / ${trimNumber(main.power_pct, 1)}`}</td>
                  <td className="c-num-r">{r.passes.reduce((n, x) => n + x.passes, 0)}</td>
                  <td className="c-tog">
                    <button
                      className={`tog${l.output ? " on" : ""}`}
                      role="switch"
                      aria-checked={l.output}
                      title={l.output ? t("layers.outputOn") : t("layers.outputOff")}
                      onClick={(e) => {
                        e.stopPropagation();
                        void run(C.layerUpdate(l.id, { output: !l.output })).catch(report);
                      }}
                    >
                      <Icon name={l.output ? "laser" : "noLaser"} size={16} />
                    </button>
                  </td>
                  <td className="c-tog">
                    <button
                      className={`tog${l.visible ? " on" : ""}`}
                      role="switch"
                      aria-checked={l.visible}
                      title={l.visible ? t("layers.shown") : t("layers.hidden")}
                      onClick={(e) => {
                        e.stopPropagation();
                        void run(C.layerUpdate(l.id, { visible: !l.visible })).catch(report);
                      }}
                    >
                      <Icon name={l.visible ? "eye" : "eyeOff"} size={16} />
                    </button>
                  </td>
                  {canMove && (
                    <td className="c-tog c-order">
                      <button
                        className="tog"
                        disabled={index === 0}
                        title={t("layers.moveUpTitle")}
                        aria-label={t("layers.moveUp")}
                        onClick={(e) => {
                          e.stopPropagation();
                          moveLayer(l.id, stepIndex(index, -1, rows.length));
                        }}
                      >
                        <Icon name="chevronUp" size={14} />
                      </button>
                      <button
                        className="tog"
                        disabled={index === rows.length - 1}
                        title={t("layers.moveDownTitle")}
                        aria-label={t("layers.moveDown")}
                        onClick={(e) => {
                          e.stopPropagation();
                          moveLayer(l.id, stepIndex(index, 1, rows.length));
                        }}
                      >
                        <Icon name="chevronDown" size={14} />
                      </button>
                    </td>
                  )}
                </tr>,
                ...(r.multi && open
                  ? r.passes.map((ps) => (
                      <tr key={`${l.id}#${ps.pass}`} className="cuts-sub" onClick={() => p.onSelectLayer(l.id, false)} onDoubleClick={() => setEditing(l.id)}>
                        <td />
                        <td />
                        <td className="c-name">
                          <span className="cuts-sub-name">
                            {ps.name}
                            {ps.cleanup ? ` · ${t("layers.cleanup")}` : ""}
                          </span>
                        </td>
                        <td>{modeLabel(ps.mode)}</td>
                        <td className="c-num-r">{`${trimNumber(speedToDisplay(ps.speed_mm_s, prefs.speedUnit), 1)} / ${trimNumber(ps.power_pct, 1)}`}</td>
                        <td className="c-num-r">{ps.passes}</td>
                        <td />
                        <td />
                        {canMove && <td />}
                      </tr>
                    ))
                  : []),
              ];
            })}
          </tbody>
        </table>
        {doc.layers.length === 0 && <p className="hint">{t("layers.empty")}</p>}
      </div>

      {sel && (
        <div className="cuts-quick" aria-label={t("layers.quickEdit")}>
          <div className="cuts-quick-head">
            <span className="swatch small" style={{ background: sel.color }} />
            <span className="cuts-quick-name">{sel.name}</span>
            <span className="muted">{(sel.sub_layers?.length ?? 0) > 0 ? t("layers.mainOnly") : ""}</span>
            <span className="tool-spacer" />
            <button className="btn small" onClick={() => copy(sel)} title={t("layers.copySettings")}>
              {t("layers.copy")}
            </button>
            <button className="btn small" disabled={!clipboard} onClick={() => paste(sel)} title={clipboard ? t("layers.pasteSettingsFrom", { name: clipboard.from }) : t("layers.pasteSettings")}>
              {t("layers.paste")}
            </button>
            <button className="btn small" onClick={() => setEditing(sel.id)}>
              {t("layers.more")}
            </button>
          </div>
          <div className="field-grid cuts-quick-grid">
            <NumberField label={t("layers.speed")} unit="mm/s" value={sel.settings.speed_mm_s} min={0.01} onCommit={(v) => v !== null && patch(sel, { speed_mm_s: v })} />
            <NumberField label={t("layers.power")} unit="%" value={sel.settings.power_pct} min={0} max={100} onCommit={(v) => v !== null && patch(sel, { power_pct: v })} />
            <NumberField label={t("layers.passes")} value={sel.settings.passes} min={1} max={1000} digits={0} onCommit={(v) => v !== null && patch(sel, { passes: Math.round(v) })} />
            <NumberField label={t("layers.frequency")} unit="kHz" value={sel.settings.frequency_khz} min={1} onCommit={(v) => v !== null && patch(sel, { frequency_khz: v })} />
            {sel.settings.mode !== "line" && (
              <NumberField
                label={t("layers.interval")}
                unit="mm"
                value={sel.settings.hatch.interval_mm}
                min={0.001}
                digits={4}
                onCommit={(v) => v !== null && patch(sel, { hatch: { interval_mm: v } })}
              />
            )}
          </div>
          <p className="hint">{t("layers.speedHint", { speed: formatSpeed(sel.settings.speed_mm_s, prefs.speedUnit) })}</p>
        </div>
      )}

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}
      {editLayer && (
        <WsModal title={t("layers.settingsTitle", { name: editLayer.name })} wide onClose={() => setEditing(null)}>
          <LayerSection layer={editLayer} machine={p.machine} run={run} report={report} doc={doc} has={p.has} />
        </WsModal>
      )}
      {delLayer && (
        <DeleteLayerDialog
          layer={delLayer}
          doc={doc}
          onClose={() => setConfirmDelete(null)}
          onDelete={(reassign) => {
            setConfirmDelete(null);
            void run(C.layerDelete(delLayer.id, reassign)).catch(report);
          }}
        />
      )}
    </Dock>
  );
}

function DeleteLayerDialog(p: { layer: Layer; doc: GDocument; onClose: () => void; onDelete: (reassign?: string) => void }) {
  const t = useT();
  const others = p.doc.layers.filter((l) => l.id !== p.layer.id);
  const count = p.doc.objects.filter((o) => o.layer === p.layer.id).length;
  const [to, setTo] = useState(others[0]?.id ?? "");
  return (
    <WsModal
      title={t("layers.deleteTitle", { name: p.layer.name })}
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-warn" disabled={count > 0 && !to} onClick={() => p.onDelete(count ? to : undefined)}>
            {t("layers.delete")}
          </button>
        </>
      }
    >
      {count > 0 ? (
        <label className="field">
          <span className="field-label">{t("layers.moveObjectsTo", { count })}</span>
          <span className="field-input">
            <select value={to} onChange={(e) => setTo(e.target.value)}>
              {others.map((l) => (
                <option key={l.id} value={l.id}>
                  {l.name}
                </option>
              ))}
            </select>
          </span>
        </label>
      ) : (
        <p>{t("layers.deleteEmpty")}</p>
      )}
    </WsModal>
  );
}
