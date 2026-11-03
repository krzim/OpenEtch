// The bracket test (short list #7): one click makes a small grid around a library preset (±10 % power x speed in
// 5 x 5 boxes by default, configurable), in a new document, with the material test generator. After it is burned,
// the canvas HUD bar asks for the best box; "Use this box" writes its settings back to the preset and opens the
// photo flow; a second box in the same row or column, one hue away, refines the device's hue step model.

import { Fragment, useEffect, useMemo, useState } from "react";
import type { GDocument } from "../api/types.ts";
import { CheckField, Icon, NumberField } from "../components/ui.tsx";
import { HudItem } from "../hud/CanvasHud.tsx";
import { treeEntries } from "../libs/model.ts";
import { useLibrary } from "../libs/store.ts";
import { errText } from "../libs/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { materialsApi } from "./api.ts";
import { bracketTests, parseHueModel, pctText, sameLine, selectedCell, type BracketCell } from "./model.ts";
import { hueModel, openMaterials } from "./store.ts";
import "./materials.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

const PARAMS = ["power", "speed", "frequency", "pulse_width", "interval", "passes"] as const;
type Param = (typeof PARAMS)[number];

interface Axis {
  param: Param;
  spread: number;
  count: number;
}

export function BracketDialog(p: { library: string; entryId: string; notify: Notify; report: (e: unknown) => void; onClose: () => void; onCreated?: () => void }) {
  const t = useT();
  const lib = useLibrary();
  const entry = treeEntries(lib.view?.tree ?? []).find((e) => e.id === p.entryId) ?? null;
  const [cols, setCols] = useState<Axis>({ param: "power", spread: 10, count: 5 });
  const [rows, setRows] = useState<Axis>({ param: "speed", spread: 10, count: 5 });
  const [cell, setCell] = useState(5);
  const [newDoc, setNewDoc] = useState(true);
  const [busy, setBusy] = useState(false);
  if (!entry) return null;
  const noPulse = entry.summary?.pulse_width_ns == null;
  const same = cols.param === rows.param;
  const bad = same || (noPulse && (cols.param === "pulse_width" || rows.param === "pulse_width"));
  const create = async () => {
    if (bad) return;
    setBusy(true);
    try {
      const r = (await materialsApi.bracket({
        library: p.library,
        entry: entry.id,
        columns: { param: cols.param, spread_pct: cols.spread, count: cols.count },
        rows: { param: rows.param, spread_pct: rows.spread, count: rows.count },
        cell_mm: cell,
        new_document: newDoc,
      })) as { result?: { boxes?: number } };
      p.notify(t("mat.bracketCreated", { path: entry.path, boxes: r.result?.boxes ?? 0 }));
      p.onCreated?.();
      p.onClose();
    } catch (e) {
      p.report(new Error(errText(e)));
      setBusy(false);
    }
  };
  const axis = (label: string, a: Axis, set: (a: Axis) => void) => (
    <fieldset className="mt-axis">
      <legend>{label}</legend>
      <label className="field">
        <span className="field-label">{t("mat.via")}</span>
        <span className="field-input">
          <select value={a.param} onChange={(e) => set({ ...a, param: e.target.value as Param })}>
            {PARAMS.map((x) => (
              <option key={x} value={x} disabled={x === "pulse_width" && noPulse}>
                {t(`mat.param.${x}`)}
              </option>
            ))}
          </select>
        </span>
      </label>
      <NumberField label={t("mat.bracketSpread")} unit="%" value={a.spread} min={0.1} max={50} digits={1} stepper onCommit={(v) => v !== null && set({ ...a, spread: v })} />
      <NumberField label={t("mat.bracketCount")} value={a.count} min={1} max={15} digits={0} stepper onCommit={(v) => v !== null && set({ ...a, count: Math.max(1, Math.min(15, Math.round(v))) })} />
    </fieldset>
  );
  return (
    <WsModal
      title={t("mat.bracketTitle", { path: entry.path })}
      onClose={p.onClose}
      onSubmit={() => void create()}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || bad} onClick={() => void create()}>
            {t("mat.bracketCreate")}
          </button>
        </>
      }
    >
      <div className="mt-axes">
        {axis(t("mat.bracketColumns"), cols, setCols)}
        {axis(t("mat.bracketRows"), rows, setRows)}
      </div>
      <NumberField label={t("mat.bracketCell")} unit="mm" value={cell} min={1} max={30} digits={1} stepper onCommit={(v) => v !== null && setCell(v)} />
      <CheckField label={t("mat.bracketNewDoc")} checked={newDoc} onChange={setNewDoc} />
      <p className="hint">{t("mat.bracketHint")}</p>
    </WsModal>
  );
}

/**
 * The canvas HUD bar of a bracket test in the active document (top centre). Boxes are grouped (they move as one),
 * so the best box is tapped in the bar's own grid, laid out as on the canvas (rows top to bottom, columns left to
 * right; the dot is the preset itself); a single box selected on the canvas picks it too. "Use this box" writes it
 * back to the preset and opens the photo flow; a second box in the same row or column, one hue away, refines the
 * device's hue step model.
 */
export function BracketHud(p: { doc: GDocument | null; selection: readonly string[]; design: boolean; notify: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const lib = useLibrary();
  const tests = useMemo(() => bracketTests(p.doc), [p.doc]);
  const [hidden, setHidden] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [chosen, setChosen] = useState<{ group: string; layer: string } | null>(null);
  const [best, setBest] = useState<{ group: string; cell: BracketCell } | null>(null);
  const [busy, setBusy] = useState(false);
  const canvasSel = selectedCell(tests, p.selection);
  const selLayer = canvasSel?.cell.layer ?? null;
  useEffect(() => {
    if (!canvasSel) return;
    setChosen({ group: canvasSel.test.group, layer: canvasSel.cell.layer });
    setOpen(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selLayer]);
  const test = canvasSel?.test ?? tests.find((x) => x.group !== hidden) ?? null;
  if (!p.design || !test || test.group === hidden) return null;
  const pname = (k: string) => (PARAMS.includes(k as Param) ? t(`mat.param.${k as Param}`) : k);
  const cellText = (c: BracketCell) => t("mat.hudCell", { rows: pname(test.rows), r: pctText(c.rowPct), columns: pname(test.columns), c: pctText(c.colPct) });
  const bestHere = best && best.group === test.group ? best.cell : null;
  const c = chosen && chosen.group === test.group ? (test.cells.find((x) => x.layer === chosen.layer) ?? null) : null;
  const rows = [...new Set(test.cells.map((x) => x.rowPct))].sort((a, b) => a - b);
  const cols = [...new Set(test.cells.map((x) => x.colPct))].sort((a, b) => a - b);
  const at = new Map(test.cells.map((x) => [`${x.rowPct},${x.colPct}`, x]));

  const use = async (cell: BracketCell) => {
    setBusy(true);
    try {
      const r = (await materialsApi.pick(cell.layer)) as { library?: string; id?: string; path?: string };
      await lib.refresh();
      setBest({ group: test.group, cell });
      p.notify(t("mat.hudPicked", { path: test.path, cell: cellText(cell) }));
      // The photo flow: the entry's finish dialog with the camera.
      const id = r.id ?? treeEntries(lib.view?.tree ?? []).find((e) => e.path === test.path)?.id;
      if (r.library && id) openMaterials({ kind: "finish", library: r.library, id, photo: true });
    } catch (e) {
      p.report(new Error(errText(e)));
    } finally {
      setBusy(false);
    }
  };
  const calibrate = async (cell: BracketCell, hues: number) => {
    if (!bestHere) return;
    setBusy(true);
    try {
      const r = (await materialsApi.calibrate(bestHere.layer, cell.layer, hues)) as { model?: unknown; calibrated?: unknown };
      const m = parseHueModel(r.model);
      hueModel.set({ model: m, calibrated: r.calibrated === true, loaded: true });
      // The setting that differs between the two boxes is the one calibrated.
      const param = cell.rowPct !== bestHere.rowPct ? test.rows : test.columns;
      const step = param === "power" ? `${m.power_pct} %` : param === "speed" ? `${m.speed_mm_s} mm/s` : `${m.frequency_khz} kHz`;
      p.notify(t("mat.hudCalibrated", { step: `${pname(param)} ${step}` }));
    } catch (e) {
      p.report(new Error(errText(e)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <HudItem id="bracket-bar" slot="tc" rank={22} minW={220} maxW={560}>
      <div className="vp-overlay mt-hud" role="group" aria-label={t("mat.bracketTitle", { path: test.path })}>
        <div className="mt-hud-row mt-hud-head">
          <span className="mt-hud-title">{c ? cellText(c) : t("mat.hudSelect", { path: test.path })}</span>
          <button type="button" className="btn small" aria-expanded={open} onClick={() => setOpen(!open)}>
            <Icon name="grid" size={14} /> {t("mat.hudBoxes")}
          </button>
          <button type="button" className="icon-btn small" aria-label={t("mat.hudClose")} title={t("mat.hudClose")} onClick={() => setHidden(test.group)}>
            <Icon name="close" size={14} />
          </button>
        </div>
        {open && (
          <div className="mt-hud-grid" style={{ gridTemplateColumns: `auto repeat(${cols.length}, var(--mt-box))` }}>
            <span className="mt-hud-axis">
              {pname(test.rows)} ↓ {pname(test.columns)} →
            </span>
            {cols.map((x) => (
              <span key={`c${x}`} className="mt-hud-axis">
                {pctText(x)}
              </span>
            ))}
            {rows.map((r) => (
              <Fragment key={`r${r}`}>
                <span className="mt-hud-axis">{pctText(r)}</span>
                {cols.map((x) => {
                  const cell = at.get(`${r},${x}`);
                  if (!cell) return <span key={`${r},${x}`} />;
                  const cls = ["mt-hud-box", c?.layer === cell.layer ? "on" : "", bestHere?.layer === cell.layer ? "best" : "", r === 0 && x === 0 ? "preset" : ""].filter(Boolean).join(" ");
                  return (
                    <button key={cell.layer} type="button" className={cls} aria-pressed={c?.layer === cell.layer} aria-label={cellText(cell)} title={cellText(cell)} onClick={() => setChosen({ group: test.group, layer: cell.layer })} />
                  );
                })}
              </Fragment>
            ))}
          </div>
        )}
        {c && (
          <div className="mt-hud-row">
            <button type="button" className="btn small btn-primary" disabled={busy} title={t("mat.hudUseTitle", { path: test.path })} onClick={() => void use(c)}>
              <Icon name="camera" size={14} /> {t("mat.hudUse")}
            </button>
            {bestHere && c.layer !== bestHere.layer && sameLine(bestHere, c) && (
              <span className="mt-hud-cal">
                <span>{t("mat.hudCalibrate")}</span>
                <button type="button" className="btn small" disabled={busy} onClick={() => void calibrate(c, 1)}>
                  {t("mat.hudWarmer")}
                </button>
                <button type="button" className="btn small" disabled={busy} onClick={() => void calibrate(c, -1)}>
                  {t("mat.hudCooler")}
                </button>
              </span>
            )}
            {bestHere && c.layer !== bestHere.layer && !sameLine(bestHere, c) && <span className="hint">{t("mat.hudSameLine")}</span>}
          </div>
        )}
      </div>
    </HudItem>
  );
}
