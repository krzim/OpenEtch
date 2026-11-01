// Dialogs of the design-engine UI: Move Selected Objects (nine page positions on the workspace or the stock, LightBurn's
// Move window), Move Horizontally / Vertically Together (fixed spacing), Grid Array (LightBurn's options with a live
// preview), Curve Tolerance (the document's output setting) and a guide's numeric position.

import { useMemo, useState } from "react";
import type { Invocation } from "../api/client.ts";
import { isSquare, workspaceText } from "../api/devices.ts";
import * as G from "../api/geomCommands.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import { evalNumber } from "../editing.ts";
import { geometryCmds, objectBounds, toPathData, unionBoxes, type Box } from "../geometry.ts";
import { Modal, UnitTextField } from "../components/ui.tsx";
import { usePrefs } from "../workspace/usePrefs.ts";
import { formatLength } from "../workspace/units.ts";
import { DCMD, curveTolerance, distributeSpacing, pagePosition } from "./designApi.ts";
import { PAGE_ANCHORS, arrayCells, arrayParams, arrayProblem, cellsOutside, type ArrayOptions, type PageAnchor } from "./model.ts";

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
      setError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, go };
}

const ANCHOR_LABEL: Record<PageAnchor, string> = {
  top_left: "Top left",
  top: "Top centre",
  top_right: "Top right",
  left: "Middle left",
  center: "Centre",
  right: "Middle right",
  bottom_left: "Bottom left",
  bottom: "Bottom centre",
  bottom_right: "Bottom right",
};

/** LightBurn's Move Selected Objects window: one click sends the selection to a corner, an edge centre or the centre. */
export function MovePositionDialog(p: { doc: GDocument; ids: string[]; run: Run; onClose: () => void }) {
  const [margin, setMargin] = useState("0");
  const [ref, setRef] = useState<"workspace" | "stock">("workspace");
  const { busy, error, go } = useBusy(p.run);
  const lu = usePrefs().units;
  const move = (a: PageAnchor) => {
    const m = num(margin);
    if (m === null || m < 0) return void go(`The margin must be ${formatLength(0, lu)} or more.`);
    if (!p.ids.length) return void go("Select something to move.");
    void go(pagePosition(p.ids, a, m, ref));
  };
  return (
    <Modal
      title="Move selected objects"
      onClose={p.onClose}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn btn-primary" onClick={p.onClose}>
            Done
          </button>
        </>
      }
    >
      <div className="dz-move-grid" role="group" aria-label="Page position">
        {PAGE_ANCHORS.map((a) => (
          <button key={a} className="btn" disabled={busy || !p.ids.length} title={`Move the selection to the ${ANCHOR_LABEL[a].toLowerCase()} of the ${ref === "stock" ? "stock" : "workspace"}`} onClick={() => move(a)}>
            <span className={`dz-anchor dz-anchor-${a}`} aria-hidden="true" />
            <span className="sr-only">{ANCHOR_LABEL[a]}</span>
          </button>
        ))}
      </div>
      <label className="field">
        <span className="field-label">Relative to</span>
        <span className="field-input">
          <select value={ref} onChange={(e) => setRef(e.target.value as "workspace" | "stock")}>
            <option value="workspace">The workspace (galvo field)</option>
            <option value="stock" disabled={!p.doc.stock}>
              The stock{p.doc.stock ? "" : " (none defined)"}
            </option>
          </select>
        </span>
      </label>
      <Num label="Margin" unit="mm" value={margin} onChange={setMargin} title="Keep this far in from the edges" />
      <p className="hint">The selection keeps its size and rotation; each click is one undo step.</p>
    </Modal>
  );
}

/** Move Horizontally / Vertically Together: the selection packed side by side with a fixed gap. */
export function MoveTogetherDialog(p: { ids: string[]; axis: "x" | "y"; run: Run; onClose: () => void }) {
  const [gap, setGap] = useState("1");
  const [axis, setAxis] = useState<"x" | "y">(p.axis);
  const { busy, error, go } = useBusy(p.run);
  const ok = async () => {
    const g = num(gap);
    if (g === null || !Number.isFinite(g)) return void go("The spacing must be a number.");
    if (p.ids.length < 2) return void go("Select at least two objects.");
    if (await go(distributeSpacing(p.ids, axis, g))) p.onClose();
  };
  return (
    <Modal
      title={axis === "x" ? "Move horizontally together" : "Move vertically together"}
      onClose={p.onClose}
      onSubmit={() => void ok()}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void ok()}>
            Move
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">Direction</span>
        <span className="field-input">
          <select value={axis} onChange={(e) => setAxis(e.target.value as "x" | "y")}>
            <option value="x">Horizontally (left to right)</option>
            <option value="y">Vertically (bottom to top)</option>
          </select>
        </span>
      </label>
      <Num label="Spacing" unit="mm" value={gap} onChange={setGap} title="Gap between neighbouring objects' bounding boxes (negative overlaps)" />
      <p className="hint">The first object stays; the others line up after it, this far apart. A group moves as one object.</p>
    </Modal>
  );
}

/** The document's output curve tolerance (LightBurn's Curve Tolerance), saved with the project. */
export function CurveToleranceDialog(p: { doc: GDocument; run: Run; onClose: () => void }) {
  const cur = typeof p.doc.curve_tolerance_mm === "number" ? p.doc.curve_tolerance_mm : null;
  const [v, setV] = useState(cur === null ? "" : String(cur));
  const { busy, error, go } = useBusy(p.run);
  const lu = usePrefs().units;
  const ok = async () => {
    const t = v.trim() === "" ? null : num(v);
    if (v.trim() !== "" && (t === null || !(t >= 0.001 && t <= 1))) return void go(`Enter ${formatLength(0.001, lu, 3)} to ${formatLength(1, lu, 3)}, or leave it empty for the device's setting.`);
    if (t === cur) return p.onClose();
    if (await go(curveTolerance(t))) p.onClose();
  };
  return (
    <Modal
      title="Curve tolerance"
      onClose={p.onClose}
      onSubmit={() => void ok()}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void ok()}>
            Save
          </button>
        </>
      }
    >
      <Num label="Curve tolerance" unit="mm" value={v} onChange={setV} title="Largest distance between a curve and the straight pieces a job marks for it" />
      <p className="hint">
        How finely jobs turn curves into straight marks. Coarser marks faster with fewer points; finer follows curves more closely. Saved with this
        project; empty uses the device's setting ({formatLength(0.01, lu, 3)} unless the device says otherwise).
      </p>
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
  const [axis, setAxis] = useState<"x" | "y">(p.guide?.axis ?? "x");
  const [v, setV] = useState(String(p.guide?.v ?? 0));
  const [err, setErr] = useState<string | null>(null);
  const ok = () => {
    const x = num(v);
    if (x === null || Math.abs(x) > 1e6) return setErr("Enter a position in mm.");
    p.onOk(axis, Math.round(x * 1e4) / 1e4);
  };
  return (
    <Modal
      title={p.guide ? "Guide" : "Add guide"}
      onClose={p.onClose}
      onSubmit={ok}
      footer={
        <>
          {err && <span className="form-error">{err}</span>}
          {p.onDelete && (
            <button className="btn btn-danger" onClick={p.onDelete}>
              Delete
            </button>
          )}
          <button className="btn" onClick={p.onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={ok}>
            OK
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">Guide</span>
        <span className="field-input">
          <select value={axis} onChange={(e) => setAxis(e.target.value as "x" | "y")}>
            <option value="x">Vertical (at X)</option>
            <option value="y">Horizontal (at Y)</option>
          </select>
        </span>
      </label>
      <Num label={axis === "x" ? "X" : "Y"} unit="mm" value={v} onChange={setV} />
      <p className="hint">Guides are saved with the project. Drag one with the Select tool to move it, or onto a ruler to remove it.</p>
    </Modal>
  );
}

// ---- grid array -----------------------------------------------------------------------------------------------------

const MAX_PREVIEW_CELLS = 400;

/** LightBurn's Grid Array: columns, rows, spacing (centre to centre or gap), direction, alternate shifts and mirrors. */
export function GridArrayDialog(p: {
  doc: GDocument;
  ids: string[];
  workspaceMm: Vec2;
  has: (cmd: string) => boolean;
  run: Run;
  onClose: () => void;
  onCreated?: (ids: string[]) => void;
}) {
  const objs = useMemo(() => {
    const want = new Set(p.ids);
    return p.doc.objects.filter((o) => want.has(o.id) || (o.group && want.has(o.group)));
  }, [p.doc, p.ids]);
  const box = useMemo(() => unionBoxes(objs.map(objectBounds)), [objs]);
  const [bw, bh] = box ? [box.x1 - box.x0, box.y1 - box.y0] : [10, 10];
  const [cols, setCols] = useState("3");
  const [rows, setRows] = useState("2");
  const [gapMode, setGapMode] = useState(true);
  const [dx, setDx] = useState("2");
  const [dy, setDy] = useState("2");
  const [revX, setRevX] = useState(false);
  const [revY, setRevY] = useState(false);
  const [rowShift, setRowShift] = useState("");
  const [rowHalf, setRowHalf] = useState(false);
  const [colShift, setColShift] = useState("");
  const [colHalf, setColHalf] = useState(false);
  const [mirrorCols, setMirrorCols] = useState<"" | "x" | "y">("");
  const [mirrorRows, setMirrorRows] = useState<"" | "x" | "y">("");
  const newOptions = p.has(DCMD.array);
  const { busy, error, go } = useBusy(p.run, (r) => p.onCreated?.(G.resultIds(r).filter((id) => !p.ids.includes(id))));
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
    return {
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
  const problem = opts ? arrayProblem(opts, box) : "Enter numbers for the counts and spacing.";
  const outside = opts && box && !problem ? cellsOutside(opts, box, p.workspaceMm) : 0;
  const create = async () => {
    if (!opts || problem || !box) return void go(problem ?? "Enter numbers.");
    if (await go({ id: DCMD.array, params: arrayParams(p.ids, opts) })) p.onClose();
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
          <option value="">No</option>
          <option value="x">Left-right</option>
          <option value="y">Top-bottom</option>
        </select>
      </span>
    </label>
  );
  return (
    <Modal
      title="Grid array"
      wide
      onClose={p.onClose}
      onSubmit={() => void create()}
      footer={
        <>
          {(error ?? (opts ? null : problem)) && <span className="form-error">{error ?? problem}</span>}
          <button className="btn" onClick={p.onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy || !!problem || !newOptions} onClick={() => void create()}>
            Create
          </button>
        </>
      }
    >
      <div className="dz-array">
        <div className="dz-array-form">
          <div className="field-grid">
            <Num label="Columns" value={cols} onChange={setCols} />
            <Num label="Rows" value={rows} onChange={setRows} />
          </div>
          <label className="field">
            <span className="field-label">Spacing</span>
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
                <option value="gap">Padding between edges</option>
                <option value="center">Centre to centre</option>
              </select>
            </span>
          </label>
          <div className="field-grid">
            <Num label={gapMode ? "Gap X" : "Pitch X"} unit="mm" value={dx} onChange={setDx} />
            <Num label={gapMode ? "Gap Y" : "Pitch Y"} unit="mm" value={dy} onChange={setDy} />
          </div>
          <div className="field-grid">
            {check("Reverse X", revX, setRevX, "Build the columns toward -X (left)")}
            {check("Reverse Y", revY, setRevY, "Build the rows toward -Y (toward the operator)")}
          </div>
          <h4 className="et-dlg-h">Alternate rows and columns</h4>
          <div className="field-grid">
            <Num label="Row shift" unit="mm" value={rowHalf ? "" : rowShift} onChange={setRowShift} title="Shift every other row along X (brick or honeycomb layouts)" />
            {check("Half width", rowHalf, setRowHalf, "Shift every other row by half the selection's width")}
            <Num label="Column shift" unit="mm" value={colHalf ? "" : colShift} onChange={setColShift} title="Shift every other column along Y" />
            {check("Half height", colHalf, setColHalf, "Shift every other column by half the selection's height")}
          </div>
          <div className="field-grid">
            {mirrorSel("Mirror columns", mirrorCols, setMirrorCols)}
            {mirrorSel("Mirror rows", mirrorRows, setMirrorRows)}
          </div>
        </div>
        <ArrayPreview objs={objs} box={box} opts={problem ? null : opts} workspaceMm={p.workspaceMm} />
      </div>
      {outside > 0 && opts && (
        <p className="warn-text" role="status">
          {outside} of the {opts.cols * opts.rows} cells would be outside the {workspaceText(p.workspaceMm)} {isSquare(p.workspaceMm) ? "galvo field" : "workspace"}.
        </p>
      )}
      {!newOptions && <p className="warn-text">This engine has no grid array command.</p>}
      <p className="hint">
        Counts include the original (row 1, column 1). Selection {formatLength(bw, lu)} × {formatLength(bh, lu)}.
      </p>
    </Modal>
  );
}

function ArrayPreview(p: { objs: GDocument["objects"]; box: Box | null; opts: ArrayOptions | null; workspaceMm: Vec2 }) {
  const shape = useMemo(() => p.objs.map((o) => toPathData(geometryCmds(o.geometry), o.transform)).join(" "), [p.objs]);
  if (!p.box || !p.opts) return <div className="dz-array-preview empty">No preview</div>;
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
      <svg viewBox={vb} preserveAspectRatio="xMidYMid meet" role="img" aria-label="Array preview">
        <g transform="scale(1 -1)">
          <rect className="dz-ap-field" x={-p.workspaceMm[0] / 2} y={-p.workspaceMm[1] / 2} width={p.workspaceMm[0]} height={p.workspaceMm[1]} vectorEffect="non-scaling-stroke" />
          {shown.map((c) => {
            const t = `translate(${c.d[0] + cx} ${c.d[1] + cy}) scale(${c.flipX ? -1 : 1} ${c.flipY ? -1 : 1}) translate(${-cx} ${-cy})`;
            return <path key={`${c.row}-${c.col}`} className={c.row === 0 && c.col === 0 ? "dz-ap-orig" : "dz-ap-copy"} d={shape} transform={t} vectorEffect="non-scaling-stroke" />;
          })}
        </g>
      </svg>
      {cells.length > shown.length && <span className="hint">First {MAX_PREVIEW_CELLS} of {cells.length} cells shown</span>}
    </div>
  );
}
