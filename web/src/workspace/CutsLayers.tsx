// Cuts / Layers window (LightBurn's Cuts/Layers): one row per layer with #, colour, name, mode, speed/power, passes,
// output and show toggles. A layer with sub-layers shows "Multi" and expands into one row per pass. Click selects the
// layer (Shift+click selects its shapes), double-click opens its cut settings, right-click has the layer menu (show,
// output, select, copy/paste settings, delete), the Output and Show headers toggle every layer, and the quick-edit row
// under the list changes the selected layer's main speed, power, passes, interval and frequency.
//
// The list order is the cut order (jobs burn layer by layer). Drag a row by its grip (above or below another), or use
// the up/down buttons or the right-click menu, to reorder: one `layer.move {id, to_index}` each, shown only on engines
// that have it. Every change is a `layer.*` command. On touch a long-press is the right-click, each row has a ⋯ button,
// the grip works with a finger (pointer events, touch/reorder.ts), and the modifier bar's Add counts as Shift.
//
// LightBurn's further details: "Flash layer" in the right-click menu (or Shift+right-click) blinks the layer's shapes
// on the canvas (flash.tsx); tool layers T1 / T2 (`Layer.tool`) read "Tool", never output and can be made or cleared
// from the menu; a layer imported from LightBurn with settings OpenEtch will not execute gets a warning badge listing
// them (GET /api/layers/unsupported); a layer with sub-layers shows a tab per sub-layer above the quick-edit fields
// (LightBurn's sub-layer tabs): the fields edit the chosen tab's pass, and dragging a tab reorders the sub-layers (with
// the mouse anywhere on the tab, or by its grip with a finger, a pen or the mouse: pointer events, touch/reorder.ts).
//
// Narrow windows (a 300 px dock, the phone's Layers sheet): the table drops its least important columns (up/down,
// passes, mode, then speed/power; layersTable.ts cutsColumns) and shows their values in a second line under each
// layer's name, so it never scrolls sideways. On a coarse pointer the quick-edit numbers get -/+ steppers.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { createdIds } from "../api/drop.ts";
import { frequencyRange, sourceFields } from "../api/laser.ts";
import type { GDocument, Layer, MachineStatus } from "../api/types.ts";
import { ContextMenu, type MenuItem } from "../components/ContextMenu.tsx";
import { LayerSection } from "../components/PropertiesPanel.tsx";
import { Icon, NumberField } from "../components/ui.tsx";
import type { Selection } from "../selection.ts";
import { Dock } from "./Dock.tsx";
import { flashLayer } from "./flash.tsx";
import { hasLightBurnExtras, unsupportedText, wsApi, type UnsupportedSetting } from "./wsApi.ts";
import { layerPresetMenuItems } from "../libs/LayerDefaults.tsx";
import { LinkIcon } from "../libs/MaterialLibrary.tsx";
import { layerLink } from "../libs/model.ts";
import { WsModal } from "./WsModal.tsx";
import {
  copyLayerSettings,
  cutsColumns,
  cutsIconHeads,
  invertAll,
  layerRows,
  onlyThis,
  pasteLayerSettings,
  setAll,
  stepIndex,
  type LayerSettingsClip,
} from "./layersTable.ts";
import { formatSpeed, speedToDisplay, trimNumber } from "./units.ts";
import { usePrefs, useT } from "./usePrefs.ts";
import { useLayoutMode } from "./layoutMode.ts";
import { getVirtualMods } from "../touch/virtualMods.ts";
import { useLongPress } from "../touch/useLongPress.ts";
import { useReorder } from "../touch/reorder.ts";

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
  // The quick-edit tab (0 = main settings, n = sub-layer n) per layer, and the sub-layer tab being dragged.
  const [passTab, setPassTab] = useState<Record<string, number>>({});
  const tabDrag = useRef<number | null>(null);
  const { doc, run, report } = p;
  const lm = useLayoutMode();
  const coarse = lm.pointer === "coarse" || lm.density === "touch";

  // The table's width decides which columns it shows (layersTable.ts cutsColumns).
  const [wrapEl, setWrapEl] = useState<HTMLDivElement | null>(null);
  const [wrapW, setWrapW] = useState(Infinity);
  useLayoutEffect(() => {
    if (!wrapEl) return;
    const measure = () => setWrapW(wrapEl.clientWidth || Infinity);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(wrapEl);
    return () => ro.disconnect();
  }, [wrapEl]);

  // Sub-layer tabs: reordered by their grips (pointer events, so a finger works too; the mouse can also drag the tab).
  const tabsRef = useRef<HTMLDivElement>(null);
  const subOf = doc && p.selection.layer ? doc.layers.find((l) => l.id === p.selection.layer) : undefined;
  const subLayerId = useRef<string | null>(null);
  subLayerId.current = subOf?.id ?? null;
  const subReorder = useReorder(
    tabsRef,
    subOf?.sub_layers?.length ?? 0,
    (from, to) => {
      const id = subLayerId.current;
      if (!id) return;
      // The moved tab stays selected (tab n is sub-layer n - 1).
      void run(C.layerSubMove(id, from, to))
        .then(() => setPassTab((x) => ({ ...x, [id]: to + 1 })))
        .catch(report);
    },
    "row",
  );

  // LightBurn settings the engine will not execute, per layer: asked again when the document changes, and only for
  // documents with LightBurn layer settings in them.
  const [unsupported, setUnsupported] = useState<Map<string, UnsupportedSetting[]>>(() => new Map());
  const lbExtras = !!doc && hasLightBurnExtras(doc.layers);
  const revision = doc?.revision;
  useEffect(() => {
    if (!lbExtras) {
      setUnsupported((m) => (m.size ? new Map() : m));
      return;
    }
    let alive = true;
    wsApi
      .unsupported()
      .then((m) => alive && setUnsupported(m))
      .catch(() => alive && setUnsupported(new Map()));
    return () => {
      alive = false;
    };
  }, [lbExtras, revision]);

  const batch = (invs: Invocation[]) => {
    if (invs.length) void run(invs.length === 1 ? invs[0] : invs).catch(report);
  };

  // Reordering by the grip (mouse, pen or finger) and the row menu by long-press or right-click.
  const bodyRef = useRef<HTMLTableSectionElement>(null);
  const reorder = useReorder(bodyRef, doc?.layers.length ?? 0, (from, to) => {
    const id = doc?.layers[from]?.id;
    if (id) void run(C.layerMove(id, to)).catch(report);
  });
  const menuFor = useRef<(id: string, x: number, y: number) => void>(() => {});
  const longPress = useLongPress((x, y, el) => {
    const id = el.getAttribute("data-id");
    if (id) menuFor.current(id, x, y);
  });

  if (!doc) return null;
  const rows = layerRows(doc);
  const current = p.selection.layer;
  const canMove = p.has(C.OPT.layerMove);
  const cols = cutsColumns(wrapW, { canMove, coarse });
  const iconHeads = cutsIconHeads(wrapW, { canMove, coarse });
  const show = (c: Parameters<typeof cols.has>[0]) => cols.has(c);
  const canSub = p.has(C.OPT.layerSubAdd) && p.has(C.OPT.layerSubDelete);
  const editLayer = editing ? doc.layers.find((l) => l.id === editing) : undefined;
  const delLayer = confirmDelete ? doc.layers.find((l) => l.id === confirmDelete) : undefined;

  const moveLayer = (id: string, to: number | null) => {
    if (to === null || to < 0 || to >= doc.layers.length) return;
    void run(C.layerMove(id, to)).catch(report);
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

  const canTool = p.has(C.OPT.layerTool);
  const toolItems = (l: Layer): MenuItem[] => {
    if (!canTool) return [];
    const owner = (tool: "T1" | "T2") => doc.layers.find((x) => x.tool === tool && x.id !== l.id);
    const make = (tool: "T1" | "T2"): MenuItem => ({
      label: t("layers.makeTool", { tool }),
      disabled: l.tool === tool || !!owner(tool),
      action: () => void run(C.layerTool(tool, l.id)).catch(report),
    });
    return [{ sep: true }, make("T1"), make("T2"), { label: t("layers.makeOrdinary"), disabled: !l.tool, action: () => void run(C.layerTool(null, l.id)).catch(report) }];
  };

  const layerMenu = (l: Layer, index: number): MenuItem[] => {
    const others = doc.layers.filter((x) => x.id !== l.id);
    const items: MenuItem[] = [
      { label: t("layers.settings"), hint: t("layers.doubleClick"), action: () => setEditing(l.id) },
      { label: t("layers.selectShapes"), hint: t("layers.shiftClick"), action: () => p.onSelectLayer(l.id, true) },
      { label: t("layers.flash"), hint: t("layers.shiftRightClick"), action: () => flashLayer(l.id) },
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
    items.push(...toolItems(l));
    // Make Default / Reset / Library / Unlink (src/libs/LayerDefaults.tsx).
    items.push(...layerPresetMenuItems(l, { t, run, notify: p.notify, report }));
    items.push(
      { sep: true },
      { label: t("layers.delete"), disabled: !p.has(C.OPT.layerDelete) || others.length === 0, action: () => setConfirmDelete(l.id) },
    );
    return items;
  };

  menuFor.current = (id, x, y) => {
    const index = doc.layers.findIndex((l) => l.id === id);
    if (index >= 0) setMenu({ x, y, items: layerMenu(doc.layers[index], index) });
  };

  const sel = current ? doc.layers.find((l) => l.id === current) : undefined;
  const subCount = sel?.sub_layers?.length ?? 0;
  // The quick-edit pass: 0 = the main settings, n = sub-layer n (kept in range when sub-layers are deleted).
  const pass = sel ? Math.min(passTab[sel.id] ?? 0, subCount) : 0;
  const passSettings = sel ? (pass === 0 ? sel.settings : sel.sub_layers[pass - 1]) : undefined;
  const fields = sourceFields(p.machine);
  const freq = frequencyRange(p.machine);
  const patch = (l: Layer, settings: C.CutSettingsPatch) =>
    void run(pass === 0 ? C.layerUpdate(l.id, { settings }) : C.layerSubUpdate(l.id, pass - 1, { settings })).catch(report);
  const modeText = (m: string) =>
    m === "line" ? t("layers.modeLine") : m === "fill" ? t("layers.modeFill") : m === "offset_fill" ? t("layers.modeOffset") : t("layers.modeFillLine");
  const passName = (l: Layer, n: number) => (n === 0 ? t("layers.main") : l.sub_layers[n - 1]?.name?.trim() || t("layers.subN", { n }));
  const subLabel = (n: number) => t("layers.subN", { n });
  const spdPwr = (speed: number, power: number) => `${trimNumber(speedToDisplay(speed, prefs.speedUnit), 1)} / ${trimNumber(power, 1)}`;
  // The values of the columns a narrow table leaves out, as a second line under the name ("Fill · 100 / 20 · ×2").
  const detail = (mode: string, sp: string, passes: number) => {
    const parts: string[] = [];
    if (!show("mode")) parts.push(mode);
    if (!show("spdPwr") && sp !== parts[parts.length - 1]) parts.push(sp);
    if (!show("passes")) parts.push(`×${passes}`);
    return parts.length ? <span className="cuts-detail">{parts.join(" · ")}</span> : null;
  };

  return (
    <Dock
      help="layers-and-settings#cuts-layers-window"
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
      <div className="cuts-wrap" ref={setWrapEl}>
        <table className="cuts-table">
          <thead>
            <tr>
              <th className="c-num">#</th>
              <th className="c-sw" aria-label={t("layers.colour")} />
              <th className="c-name">{t("layers.name")}</th>
              {show("mode") && <th>{t("layers.mode")}</th>}
              {show("spdPwr") && <th title={t("layers.spdPwrTitle", { unit: prefs.speedUnit })}>{t("layers.spdPwr")}</th>}
              {show("passes") && <th title={t("layers.passes")}>#P</th>}
              <th className="c-tog">
                <button
                  className="th-btn"
                  title={t("layers.outputHeader")}
                  aria-label={t("layers.output")}
                  onClick={(e) => setMenu({ x: e.clientX, y: e.clientY, items: headerMenu("output") })}
                >
                  {iconHeads ? <Icon name="laser" size={14} /> : t("layers.output")}
                </button>
              </th>
              <th className="c-tog">
                <button
                  className="th-btn"
                  title={t("layers.showHeader")}
                  aria-label={t("layers.show")}
                  onClick={(e) => setMenu({ x: e.clientX, y: e.clientY, items: headerMenu("visible") })}
                >
                  {iconHeads ? <Icon name="eye" size={14} /> : t("layers.show")}
                </button>
              </th>
              {show("order") && <th className="c-tog" aria-label={t("layers.order")} />}
            </tr>
          </thead>
          <tbody ref={bodyRef}>
            {rows.map((r, index) => {
              const l = r.layer;
              const open = !!expanded[l.id];
              const main = r.passes[0];
              const modeCell = l.tool ? t("layers.modeTool") : r.multi ? t("layers.multi") : modeText(main.mode);
              const spdPwrCell = l.tool ? "–" : r.multi ? t("layers.multi") : spdPwr(main.speed_mm_s, main.power_pct);
              const passCount = r.passes.reduce((n, x) => n + x.passes, 0);
              return [
                <tr
                  key={l.id}
                  data-reorder-row=""
                  data-id={l.id}
                  className={`cuts-row${current === l.id ? " selected" : ""}${l.output ? "" : " no-output"}${l.visible ? "" : " hidden-layer"}${reorder.dropClass(index)}${
                    reorder.dragging === index ? " dragging" : ""
                  }`}
                  title={canMove ? t("layers.rowTitleMove", { count: r.objects }) : t("layers.rowTitle", { count: r.objects })}
                  {...longPress}
                  onClick={(e) => p.onSelectLayer(l.id, e.shiftKey || getVirtualMods().shift)}
                  onDoubleClick={() => setEditing(l.id)}
                  onContextMenu={(e) => {
                    // LightBurn: Shift+right-click flashes the layer's shapes.
                    if (e.shiftKey) {
                      e.preventDefault();
                      flashLayer(l.id);
                    } else longPress.onContextMenu?.(e);
                  }}
                >
                  <td className="c-num">
                    {canMove && (
                      <button type="button" className="row-grip" title={t("touch.reorder")} aria-label={t("touch.reorder")} onClick={(e) => e.stopPropagation()} {...reorder.grip(index)}>
                        <Icon name="grip" size={14} />
                      </button>
                    )}
                    {r.number}
                  </td>
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
                    {l.tool && (
                      <span className="tool-badge" title={t("layers.toolTitle", { tool: l.tool })}>
                        {l.tool}
                      </span>
                    )}
                    {unsupportedText(unsupported.get(l.id) ?? [], subLabel) && (
                      <span
                        className="unsup-badge"
                        role="img"
                        aria-label={t("layers.unsupported")}
                        title={`${t("layers.unsupported")}\n${unsupportedText(unsupported.get(l.id) ?? [], subLabel)}`}
                      >
                        !
                      </span>
                    )}
                    {layerLink(l) && (
                      <span className="lx-link-badge" title={t("lib.layerLinked", { path: layerLink(l)!.path, library: layerLink(l)!.library })}>
                        <LinkIcon />
                      </span>
                    )}
                    <button
                      type="button"
                      className="icon-btn small row-more"
                      // A double-tap on ⋯ is two presses of ⋯, not the row's double-click (which opens its settings).
                      onDoubleClick={(e) => e.stopPropagation()}
                      title={t("touch.rowMenu")}
                      aria-label={t("touch.rowMenu")}
                      onClick={(e) => {
                        e.stopPropagation();
                        const b = e.currentTarget.getBoundingClientRect();
                        setMenu({ x: b.left, y: b.bottom, items: layerMenu(l, index) });
                      }}
                    >
                      <Icon name="more" size={16} />
                    </button>
                    {detail(modeCell, spdPwrCell, passCount)}
                  </td>
                  {show("mode") && <td>{modeCell}</td>}
                  {show("spdPwr") && <td className="c-num-r">{spdPwrCell}</td>}
                  {show("passes") && <td className="c-num-r">{passCount}</td>}
                  <td className="c-tog">
                    <button
                      className={`tog${l.output && !l.tool ? " on" : ""}`}
                      role="switch"
                      aria-checked={l.output && !l.tool}
                      // A tool layer never outputs (the engine refuses output on for it).
                      disabled={!!l.tool}
                      title={l.tool ? t("layers.toolNoOutput") : l.output ? t("layers.outputOn") : t("layers.outputOff")}
                      onClick={(e) => {
                        e.stopPropagation();
                        void run(C.layerUpdate(l.id, { output: !l.output })).catch(report);
                      }}
                    >
                      <Icon name={l.output && !l.tool ? "laser" : "noLaser"} size={16} />
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
                  {show("order") && (
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
                      <tr
                        key={`${l.id}#${ps.pass}`}
                        className={`cuts-sub${current === l.id && pass === ps.pass ? " selected" : ""}`}
                        title={t("layers.subRowTitle")}
                        onClick={() => {
                          // A sub-layer row selects the layer and that pass's quick-edit tab.
                          p.onSelectLayer(l.id, false);
                          setPassTab((x) => ({ ...x, [l.id]: ps.pass }));
                        }}
                        onDoubleClick={() => setEditing(l.id)}
                      >
                        <td />
                        <td />
                        <td className="c-name">
                          <span className="cuts-sub-name">
                            {passName(l, ps.pass)}
                            {ps.cleanup ? ` · ${t("layers.cleanup")}` : ""}
                          </span>
                          {detail(modeText(ps.mode), spdPwr(ps.speed_mm_s, ps.power_pct), ps.passes)}
                        </td>
                        {show("mode") && <td>{modeText(ps.mode)}</td>}
                        {show("spdPwr") && <td className="c-num-r">{spdPwr(ps.speed_mm_s, ps.power_pct)}</td>}
                        {show("passes") && <td className="c-num-r">{ps.passes}</td>}
                        <td />
                        <td />
                        {show("order") && <td />}
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
            {/* The mode, which a narrow table leaves out of its columns. */}
            <span className="muted">{sel.tool ? t("layers.modeTool") : passSettings ? modeText(passSettings.mode) : ""}</span>
            {/* The buttons move under the name together when the dock is too narrow for both (the name read "Lay…"
                in a 320 px dock). */}
            <span className="cuts-quick-actions">
              <button className="btn small" onClick={() => copy(sel)} title={t("layers.copySettings")}>
                {t("layers.copy")}
              </button>
              <button className="btn small" disabled={!clipboard} onClick={() => paste(sel)} title={clipboard ? t("layers.pasteSettingsFrom", { name: clipboard.from }) : t("layers.pasteSettings")}>
                {t("layers.paste")}
              </button>
              <button className="btn small" onClick={() => setEditing(sel.id)}>
                {t("layers.more")}
              </button>
            </span>
          </div>
          {subCount > 0 && (
            <div className="cuts-tabs" role="tablist" aria-label={t("layers.subTabs")} ref={tabsRef}>
              {[0, ...sel.sub_layers.map((_, i) => i + 1)].map((n) => {
                const movable = n > 0 && p.has(C.OPT.layerSubMove) && !layerLink(sel);
                const tab = (
                  <button
                    key={n}
                    role="tab"
                    aria-selected={pass === n}
                    className={`cuts-tab${pass === n ? " active" : ""}${sel.sub_layers[n - 1]?.cleanup ? " cleanup" : ""}`}
                    title={n === 0 ? t("layers.mainTabTitle") : p.has(C.OPT.layerSubMove) ? t("layers.subTabTitle") : undefined}
                    draggable={movable}
                    onClick={() => setPassTab((x) => ({ ...x, [sel.id]: n }))}
                    onDragStart={(e) => {
                      tabDrag.current = n;
                      e.dataTransfer.effectAllowed = "move";
                      e.dataTransfer.setData("text/plain", String(n));
                    }}
                    onDragOver={(e) => {
                      if (tabDrag.current !== null && n > 0) e.preventDefault();
                    }}
                    onDrop={(e) => {
                      e.preventDefault();
                      const from = tabDrag.current;
                      tabDrag.current = null;
                      if (from === null || n === 0 || from === n) return;
                      // Sub-layers burn left to right after the main settings; the dragged tab stays selected.
                      void run(C.layerSubMove(sel.id, from - 1, n - 1))
                        .then(() => setPassTab((x) => ({ ...x, [sel.id]: n })))
                        .catch(report);
                    }}
                    onDragEnd={() => (tabDrag.current = null)}
                  >
                    {passName(sel, n)}
                  </button>
                );
                if (n === 0) return tab;
                // A sub-layer tab with its grip (the same pointer-event reorder as the layer rows' grips).
                const k = n - 1;
                return (
                  <span
                    key={n}
                    role="presentation"
                    data-reorder-row=""
                    className={`cuts-tab-wrap${subReorder.dropClass(k)}${subReorder.dragging === k ? " dragging" : ""}`}
                  >
                    {tab}
                    {movable && (
                      <button
                        type="button"
                        className="row-grip tab-grip"
                        title={t("touch.reorder")}
                        aria-label={`${t("touch.reorder")}: ${passName(sel, n)}`}
                        onClick={(e) => e.stopPropagation()}
                        {...subReorder.grip(k)}
                      >
                        <Icon name="grip" size={14} />
                      </button>
                    )}
                  </span>
                );
              })}
            </div>
          )}
          <div
            className={`field-grid cuts-quick-grid${layerLink(sel) ? " lx-readonly" : ""}`}
            aria-disabled={layerLink(sel) ? true : undefined}
            // `inert` keeps a linked layer's read-only fields out of the Tab order too (React 18 has no prop for it).
            ref={(el) => {
              if (el) el.toggleAttribute("inert", !!layerLink(sel));
            }}
          >
            {passSettings && (
              <>
                <NumberField label={t("layers.speed")} unit="mm/s" value={passSettings.speed_mm_s} min={0.01} stepper onCommit={(v) => v !== null && patch(sel, { speed_mm_s: v })} />
                <NumberField label={t("layers.power")} unit="%" value={passSettings.power_pct} min={0} max={100} stepper onCommit={(v) => v !== null && patch(sel, { power_pct: v })} />
                <NumberField label={t("layers.passes")} value={passSettings.passes} min={1} max={1000} digits={0} stepper onCommit={(v) => v !== null && patch(sel, { passes: Math.round(v) })} />
                {/* Only the source settings the active device type takes: a GRBL or Ruida gantry has neither. */}
                {fields.frequency && (
                  <NumberField
                    label={t("layers.frequency")}
                    unit="kHz"
                    value={passSettings.frequency_khz}
                    min={freq.min}
                    max={freq.max}
                    stepper
                    onCommit={(v) => v !== null && patch(sel, { frequency_khz: v })}
                  />
                )}
                {/* LightBurn's Q-Pulse (MOPA): empty keeps the source's current pulse width. */}
                {fields.pulseWidth && (
                <NumberField
                  label={t("layers.qpulse")}
                  unit="ns"
                  value={passSettings.pulse_width_ns}
                  min={1}
                  digits={0}
                  allowEmpty
                  placeholder={t("layers.qpulseDefault")}
                  onCommit={(v) => patch(sel, { pulse_width_ns: v === null ? null : Math.round(v) })}
                />
                )}
                {passSettings.mode !== "line" && (
                  <NumberField
                    label={t("layers.interval")}
                    unit="mm"
                    value={passSettings.hatch.interval_mm}
                    min={0.001}
                    digits={4}
                    onCommit={(v) => v !== null && patch(sel, { hatch: { interval_mm: v } })}
                  />
                )}
              </>
            )}
          </div>
          <p className="hint">
            {passSettings && t("layers.speedHint", { speed: formatSpeed(passSettings.speed_mm_s, prefs.speedUnit) })}
            {subCount > 0 && passSettings && ` ${t("layers.passHint", { mode: modeText(passSettings.mode) })}`}
          </p>
        </div>
      )}

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}
      {editLayer && (
        <WsModal help="layers-and-settings#cut-settings" title={t("layers.settingsTitle", { name: editLayer.name })} wide onClose={() => setEditing(null)}>
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
      help="layers-and-settings#delete-a-layer"
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
