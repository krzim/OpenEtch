// Dialogs of the design-engine UI: Move Selected Objects (nine page positions on the workspace or the stock, LightBurn's
// Move window), Move Horizontally / Vertically Together (fixed spacing), Grid Array (LightBurn's options with a live
// preview), Curve Tolerance (the document's output setting) and a guide's numeric position.

import { useMemo, useState } from "react";
import type { Invocation } from "../api/client.ts";
import { isSquare, workspaceText } from "../api/devices.ts";
import * as G from "../api/geomCommands.ts";
import type { GDocument, Vec2, VirtualArray } from "../api/types.ts";
import { evalNumber } from "../editing.ts";
import { geometryCmds, objectBounds, toPathData, unionBoxes, type Box } from "../geometry.ts";
import { Modal, UnitTextField } from "../components/ui.tsx";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import type { MsgKey } from "../workspace/i18n.ts";
import { formatLength } from "../workspace/units.ts";
import { dBool, dNum, dStr } from "../workspace/dialogDefaults.ts";
import { DCMD, curveTolerance, distributeSpacing, pagePosition } from "./designApi.ts";
import {
  PAGE_ANCHORS,
  arrayCells,
  arrayOptionsOf,
  arrayParams,
  arrayProblem,
  cellsOutside,
  editArrayParams,
  resolveCounts,
  type ArrayOptions,
  type PageAnchor,
} from "./model.ts";
import { errorText } from "../api/errorText.ts";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
const Num = UnitTextField;
const num = (v: string) => evalNumber(v);

function useBusy(run: Run, onDone?: (r: unknown) => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const go = async (inv: Invocation | Invocation[] | string): Promise<boolean> => {
    if (busy) return false;
    if (typeof inv === "string") {
      setError(inv);
      return false;
    }
    setBusy(true);
    setError(null);
    try {
      const r = await run(inv);
      onDone?.(r);
      return true;
    } catch (e) {
      setError(errorText(e));
      return false;
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, go };
}

/** Message key of a page position's name (locales/en-design.ts). */
const anchorKey = (a: PageAnchor) => `dz.anchor.${a}` as MsgKey;

/** LightBurn's Move Selected Objects window: one click sends the selection to a corner, an edge centre or the centre. */
export function MovePositionDialog(p: { doc: GDocument; ids: string[]; run: Run; onClose: () => void }) {
  const t = useT();
  const [margin, setMargin] = useState("0");
  const [ref, setRef] = useState<"workspace" | "stock">("workspace");
  const { busy, error, go } = useBusy(p.run);
  const lu = usePrefs().units;
  const move = (a: PageAnchor) => {
    const m = num(margin);
    if (m === null || m < 0) return void go(t("dz.marginMin", { min: formatLength(0, lu) }));
    if (!p.ids.length) return void go(t("dz.selectToMove"));
    void go(pagePosition(p.ids, a, m, ref));
  };
  return (
    <Modal
      help="drawing-and-editing#move-to-a-page-position"
      title={t("dz.moveTitle")}
      onClose={p.onClose}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn btn-primary" onClick={p.onClose}>
            {t("dz.done")}
          </button>
        </>
      }
    >
      <div className="dz-move-grid" role="group" aria-label={t("dz.pagePosition")}>
        {PAGE_ANCHORS.map((a) => (
          <button key={a} className="btn" disabled={busy || !p.ids.length} title={t(ref === "stock" ? "dz.moveToStock" : "dz.moveToWorkspace", { anchor: t(anchorKey(a)).toLowerCase() })} onClick={() => move(a)}>
            <span className={`dz-anchor dz-anchor-${a}`} aria-hidden="true" />
            <span className="sr-only">{t(anchorKey(a))}</span>
          </button>
        ))}
      </div>
      <label className="field">
        <span className="field-label">{t("dz.relativeTo")}</span>
        <span className="field-input">
          <select value={ref} onChange={(e) => setRef(e.target.value as "workspace" | "stock")}>
            <option value="workspace">{t("dz.relWorkspace")}</option>
            <option value="stock" disabled={!p.doc.stock}>
              {p.doc.stock ? t("dz.relStock") : t("dz.relStockNone")}
            </option>
          </select>
        </span>
      </label>
      <Num label={t("dz.margin")} unit="mm" value={margin} onChange={setMargin} title={t("dz.marginTitle")} />
      <p className="hint">{t("dz.moveHint")}</p>
    </Modal>
  );
}

/** Move Horizontally / Vertically Together: the selection packed side by side with a fixed gap. */
export function MoveTogetherDialog(p: { ids: string[]; axis: "x" | "y"; run: Run; onClose: () => void }) {
  const t = useT();
  const [gap, setGap] = useState("1");
  const [axis, setAxis] = useState<"x" | "y">(p.axis);
  const { busy, error, go } = useBusy(p.run);
  const ok = async () => {
    const g = num(gap);
    if (g === null || !Number.isFinite(g)) return void go(t("dz.spacingNumber"));
    if (p.ids.length < 2) return void go(t("dz.selectTwo"));
    if (await go(distributeSpacing(p.ids, axis, g))) p.onClose();
  };
  return (
    <Modal
      help="drawing-and-editing#move-together"
      title={axis === "x" ? t("dz.moveHTogether") : t("dz.moveVTogether")}
      onClose={p.onClose}
      onSubmit={() => void ok()}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("dz.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void ok()}>
            {t("dz.move")}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{t("dz.direction")}</span>
        <span className="field-input">
          <select value={axis} onChange={(e) => setAxis(e.target.value as "x" | "y")}>
            <option value="x">{t("dz.dirX")}</option>
            <option value="y">{t("dz.dirY")}</option>
          </select>
        </span>
      </label>
      <Num label={t("dz.spacing")} unit="mm" value={gap} onChange={setGap} title={t("dz.spacingTitle")} />
      <p className="hint">{t("dz.togetherHint")}</p>
    </Modal>
  );
}

/** The document's output curve tolerance (LightBurn's Curve Tolerance), saved with the project. */
export function CurveToleranceDialog(p: { doc: GDocument; run: Run; onClose: () => void }) {
  const tr = useT();
  const cur = typeof p.doc.curve_tolerance_mm === "number" ? p.doc.curve_tolerance_mm : null;
  const [v, setV] = useState(cur === null ? "" : String(cur));
  const { busy, error, go } = useBusy(p.run);
  const lu = usePrefs().units;
  const ok = async () => {
    const t = v.trim() === "" ? null : num(v);
    if (v.trim() !== "" && (t === null || !(t >= 0.001 && t <= 1))) return void go(tr("dz.tolRange", { min: formatLength(0.001, lu, 3), max: formatLength(1, lu, 3) }));
    if (t === cur) return p.onClose();
    if (await go(curveTolerance(t))) p.onClose();
  };
  return (
    <Modal
      help="drawing-and-editing#curve-tolerance"
      title={tr("dz.curveTolerance")}
      onClose={p.onClose}
      onSubmit={() => void ok()}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            {tr("dz.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void ok()}>
            {tr("dz.save")}
          </button>
        </>
      }
    >
      <Num label={tr("dz.curveTolerance")} unit="mm" value={v} onChange={setV} title={tr("dz.tolTitle")} />
      <p className="hint">{tr("dz.tolHint", { dflt: formatLength(0.01, lu, 3) })}</p>
    </Modal>
  );
}

/** A guide's position typed in (LightBurn positions guides numerically), or a new guide. */
export function GuideDialog(p: {
  guide: { axis: "x" | "y"; v: number } | null;
  onClose: () => void;
  onOk: (axis: "x" | "y", v: number) => void;
  onDelete?: () => void;
}) {
  const t = useT();
  const [axis, setAxis] = useState<"x" | "y">(p.guide?.axis ?? "x");
  const [v, setV] = useState(String(p.guide?.v ?? 0));
  const [err, setErr] = useState<string | null>(null);
  const ok = () => {
    const x = num(v);
    if (x === null || Math.abs(x) > 1e6) return setErr(t("dz.guidePosition"));
    p.onOk(axis, Math.round(x * 1e4) / 1e4);
  };
  return (
    <Modal
      help="drawing-and-editing#guides"
      title={p.guide ? t("dz.guide") : t("dz.addGuide")}
      onClose={p.onClose}
      onSubmit={ok}
      footer={
        <>
          {err && <span className="form-error">{err}</span>}
          {p.onDelete && (
            <button className="btn btn-danger" onClick={p.onDelete}>
              {t("dz.delete")}
            </button>
          )}
          <button className="btn" onClick={p.onClose}>
            {t("dz.cancel")}
          </button>
          <button className="btn btn-primary" onClick={ok}>
            {t("dz.ok")}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{t("dz.guide")}</span>
        <span className="field-input">
          <select value={axis} onChange={(e) => setAxis(e.target.value as "x" | "y")}>
            <option value="x">{t("dz.guideV")}</option>
            <option value="y">{t("dz.guideH")}</option>
          </select>
        </span>
      </label>
      <Num label={axis === "x" ? "X" : "Y"} unit="mm" value={v} onChange={setV} />
      <p className="hint">{t("dz.guideHint")}</p>
    </Modal>
  );
}

// ---- grid array -----------------------------------------------------------------------------------------------------

const MAX_PREVIEW_CELLS = 400;

/**
 * LightBurn's Grid Array: columns, rows (or Total Width / Height), spacing (centre to centre or gap), direction,
 * alternate shifts and mirrors, Random Orientation, Auto-Increment Variable Text, Group / Select Results and Create
 * Virtual Array; with `editArray`, LightBurn's Edit Array for that virtual array (its settings, laid out again).
 */
export function GridArrayDialog(p: {
  doc: GDocument;
  ids: string[];
  workspaceMm: Vec2;
  has: (cmd: string) => boolean;
  run: Run;
  onClose: () => void;
  onCreated?: (ids: string[]) => void;
  editArray?: VirtualArray | null;
}) {
  const t = useT();
  const init = useMemo(() => (p.editArray ? arrayOptionsOf(p.editArray.settings) : null), [p.editArray]);
  const ids = p.editArray ? p.editArray.sources : p.ids;
  const objs = useMemo(() => {
    const want = new Set(ids);
    return p.doc.objects.filter((o) => want.has(o.id) || (o.group && want.has(o.group)));
  }, [p.doc, ids]);
  const box = useMemo(() => unionBoxes(objs.map(objectBounds)), [objs]);
  const [bw, bh] = box ? [box.x1 - box.x0, box.y1 - box.y0] : [10, 10];
  const [cols, setCols] = useState(String(init?.cols ?? 3));
  const [rows, setRows] = useState(String(init?.rows ?? 2));
  // A new array starts from the last LightBurn settings imported (GridArray_* keys, workspace/dialogDefaults.ts).
  const [gapMode, setGapMode] = useState(init ? init.gapX : dStr("grid_array", "spacing_x", "gap") === "gap");
  const [dx, setDx] = useState(String(init?.pitchX ?? dNum("grid_array", "pitch_x_mm", 2)));
  const [dy, setDy] = useState(String(init?.pitchY ?? dNum("grid_array", "pitch_y_mm", 2)));
  const [revX, setRevX] = useState(init?.reverseX ?? dBool("grid_array", "reverse_x", false));
  const [revY, setRevY] = useState(init?.reverseY ?? dBool("grid_array", "reverse_y", false));
  const [rowShift, setRowShift] = useState(typeof init?.rowShift === "number" ? String(init.rowShift) : "");
  const [rowHalf, setRowHalf] = useState(init ? init.rowShift === "half" : dStr("grid_array", "row_shift", "") === "half");
  const [colShift, setColShift] = useState(typeof init?.colShift === "number" ? String(init.colShift) : "");
  const [colHalf, setColHalf] = useState(init ? init.colShift === "half" : dStr("grid_array", "col_shift", "") === "half");
  const [mirrorCols, setMirrorCols] = useState<"" | "x" | "y">(init?.mirrorCols ?? "");
  const [mirrorRows, setMirrorRows] = useState<"" | "x" | "y">(init?.mirrorRows ?? "");
  const [totalW, setTotalW] = useState("");
  const [totalH, setTotalH] = useState("");
  const [random, setRandom] = useState(init ? init.seed != null : dBool("grid_array", "random_orientation", false));
  const [seed, setSeed] = useState(String(init?.seed ?? dNum("grid_array", "random_seed", Math.floor(Math.random() * 1e6))));
  const [autoInc, setAutoInc] = useState(String(init?.autoIncrement ?? 0));
  const [groupResults, setGroupResults] = useState(dBool("grid_array", "group_results", false));
  const [selectResults, setSelectResults] = useState(dBool("grid_array", "select_results", true));
  const [virtualArr, setVirtualArr] = useState(!!p.editArray || dBool("grid_array", "virtual", false));
  const editing = !!p.editArray;
  const newOptions = p.has(DCMD.array);
  const { busy, error, go } = useBusy(p.run, (r) => p.onCreated?.(selectResults ? G.resultIds(r).filter((id) => !ids.includes(id)) : []));
  const lu = usePrefs().units;
  const shift = (txt: string, half: boolean): number | "half" | null => (half ? "half" : txt.trim() === "" ? null : num(txt));
  const opts: ArrayOptions | null = (() => {
    const c = num(cols);
    const r = num(rows);
    const x = num(dx);
    const y = num(dy);
    if (c === null || r === null || x === null || y === null) return null;
    const rs = shift(rowShift, rowHalf);
    const cs = shift(colShift, colHalf);
    if ((rowShift.trim() && rs === null) || (colShift.trim() && cs === null)) return null;
    const tw = totalW.trim() === "" ? null : num(totalW);
    const th = totalH.trim() === "" ? null : num(totalH);
    const sd = random ? num(seed) : null;
    const inc = num(autoInc);
    if ((totalW.trim() && tw === null) || (totalH.trim() && th === null) || (random && sd === null) || inc === null) return null;
    return {
      totalWidth: tw,
      totalHeight: th,
      seed: sd === null ? null : Math.max(0, Math.round(sd)),
      autoIncrement: Math.round(inc),
      groupResults: !virtualArr && groupResults,
      virtual: virtualArr,
      cols: c,
      rows: r,
      pitchX: x,
      pitchY: y,
      gapX: gapMode,
      gapY: gapMode,
      reverseX: revX,
      reverseY: revY,
      rowShift: rs,
      colShift: cs,
      mirrorCols: mirrorCols || null,
      mirrorRows: mirrorRows || null,
    };
  })();
  // Total Width / Height resolve the counts the way the engine does, for the checks and the preview.
  const counted = opts && box ? resolveCounts(opts, box) : opts;
  const problem = !opts ? t("dz.enterCounts") : !counted ? t("dz.totalTooSmall") : arrayProblem(counted, box);
  const outside = counted && box && !problem ? cellsOutside(counted, box, p.workspaceMm) : 0;
  const create = async () => {
    if (!opts || problem || !box) return void go(problem ?? t("dz.enterNumbers"));
    const params = p.editArray ? editArrayParams(p.editArray.id, opts) : arrayParams(p.ids, opts);
    if (await go({ id: DCMD.array, params })) p.onClose();
  };
  const check = (label: string, v: boolean, set: (v: boolean) => void, title?: string) => (
    <label className="field field-check" title={title}>
      <span className="field-label">{label}</span>
      <input type="checkbox" checked={v} onChange={(e) => set(e.target.checked)} />
    </label>
  );
  const mirrorSel = (label: string, v: "" | "x" | "y", set: (v: "" | "x" | "y") => void) => (
    <label className="field">
      <span className="field-label">{label}</span>
      <span className="field-input">
        <select value={v} onChange={(e) => set(e.target.value as "" | "x" | "y")}>
          <option value="">{t("dz.mirrorNo")}</option>
          <option value="x">{t("dz.mirrorLR")}</option>
          <option value="y">{t("dz.mirrorTB")}</option>
        </select>
      </span>
    </label>
  );
  return (
    <Modal
      help="drawing-and-editing#grid-array"
      title={editing ? t("dz.editArray") : t("dz.gridArray")}
      wide
      onClose={p.onClose}
      onSubmit={() => void create()}
      footer={
        <>
          {(error ?? (opts ? null : problem)) && <span className="form-error">{error ?? problem}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("dz.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || !!problem || !newOptions} onClick={() => void create()}>
            {editing ? t("dz.apply2") : t("dz.create")}
          </button>
        </>
      }
    >
      <div className="dz-array">
        <div className="dz-array-form">
          <div className="field-grid">
            <Num label={t("dz.columns")} value={totalW.trim() && counted ? String(counted.cols) : cols} onChange={setCols} />
            <Num label={t("dz.rows")} value={totalH.trim() && counted ? String(counted.rows) : rows} onChange={setRows} />
          </div>
          <div className="field-grid">
            <Num label={t("dz.totalWidth")} unit="mm" value={totalW} onChange={setTotalW} title={t("dz.totalWidthTitle")} />
            <Num label={t("dz.totalHeight")} unit="mm" value={totalH} onChange={setTotalH} title={t("dz.totalHeightTitle")} />
          </div>
          <label className="field">
            <span className="field-label">{t("dz.spacing")}</span>
            <span className="field-input">
              <select
                value={gapMode ? "gap" : "center"}
                onChange={(e) => {
                  const g = e.target.value === "gap";
                  // Keep the same layout when switching how the spacing is measured.
                  const x = num(dx);
                  const y = num(dy);
                  if (x !== null) setDx(String(Number((g ? x - bw : x + bw).toFixed(3))));
                  if (y !== null) setDy(String(Number((g ? y - bh : y + bh).toFixed(3))));
                  setGapMode(g);
                }}
              >
                <option value="gap">{t("dz.spacingGap")}</option>
                <option value="center">{t("dz.spacingCentre")}</option>
              </select>
            </span>
          </label>
          <div className="field-grid">
            <Num label={gapMode ? t("dz.gapX") : t("dz.pitchX")} unit="mm" value={dx} onChange={setDx} />
            <Num label={gapMode ? t("dz.gapY") : t("dz.pitchY")} unit="mm" value={dy} onChange={setDy} />
          </div>
          <div className="field-grid">
            {check(t("dz.reverseX"), revX, setRevX, t("dz.reverseXTitle"))}
            {check(t("dz.reverseY"), revY, setRevY, t("dz.reverseYTitle"))}
          </div>
          <h4 className="et-dlg-h">{t("dz.alternate")}</h4>
          <div className="field-grid">
            <Num label={t("dz.rowShift")} unit="mm" value={rowHalf ? "" : rowShift} onChange={setRowShift} title={t("dz.rowShiftTitle")} />
            {check(t("dz.halfWidth"), rowHalf, setRowHalf, t("dz.halfWidthTitle"))}
            <Num label={t("dz.colShift")} unit="mm" value={colHalf ? "" : colShift} onChange={setColShift} title={t("dz.colShiftTitle")} />
            {check(t("dz.halfHeight"), colHalf, setColHalf, t("dz.halfHeightTitle"))}
          </div>
          <div className="field-grid">
            {mirrorSel(t("dz.mirrorCols"), mirrorCols, setMirrorCols)}
            {mirrorSel(t("dz.mirrorRows"), mirrorRows, setMirrorRows)}
          </div>
          <h4 className="et-dlg-h">{t("dz.results")}</h4>
          <div className="field-grid">
            {check(t("dz.randomOrientation"), random, setRandom, t("dz.randomOrientationTitle"))}
            {random && (
              <span className="dz-seed">
                <Num label={t("dz.seed")} value={seed} onChange={setSeed} />
                <button className="btn btn-small" onClick={() => setSeed(String(Math.floor(Math.random() * 1e6)))}>
                  {t("dz.newSeed")}
                </button>
              </span>
            )}
          </div>
          <Num label={t("dz.autoIncrement")} value={autoInc} onChange={setAutoInc} title={t("dz.autoIncrementTitle")} />
          {!editing && (
            <div className="field-grid">
              {check(t("dz.virtualArray"), virtualArr, setVirtualArr, t("dz.virtualArrayTitle"))}
              {!virtualArr && check(t("dz.groupResults"), groupResults, setGroupResults, t("dz.groupResultsTitle"))}
              {check(t("dz.selectResults"), selectResults, setSelectResults, t("dz.selectResultsTitle"))}
            </div>
          )}
          {(virtualArr || editing) && <p className="hint">{t("dz.virtualHint")}</p>}
        </div>
        <ArrayPreview objs={objs} box={box} opts={problem ? null : counted} workspaceMm={p.workspaceMm} />
      </div>
      {outside > 0 && opts && (
        <p className="warn-text" role="status">
          {t(isSquare(p.workspaceMm) ? "dz.cellsOutsideField" : "dz.cellsOutsideWorkspace", { n: outside, total: (counted ?? opts).cols * (counted ?? opts).rows, size: workspaceText(p.workspaceMm) })}
        </p>
      )}
      {!newOptions && <p className="warn-text">{t("dz.noArray")}</p>}
      {random && <p className="hint">{t("dz.randomPreview")}</p>}
      <p className="hint">{t("dz.arrayHint", { w: formatLength(bw, lu), h: formatLength(bh, lu) })}</p>
    </Modal>
  );
}

function ArrayPreview(p: { objs: GDocument["objects"]; box: Box | null; opts: ArrayOptions | null; workspaceMm: Vec2 }) {
  const tr = useT();
  const shape = useMemo(() => p.objs.map((o) => toPathData(geometryCmds(o.geometry), o.transform)).join(" "), [p.objs]);
  if (!p.box || !p.opts) return <div className="dz-array-preview empty">{tr("dz.noPreview")}</div>;
  const b = p.box;
  const cells = arrayCells(p.opts, b);
  const shown = cells.slice(0, MAX_PREVIEW_CELLS);
  const all = unionBoxes([
    ...shown.map((c) => ({ x0: b.x0 + c.d[0], y0: b.y0 + c.d[1], x1: b.x1 + c.d[0], y1: b.y1 + c.d[1] })),
    { x0: -p.workspaceMm[0] / 2, y0: -p.workspaceMm[1] / 2, x1: p.workspaceMm[0] / 2, y1: p.workspaceMm[1] / 2 },
  ]) as Box;
  const pad = Math.max(all.x1 - all.x0, all.y1 - all.y0) * 0.03;
  const vb = `${all.x0 - pad} ${-all.y1 - pad} ${all.x1 - all.x0 + 2 * pad} ${all.y1 - all.y0 + 2 * pad}`;
  const [cx, cy] = [(b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2];
  return (
    <div className="dz-array-preview">
      <svg viewBox={vb} preserveAspectRatio="xMidYMid meet" role="img" aria-label={tr("dz.arrayPreview")}>
        <g transform="scale(1 -1)">
          <rect className="dz-ap-field" x={-p.workspaceMm[0] / 2} y={-p.workspaceMm[1] / 2} width={p.workspaceMm[0]} height={p.workspaceMm[1]} vectorEffect="non-scaling-stroke" />
          {shown.map((c) => {
            const t = `translate(${c.d[0] + cx} ${c.d[1] + cy}) scale(${c.flipX ? -1 : 1} ${c.flipY ? -1 : 1}) translate(${-cx} ${-cy})`;
            return <path key={`${c.row}-${c.col}`} className={c.row === 0 && c.col === 0 ? "dz-ap-orig" : "dz-ap-copy"} d={shape} transform={t} vectorEffect="non-scaling-stroke" />;
          })}
        </g>
      </svg>
      {cells.length > shown.length && <span className="hint">{tr("dz.previewCap", { shown: MAX_PREVIEW_CELLS, total: cells.length })}</span>}
    </div>
  );
}
