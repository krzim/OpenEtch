// Dialogs of the Modify and Arrange menus: Offset, Fillet, Transform (scale %, shear, and the mirror-line and
// two-point picks), Circular array (with a centre pick), Copy along path, Quick nest, and the tolerance dialogs for
// Optimize, Close paths and Auto-join. Each one runs one engine command; picks hand back to the canvas and reopen.

import { useState, type ReactNode } from "react";
import type { Invocation } from "../api/client.ts";
import * as G from "../api/geomCommands.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import { evalNumber } from "../editing.ts";
import { boxCenter, objectBounds, unionBoxes } from "../geometry.ts";
import { Modal } from "./ui.tsx";
import { useUnitConversion } from "../workspace/usePrefs.ts";
import { trimNumber } from "../workspace/units.ts";

export type PickKind = "circularCenter" | "mirror" | "rotate2" | "scale2" | "scale2rot";

export interface CircularValues {
  count: string;
  angle: string;
  rotate: boolean;
  cx: string;
  cy: string;
}

export type EditDialog =
  | { kind: "offset" }
  | { kind: "fillet"; nodeId: string | null; corners: number[] }
  | { kind: "transform" }
  | { kind: "circular"; values: CircularValues | null }
  | { kind: "along" }
  | { kind: "nest" }
  | { kind: "optimize" }
  | { kind: "close" }
  | { kind: "autojoin" };

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

interface Props {
  dialog: EditDialog;
  doc: GDocument;
  /** The selection's units (a group counts once), in selection order. */
  ids: string[];
  run: Run;
  has: (cmd: string) => boolean;
  onClose: () => void;
  /** Ids to select after the edit. */
  onDone: (ids: string[]) => void;
  /** Close the dialog and pick points on the canvas. */
  onPick: (kind: PickKind, values?: CircularValues) => void;
}

/**
 * A number entry whose value stays in engine units (mm): with Preferences set to inches the field shows and takes
 * inches (workspace/usePrefs.ts) and hands the converted mm text back, like ui.tsx's NumberField.
 */
function Num(props: { label: string; value: string; onChange: (v: string) => void; unit?: string; title?: string }) {
  const conv = useUnitConversion(props.unit);
  // What the user typed, kept while the parent's value is still the one that text produced.
  const [typed, setTyped] = useState<{ text: string; value: string } | null>(null);
  const converts = conv.unit !== props.unit;
  let shown = props.value;
  if (converts) {
    const n = evalNumber(props.value);
    shown = typed && typed.value === props.value ? typed.text : n === null ? props.value : trimNumber(conv.to(n), 4 + conv.extraDigits);
  }
  const change = (text: string) => {
    if (!converts) return props.onChange(text);
    const n = evalNumber(text);
    const value = n === null ? text : String(conv.from(n));
    setTyped({ text, value });
    props.onChange(value);
  };
  return (
    <label className="field" title={props.title}>
      <span className="field-label">{props.label}</span>
      <span className="field-input">
        <input type="text" inputMode="decimal" value={shown} onChange={(e) => change(e.target.value)} />
        {conv.unit && <span className="unit">{conv.unit}</span>}
      </span>
    </label>
  );
}

function Check(props: { label: string; checked: boolean; onChange: (v: boolean) => void; title?: string }) {
  return (
    <label className="field field-check" title={props.title}>
      <span className="field-label">{props.label}</span>
      <input type="checkbox" checked={props.checked} onChange={(e) => props.onChange(e.target.checked)} />
    </label>
  );
}

function Select<T extends string>(props: { label: string; value: T; options: [T, string][]; onChange: (v: T) => void }) {
  return (
    <label className="field">
      <span className="field-label">{props.label}</span>
      <span className="field-input">
        <select value={props.value} onChange={(e) => props.onChange(e.target.value as T)}>
          {props.options.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </select>
      </span>
    </label>
  );
}

/** Runs the dialog's command; closes and selects the result on success, shows the engine's error otherwise. */
function useSubmit(p: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async (inv: Invocation | Invocation[] | string, close = true) => {
    if (busy) return;
    if (typeof inv === "string") {
      setError(inv);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const r = await p.run(inv);
      p.onDone(G.resultIds(r));
      if (close) p.onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, submit };
}

function Frame(props: { title: string; p: Props; busy: boolean; error: string | null; ok: string; onOk: () => void; children: ReactNode }) {
  return (
    <Modal
      title={props.title}
      onClose={props.p.onClose}
      onSubmit={props.onOk}
      footer={
        <>
          {props.error && <span className="form-error">{props.error}</span>}
          <button className="btn" onClick={props.p.onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={props.busy} onClick={props.onOk}>
            {props.ok}
          </button>
        </>
      }
    >
      {props.children}
    </Modal>
  );
}

const num = (v: string) => evalNumber(v);
const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

export function EditDialogs(p: Props) {
  switch (p.dialog.kind) {
    case "offset":
      return <OffsetDialog p={p} />;
    case "fillet":
      return <FilletDialog p={p} nodeId={p.dialog.nodeId} corners={p.dialog.corners} />;
    case "transform":
      return <TransformDialog p={p} />;
    case "circular":
      return <CircularDialog p={p} init={p.dialog.values} />;
    case "along":
      return <AlongDialog p={p} />;
    case "nest":
      return <NestDialog p={p} />;
    case "optimize":
    case "close":
    case "autojoin":
      return <ToleranceDialog p={p} kind={p.dialog.kind} />;
  }
}

function OffsetDialog({ p }: { p: Props }) {
  const [d, setD] = useState("1");
  const [direction, setDirection] = useState<G.OffsetDirection>("outward");
  const [join, setJoin] = useState<G.OffsetJoin>("round");
  const [del, setDel] = useState(false);
  const [combine, setCombine] = useState(false);
  const { busy, error, submit } = useSubmit(p);
  const ok = () => {
    const v = num(d);
    if (v === null || v === 0 || Math.abs(v) > 1000) return submit("Distance must be a non-zero number up to 1000 mm.");
    void submit(G.offset(p.ids, { distance_mm: v, direction, join, keep_original: !del, combine }));
  };
  return (
    <Frame title={`Offset ${plural(p.ids.length, "shape")}`} p={p} busy={busy} error={error} ok="Offset" onOk={ok}>
      <Num label="Distance" unit="mm" value={d} onChange={setD} />
      <Select label="Direction" value={direction} onChange={setDirection} options={[["outward", "Outward"], ["inward", "Inward"], ["both", "Both"]]} />
      <Select label="Corners" value={join} onChange={setJoin} options={[["round", "Rounded"], ["miter", "Sharp"], ["bevel", "Bevelled"]]} />
      <Check label="Delete original shapes" checked={del} onChange={setDel} />
      <Check label="Merge shapes first (one result)" checked={combine} onChange={setCombine} title="LightBurn's 'Offset as group': overlapping shapes are merged before offsetting" />
      <p className="hint">Results are new shapes on each source's layer. Open paths are offset on both sides.</p>
    </Frame>
  );
}

function FilletDialog({ p, nodeId, corners }: { p: Props; nodeId: string | null; corners: number[] }) {
  const [r, setR] = useState("1");
  const [which, setWhich] = useState<"all" | "nodes">(nodeId && corners.length ? "nodes" : "all");
  const { busy, error, submit } = useSubmit(p);
  const ok = () => {
    const v = num(r);
    if (v === null || v === 0 || Math.abs(v) > 1000) return submit("Radius must be a non-zero number up to 1000 mm.");
    void submit(which === "nodes" && nodeId ? G.fillet([nodeId], v, corners) : G.fillet(p.ids, v));
  };
  return (
    <Frame title="Fillet corners" p={p} busy={busy} error={error} ok="Fillet" onOk={ok}>
      <Num label="Radius" unit="mm" value={r} onChange={setR} title="Negative cuts a concave bite instead (LightBurn)" />
      {nodeId && corners.length > 0 && (
        <Select
          label="Corners"
          value={which}
          onChange={setWhich}
          options={[
            ["nodes", `Selected nodes (${corners.length})`],
            ["all", `All corners of ${plural(p.ids.length, "shape")}`],
          ]}
        />
      )}
      <p className="hint">Only corners between two straight lines are rounded. A negative radius cuts a concave bite.</p>
    </Frame>
  );
}

function TransformDialog({ p }: { p: Props }) {
  const [sx, setSx] = useState("100");
  const [sy, setSy] = useState("100");
  const [lock, setLock] = useState(true);
  const [hx, setHx] = useState("0");
  const [hy, setHy] = useState("0");
  const [alsoRotate, setAlsoRotate] = useState(false);
  const { busy, error, submit } = useSubmit(p);
  const scale = () => {
    const a = num(sx);
    const b = lock ? a : num(sy);
    if (a === null || b === null || a <= 0 || b <= 0) return submit("Scale must be a percentage above 0.");
    if (a === 100 && b === 100) return submit("100 % changes nothing.");
    void submit(G.transformEx(p.ids, { scale: lock ? a / 100 : [a / 100, b / 100], about: "center" }), false);
  };
  const shear = () => {
    const a = num(hx);
    const b = num(hy);
    if (a === null || b === null || Math.abs(a) > 89 || Math.abs(b) > 89) return submit("Shear angles must be within ±89°.");
    if (a === 0 && b === 0) return submit("A 0° shear changes nothing.");
    void submit(G.transformEx(p.ids, { shear_deg: [a, b], about: "center" }), false);
  };
  return (
    <Modal
      title={`Transform ${plural(p.ids.length, "shape")}`}
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
      <h4 className="et-dlg-h">Scale about the centre</h4>
      <div className="field-grid">
        <Num label={lock ? "Scale" : "Width"} unit="%" value={sx} onChange={setSx} />
        {!lock && <Num label="Height" unit="%" value={sy} onChange={setSy} />}
      </div>
      <Check label="Keep proportions" checked={lock} onChange={setLock} />
      <div className="et-dlg-row">
        <button className="btn" disabled={busy} onClick={scale}>
          Apply scale
        </button>
      </div>
      <h4 className="et-dlg-h">Shear (slant) about the centre</h4>
      <div className="field-grid">
        <Num label="X" unit="°" value={hx} onChange={setHx} title="Slants vertical lines" />
        <Num label="Y" unit="°" value={hy} onChange={setHy} title="Slants horizontal lines" />
      </div>
      <div className="et-dlg-row">
        <button className="btn" disabled={busy} onClick={shear}>
          Apply shear
        </button>
      </div>
      <h4 className="et-dlg-h">Pick on the canvas</h4>
      <div className="et-dlg-row">
        <button className="btn" disabled={!p.has(G.GEO.transform)} onClick={() => p.onPick("mirror")} title="Click two points of the mirror line">
          Mirror across line…
        </button>
        <button className="btn" disabled={!p.has(G.GEO.rotateTo)} onClick={() => p.onPick("rotate2")} title="Pivot, reference point, target (Ctrl+2)">
          Two-point rotate…
        </button>
        <button className="btn" disabled={!p.has(G.GEO.scaleTo)} onClick={() => p.onPick(alsoRotate ? "scale2rot" : "scale2")} title="Pivot, reference point, target: scales by the ratio of distances">
          Two-point scale…
        </button>
      </div>
      <Check label="Two-point scale also rotates" checked={alsoRotate} onChange={setAlsoRotate} />
    </Modal>
  );
}

function defaultCentre(doc: GDocument): Vec2 {
  return doc.stock ? doc.stock.center : [0, 0];
}

function CircularDialog({ p, init }: { p: Props; init: CircularValues | null }) {
  const c0 = defaultCentre(p.doc);
  const [v, setV] = useState<CircularValues>(init ?? { count: "6", angle: "360", rotate: true, cx: String(c0[0]), cy: String(c0[1]) });
  const set = (patch: Partial<CircularValues>) => setV((x) => ({ ...x, ...patch }));
  const { busy, error, submit } = useSubmit(p);
  const ok = () => {
    const count = num(v.count);
    const angle = num(v.angle);
    const cx = num(v.cx);
    const cy = num(v.cy);
    if (count === null || !Number.isInteger(count) || count < 2 || count > 10000) return submit("Count must be a whole number from 2 to 10000 (the original included).");
    if (angle === null || angle === 0 || Math.abs(angle) > 360) return submit("Angle must be non-zero, within ±360°.");
    if (cx === null || cy === null) return submit("The centre must be numbers.");
    void submit(G.circularArray(p.ids, { count, center: [cx, cy], angle_deg: angle, rotate_copies: v.rotate }));
  };
  const sel = unionBoxes(p.doc.objects.filter((o) => p.ids.includes(o.id) || (o.group && p.ids.includes(o.group))).map(objectBounds));
  return (
    <Frame title="Circular array" p={p} busy={busy} error={error} ok="Create" onOk={ok}>
      <Num label="Count" value={v.count} onChange={(count) => set({ count })} title="Items in the finished array, the original included" />
      <Num label="Sweep" unit="°" value={v.angle} onChange={(angle) => set({ angle })} title="360: evenly round the circle; less: the last copy at the end of the sweep; negative: clockwise" />
      <Check label="Rotate copies" checked={v.rotate} onChange={(rotate) => set({ rotate })} title="Off keeps each copy upright (like clock numerals)" />
      <div className="field-grid">
        <Num label="Centre X" unit="mm" value={v.cx} onChange={(cx) => set({ cx })} />
        <Num label="Centre Y" unit="mm" value={v.cy} onChange={(cy) => set({ cy })} />
      </div>
      <div className="et-dlg-row">
        <button className="btn" onClick={() => p.onPick("circularCenter", v)}>
          Pick centre…
        </button>
        <button className="btn" onClick={() => set({ cx: "0", cy: "0" })}>
          Field centre
        </button>
        {p.doc.stock && (
          <button className="btn" onClick={() => set({ cx: String(p.doc.stock?.center[0] ?? 0), cy: String(p.doc.stock?.center[1] ?? 0) })}>
            Stock centre
          </button>
        )}
      </div>
      {sel && (
        <p className="hint">
          Selection centre {boxCenter(sel)[0].toFixed(2)}, {boxCenter(sel)[1].toFixed(2)} mm; the copies turn about the centre above.
        </p>
      )}
    </Frame>
  );
}

function AlongDialog({ p }: { p: Props }) {
  const [mode, setMode] = useState<"count" | "spacing">("count");
  const [count, setCount] = useState("5");
  const [spacing, setSpacing] = useState("10");
  const [rotate, setRotate] = useState(true);
  const { busy, error, submit } = useSubmit(p);
  const path = p.ids[p.ids.length - 1];
  const pathObj = p.doc.objects.find((o) => o.id === path);
  const ok = () => {
    if (!pathObj) return submit("The path (selected last) must be a single shape, not a group.");
    const ids = p.ids.slice(0, -1);
    if (mode === "count") {
      const n = num(count);
      if (n === null || !Number.isInteger(n) || n < 2 || n > 10000) return submit("Count must be a whole number from 2 to 10000.");
      return void submit(G.alongPath(ids, path, { count: n, rotate_to_path: rotate }));
    }
    const s = num(spacing);
    if (s === null || s <= 0) return submit("Spacing must be above 0 mm.");
    void submit(G.alongPath(ids, path, { spacing_mm: s, rotate_to_path: rotate }));
  };
  return (
    <Frame title="Copy along path" p={p} busy={busy} error={error} ok="Create" onOk={ok}>
      <p className="hint">
        Copies {plural(p.ids.length - 1, "shape")} along <b>{pathObj?.name ?? path}</b> (the shape selected last).
      </p>
      <Select label="Spread by" value={mode} onChange={setMode} options={[["count", "Number of copies"], ["spacing", "Distance between copies"]]} />
      {mode === "count" ? <Num label="Count" value={count} onChange={setCount} title="Items including the original" /> : <Num label="Spacing" unit="mm" value={spacing} onChange={setSpacing} />}
      <Check label="Rotate to the path" checked={rotate} onChange={setRotate} />
    </Frame>
  );
}

function NestDialog({ p }: { p: Props }) {
  const [region, setRegion] = useState<"stock" | "field">(p.doc.stock ? "stock" : "field");
  const [spacing, setSpacing] = useState("1");
  const [edge, setEdge] = useState("0");
  const [rot, setRot] = useState<"0" | "90" | "all">("90");
  const { busy, error, submit } = useSubmit(p);
  const ok = () => {
    const s = num(spacing);
    const e = num(edge);
    if (s === null || s < 0 || e === null || e < 0) return submit("Spacing and edge gap must be numbers ≥ 0.");
    const rotations = rot === "0" ? [0] : rot === "90" ? [0, 90] : [0, 90, 180, 270];
    void submit(G.nest(p.ids, region, { spacing_mm: s, edge_mm: e, rotations }));
  };
  return (
    <Frame title={`Quick nest ${plural(p.ids.length, "part")}`} p={p} busy={busy} error={error} ok="Nest" onOk={ok}>
      <Select
        label="Into"
        value={region}
        onChange={setRegion}
        options={p.doc.stock ? [["stock", "The stock"], ["field", "The whole field"]] : [["field", "The whole field"]]}
      />
      <Num label="Spacing" unit="mm" value={spacing} onChange={setSpacing} />
      <Num label="Edge gap" unit="mm" value={edge} onChange={setEdge} />
      <Select label="Rotations" value={rot} onChange={setRot} options={[["0", "None"], ["90", "0° and 90°"], ["all", "0°, 90°, 180°, 270°"]]} />
      <p className="hint">Packs parts by their bounding boxes (like LightBurn's Quick Nest). A group keeps its layout.</p>
    </Frame>
  );
}

function ToleranceDialog({ p, kind }: { p: Props; kind: "optimize" | "close" | "autojoin" }) {
  const [tol, setTol] = useState(kind === "optimize" ? "0.01" : kind === "autojoin" ? "0.05" : "0.5");
  const [mode, setMode] = useState<"line" | "move_ends">("line");
  const { busy, error, submit } = useSubmit(p);
  const max = kind === "close" ? 1000 : 10;
  const ok = () => {
    const t = num(tol);
    if (t === null || t < 0 || t > max) return submit(`Tolerance must be from 0 to ${max} mm.`);
    if (kind === "optimize") return void submit(G.optimize(p.ids, t));
    if (kind === "autojoin") return void submit(G.autoJoin(p.ids, t));
    void submit(G.closePaths(p.ids, { tolerance_mm: t, mode }));
  };
  const title = kind === "optimize" ? "Optimize shapes" : kind === "autojoin" ? "Auto-join" : "Close paths with tolerance";
  return (
    <Frame title={title} p={p} busy={busy} error={error} ok={kind === "optimize" ? "Optimize" : kind === "autojoin" ? "Join" : "Close"} onOk={ok}>
      <Num
        label="Tolerance"
        unit="mm"
        value={tol}
        onChange={setTol}
        title={kind === "optimize" ? "How far the result may stray from the original" : kind === "autojoin" ? "Largest gap between ends that still joins" : "Only paths whose ends are this close are closed"}
      />
      {kind === "close" && <Select label="Close by" value={mode} onChange={setMode} options={[["line", "Adding a line"], ["move_ends", "Moving the ends together"]]} />}
      <p className="hint">
        {kind === "optimize"
          ? "Removes zero-length segments, straightens flat curves and drops redundant nodes."
          : kind === "autojoin"
            ? "Joins open paths whose ends lie within the tolerance into continuous paths."
            : "Closes open paths whose two ends are within the tolerance."}
      </p>
    </Frame>
  );
}
