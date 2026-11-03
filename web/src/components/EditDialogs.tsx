// Dialogs of the Modify and Arrange menus: Offset, Fillet, Transform (scale %, shear, and the mirror-line and
// two-point picks), Circular array (with a centre pick), Copy along path, Nest (Quick Nest by boxes, or true shapes),
// Optimize (LightBurn's fit to lines / arcs and smoothing), and the tolerance dialogs for Close paths and Auto-join.
// Each one runs one engine command; picks hand back to the canvas and reopen.

import { useState, type ReactNode } from "react";
import type { Invocation } from "../api/client.ts";
import * as G from "../api/geomCommands.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import { evalNumber } from "../editing.ts";
import { boxCenter, objectBounds, unionBoxes } from "../geometry.ts";
import { Modal, UnitTextField } from "./ui.tsx";
import { usePrefs } from "../workspace/usePrefs.ts";
import { fixedLength, fixedNumber, formatLength } from "../workspace/units.ts";
import { useT } from "../workspace/usePrefs.ts";
import { tr } from "../workspace/tr.ts";
import { dBool, dNum } from "../workspace/dialogDefaults.ts";
import { errorText } from "../api/errorText.ts";

export type PickKind = "circularCenter" | "mirror" | "mirrorCopy" | "rotate2" | "scale2" | "scale2rot" | "unfillet";

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

/** Number entry in the display unit (inches when chosen) whose value stays mm text: ui.tsx's `UnitTextField`. */
const Num = UnitTextField;

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
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, submit };
}

function Frame(props: { title: string; help?: string; p: Props; busy: boolean; error: string | null; ok: string; onOk: () => void; children: ReactNode }) {
  const t = useT();
  return (
    <Modal
      title={props.title}
      help={props.help}
      onClose={props.p.onClose}
      onSubmit={props.onOk}
      footer={
        <>
          {props.error && <span className="form-error">{props.error}</span>}
          <button className="btn" onClick={props.p.onClose}>
            {t("ed.cancel")}
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
/** "1 shape" / "3 shapes" (or parts), translated. */
const plural = (n: number, w: "shape" | "part") => tr(n === 1 ? (w === "shape" ? "rest.shape1" : "rest.part1") : w === "shape" ? "rest.shapesN" : "rest.partsN", { n });

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
      return <OptimizeDialog p={p} />;
    case "close":
    case "autojoin":
      return <ToleranceDialog p={p} kind={p.dialog.kind} />;
  }
}

function OffsetDialog({ p }: { p: Props }) {
  const t = useT();
  // Starting values from an imported LightBurn preferences file (OffsetDistance, OffsetDeleteOriginals).
  const [d, setD] = useState(String(Math.abs(dNum("offset", "distance_mm", 1))));
  const [direction, setDirection] = useState<G.OffsetDirection>(dNum("offset", "distance_mm", 1) < 0 ? "inward" : "outward");
  const [join, setJoin] = useState<G.OffsetJoin>("round");
  const [del, setDel] = useState(!dBool("offset", "keep_original", true));
  const [combine, setCombine] = useState(false);
  const { busy, error, submit } = useSubmit(p);
  const lu = usePrefs().units;
  const ok = () => {
    const v = num(d);
    if (v === null || v === 0 || Math.abs(v) > 1000) return submit(t("ed.distanceMustBeANonZero", { v1: formatLength(1000, lu) }));
    void submit(G.offset(p.ids, { distance_mm: v, direction, join, keep_original: !del, combine }));
  };
  return (
    <Frame help="nodes-and-paths#offset-shapes" title={t("rest.offsetTitle", { what: plural(p.ids.length, "shape") })} p={p} busy={busy} error={error} ok={t("rest.offset")} onOk={ok}>
      <Num label={t("ed.distance")} unit="mm" value={d} onChange={setD} />
      <Select label={t("ed.direction")} value={direction} onChange={setDirection} options={[["outward", "Outward"], ["inward", "Inward"], ["both", "Both"]]} />
      <Select label={t("ed.corners")} value={join} onChange={setJoin} options={[["round", "Rounded"], ["miter", "Sharp"], ["bevel", "Bevelled"]]} />
      <Check label={t("ed.deleteOriginalShapes")} checked={del} onChange={setDel} />
      <Check label={t("ed.mergeShapesFirstOneResult")} checked={combine} onChange={setCombine} title={t("ed.lightburnSOffsetAsGroupOverlapping")} />
      <p className="hint">{t("ed.resultsAreNewShapesOnEach")}</p>
    </Frame>
  );
}

function FilletDialog({ p, nodeId, corners }: { p: Props; nodeId: string | null; corners: number[] }) {
  const t = useT();
  const [r, setR] = useState("1");
  const [which, setWhich] = useState<"all" | "nodes">(nodeId && corners.length ? "nodes" : "all");
  // LightBurn's Radius tool also makes a rounded corner sharp again (path.unfillet): every one, or one clicked.
  const canSharp = p.has(G.GEO.unfillet);
  const [action, setAction] = useState<"round" | "sharpAll" | "sharpClick">("round");
  const { busy, error, submit } = useSubmit(p);
  const lu = usePrefs().units;
  const ok = () => {
    if (action === "sharpAll") return void submit(G.unfillet(p.ids));
    if (action === "sharpClick") {
      if (p.ids.length !== 1) return submit(t("ed.unfilletOneShape"));
      return p.onPick("unfillet");
    }
    const v = num(r);
    if (v === null || v === 0 || Math.abs(v) > 1000) return submit(t("ed.radiusMustBeANonZero", { v1: formatLength(1000, lu) }));
    void submit(which === "nodes" && nodeId ? G.fillet([nodeId], v, corners) : G.fillet(p.ids, v));
  };
  return (
    <Frame help="nodes-and-paths#fillet-corners" title={t("ed.filletCorners")} p={p} busy={busy} error={error} ok={action === "round" ? t("ed.filletOk") : action === "sharpAll" ? t("ed.unfilletOk") : t("ed.unfilletPickOk")} onOk={ok}>
      {canSharp && (
        <Select
          label={t("ed.filletAction")}
          value={action}
          onChange={setAction}
          options={[
            ["round", t("ed.filletRound")],
            ["sharpAll", t("ed.unfilletAll")],
            ["sharpClick", t("ed.unfilletClick")],
          ]}
        />
      )}
      {action === "round" && <Num label={t("ed.radius")} unit="mm" value={r} onChange={setR} title={t("ed.negativeCutsAConcaveBiteInstead")} />}
      {action === "round" && nodeId && corners.length > 0 && (
        <Select
          label={t("ed.corners")}
          value={which}
          onChange={setWhich}
          options={[
            ["nodes", t("ed.selectedNodesV1", { v1: corners.length })],
            ["all", t("rest.allCornersOf", { what: plural(p.ids.length, "shape") })],
          ]}
        />
      )}
      <p className="hint">{action === "round" ? t("ed.roundsCornersBetweenLinesAndCurves") : t("ed.unfilletHint")}</p>
    </Frame>
  );
}

function TransformDialog({ p }: { p: Props }) {
  const t = useT();
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
    if (a === null || b === null || a <= 0 || b <= 0) return submit(t("ed.scaleMustBeAPercentageAbove"));
    if (a === 100 && b === 100) return submit(t("rest.scaleNothing"));
    void submit(G.transformEx(p.ids, { scale: lock ? a / 100 : [a / 100, b / 100], about: "center" }), false);
  };
  const shear = () => {
    const a = num(hx);
    const b = num(hy);
    if (a === null || b === null || Math.abs(a) > 89 || Math.abs(b) > 89) return submit(t("ed.shearAnglesMustBeWithin89"));
    if (a === 0 && b === 0) return submit(t("ed.a0ShearChangesNothing"));
    void submit(G.transformEx(p.ids, { shear_deg: [a, b], about: "center" }), false);
  };
  return (
    <Modal
      help="selecting-and-transforming#transform"
      title={t("rest.transformTitle", { what: plural(p.ids.length, "shape") })}
      onClose={p.onClose}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn btn-primary" onClick={p.onClose}>
            {t("ed.done")}
          </button>
        </>
      }
    >
      <h4 className="et-dlg-h">{t("ed.scaleAboutTheCentre")}</h4>
      <div className="field-grid">
        <Num label={lock ? t("rest.scale") : t("rest.width")} unit="%" value={sx} onChange={setSx} />
        {!lock && <Num label={t("ed.height")} unit="%" value={sy} onChange={setSy} />}
      </div>
      <Check label={t("ed.keepProportions")} checked={lock} onChange={setLock} />
      <div className="et-dlg-row">
        <button className="btn" disabled={busy} onClick={scale}>
          {t("ed.applyScale")}
        </button>
      </div>
      <h4 className="et-dlg-h">{t("ed.shearSlantAboutTheCentre")}</h4>
      <div className="field-grid">
        <Num label="X" unit="°" value={hx} onChange={setHx} title={t("ed.slantsVerticalLines")} />
        <Num label="Y" unit="°" value={hy} onChange={setHy} title={t("ed.slantsHorizontalLines")} />
      </div>
      <div className="et-dlg-row">
        <button className="btn" disabled={busy} onClick={shear}>
          {t("ed.applyShear")}
        </button>
      </div>
      <h4 className="et-dlg-h">{t("ed.pickOnTheCanvas")}</h4>
      <div className="et-dlg-row">
        <button className="btn" disabled={!p.has(G.GEO.transform)} onClick={() => p.onPick("mirror")} title={t("ed.clickTwoPointsOfTheMirror")}>
          {t("ed.mirrorAcrossLine")}
        </button>
        <button className="btn" disabled={!p.has(G.GEO.transform)} onClick={() => p.onPick("mirrorCopy")} title={t("ed.clickTwoPointsOfTheMirror2")}>
          {t("ed.mirrorCopyAcrossLine")}
        </button>
        <button className="btn" disabled={!p.has(G.GEO.rotateTo)} onClick={() => p.onPick("rotate2")} title={t("ed.pivotReferencePointTargetCtrl2")}>
          {t("ed.twoPointRotate")}
        </button>
        <button className="btn" disabled={!p.has(G.GEO.scaleTo)} onClick={() => p.onPick(alsoRotate ? "scale2rot" : "scale2")} title={t("ed.pivotReferencePointTargetScalesBy")}>
          {t("ed.twoPointScale")}
        </button>
      </div>
      <Check label={t("ed.twoPointScaleAlsoRotates")} checked={alsoRotate} onChange={setAlsoRotate} />
    </Modal>
  );
}

function defaultCentre(doc: GDocument): Vec2 {
  return doc.stock ? doc.stock.center : [0, 0];
}

function CircularDialog({ p, init }: { p: Props; init: CircularValues | null }) {
  const t = useT();
  const c0 = defaultCentre(p.doc);
  const [v, setV] = useState<CircularValues>(
    init ?? { count: String(dNum("circular_array", "count", 6)), angle: "360", rotate: dBool("circular_array", "rotate_copies", true), cx: String(c0[0]), cy: String(c0[1]) },
  );
  const set = (patch: Partial<CircularValues>) => setV((x) => ({ ...x, ...patch }));
  const { busy, error, submit } = useSubmit(p);
  const lu = usePrefs().units;
  const ok = () => {
    const count = num(v.count);
    const angle = num(v.angle);
    const cx = num(v.cx);
    const cy = num(v.cy);
    if (count === null || !Number.isInteger(count) || count < 2 || count > 10000) return submit(t("ed.countMustBeAWholeNumber"));
    if (angle === null || angle === 0 || Math.abs(angle) > 360) return submit(t("ed.angleMustBeNonZeroWithin"));
    if (cx === null || cy === null) return submit(t("ed.theCentreMustBeNumbers"));
    void submit(G.circularArray(p.ids, { count, center: [cx, cy], angle_deg: angle, rotate_copies: v.rotate }));
  };
  const sel = unionBoxes(p.doc.objects.filter((o) => p.ids.includes(o.id) || (o.group && p.ids.includes(o.group))).map(objectBounds));
  return (
    <Frame help="arranging-and-arrays#circular-array" title={t("ed.circularArray")} p={p} busy={busy} error={error} ok="Create" onOk={ok}>
      <Num label={t("ed.count")} value={v.count} onChange={(count) => set({ count })} title={t("ed.itemsInTheFinishedArrayThe")} />
      <Num label={t("ed.sweep")} unit="°" value={v.angle} onChange={(angle) => set({ angle })} title={t("ed.360EvenlyRoundTheCircleLess")} />
      <Check label={t("ed.rotateCopies")} checked={v.rotate} onChange={(rotate) => set({ rotate })} title={t("ed.offKeepsEachCopyUprightLike")} />
      <div className="field-grid">
        <Num label={t("ed.centreX")} unit="mm" value={v.cx} onChange={(cx) => set({ cx })} />
        <Num label={t("ed.centreY")} unit="mm" value={v.cy} onChange={(cy) => set({ cy })} />
      </div>
      <div className="et-dlg-row">
        <button className="btn" onClick={() => p.onPick("circularCenter", v)}>
          {t("ed.pickCentre")}
        </button>
        <button className="btn" onClick={() => set({ cx: "0", cy: "0" })}>
          {t("ed.fieldCentre")}
        </button>
        {p.doc.stock && (
          <button className="btn" onClick={() => set({ cx: String(p.doc.stock?.center[0] ?? 0), cy: String(p.doc.stock?.center[1] ?? 0) })}>
            {t("ed.stockCentre")}
          </button>
        )}
      </div>
      {sel && (
        <p className="hint">
          Selection centre {fixedNumber(boxCenter(sel)[0], lu)}, {fixedLength(boxCenter(sel)[1], lu)}; the copies turn about the centre above.
        </p>
      )}
    </Frame>
  );
}

function AlongDialog({ p }: { p: Props }) {
  const t = useT();
  const [mode, setMode] = useState<"count" | "spacing">("count");
  const [count, setCount] = useState("5");
  const [spacing, setSpacing] = useState("10");
  const [rotate, setRotate] = useState(true);
  const { busy, error, submit } = useSubmit(p);
  const lu = usePrefs().units;
  const path = p.ids[p.ids.length - 1];
  const pathObj = p.doc.objects.find((o) => o.id === path);
  const ok = () => {
    if (!pathObj) return submit(t("ed.thePathSelectedLastMustBe"));
    const ids = p.ids.slice(0, -1);
    if (mode === "count") {
      const n = num(count);
      if (n === null || !Number.isInteger(n) || n < 2 || n > 10000) return submit(t("ed.countMustBeAWholeNumber2"));
      return void submit(G.alongPath(ids, path, { count: n, rotate_to_path: rotate }));
    }
    const s = num(spacing);
    if (s === null || s <= 0) return submit(t("ed.spacingMustBeAboveV1", { v1: formatLength(0, lu) }));
    void submit(G.alongPath(ids, path, { spacing_mm: s, rotate_to_path: rotate }));
  };
  return (
    <Frame help="arranging-and-arrays#copy-along-a-path" title={t("ed.copyAlongPath")} p={p} busy={busy} error={error} ok="Create" onOk={ok}>
      <p className="hint">
        {t("rest.copiesAlong", { what: plural(p.ids.length - 1, "shape") })} <b>{pathObj?.name ?? path}</b> {t("rest.selectedLast")}
      </p>
      <Select label={t("ed.spreadBy")} value={mode} onChange={setMode} options={[["count", t("ed.numberOfCopies")], ["spacing", t("ed.distanceBetweenCopies")]]} />
      {mode === "count" ? <Num label={t("ed.count")} value={count} onChange={setCount} title={t("ed.itemsIncludingTheOriginal")} /> : <Num label={t("ed.spacing")} unit="mm" value={spacing} onChange={setSpacing} />}
      <Check label={t("ed.rotateToThePath")} checked={rotate} onChange={setRotate} />
    </Frame>
  );
}

function NestDialog({ p }: { p: Props }) {
  const t = useT();
  const shapesOk = p.has(G.GEO.nest);
  // "object": the last selected shape is the material outline the others are nested into (LightBurn's nest target).
  const [region, setRegion] = useState<"stock" | "field" | "object">(p.doc.stock ? "stock" : "field");
  const [mode, setMode] = useState<"boxes" | "shapes">("shapes");
  const [spacing, setSpacing] = useState("1");
  const [edge, setEdge] = useState("0");
  const [rot, setRot] = useState<"0" | "90" | "all" | "45">(dBool("nest", "lock_rotation", false) ? "0" : "90");
  const [res, setRes] = useState("");
  // LightBurn 2.2's Multipage: what does not fit goes onto new pages (OTH-028 pages), up to a page limit.
  const [multipage, setMultipage] = useState(dBool("nest", "multipage", false));
  const [maxPages, setMaxPages] = useState("10");
  const [report, setReport] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lu = usePrefs().units;
  const target = p.ids[p.ids.length - 1];
  const targetObj = p.doc.objects.find((o) => o.id === target);
  const ok = async () => {
    const s = num(spacing);
    const e = num(edge);
    if (s === null || s < 0 || e === null || e < 0) return setError(t("ed.spacingAndEdgeGapMustBe"));
    const r = res.trim() === "" ? undefined : num(res);
    if (r === null || (r !== undefined && !(r > 0))) return setError(t("ed.theGridCellMustBeAbove", { v1: formatLength(0, lu) }));
    const rotations = rot === "0" ? [0] : rot === "90" ? [0, 90] : rot === "all" ? [0, 90, 180, 270] : [0, 45, 90, 135, 180, 225, 270, 315];
    const parts = region === "object" ? p.ids.slice(0, -1) : p.ids;
    if (region === "object" && (!targetObj || parts.length < 1)) return setError(t("ed.selectThePartsThenTheMaterial"));
    const where: G.NestRegion = region === "object" ? { object: target } : region;
    const mp = multipage ? Math.round(num(maxPages) ?? NaN) : 0;
    if (multipage && !(mp >= 1 && mp <= 100)) return setError(t("ed.nestMaxPagesRange"));
    setBusy(true);
    setError(null);
    try {
      const out = (await p.run(
        G.nest(parts, where, { spacing_mm: s, edge_mm: e, rotations, ...(mode === "shapes" ? { mode, resolution_mm: r } : {}), ...(multipage ? { multipage, max_pages: mp } : {}) }),
      )) as {
        placed?: unknown[];
        unplaced?: unknown[];
        budget_exhausted?: boolean;
        pages?: unknown[];
      } | null;
      const pages = Array.isArray(out?.pages) ? out.pages.length : 0;
      const placed = Array.isArray(out?.placed) ? out.placed.length : 0;
      const unplaced = Array.isArray(out?.unplaced) ? out.unplaced.length : 0;
      p.onDone(G.resultIds(out));
      if (unplaced || out?.budget_exhausted) {
        setReport(
          t(out?.budget_exhausted ? "rest.nestReportBudget" : "rest.nestReport", { placed: plural(placed, "part"), pages: pages ? t("ed.nestOnPages", { n: pages }) : "", unplaced: plural(unplaced, "part") }),
        );
      } else p.onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Frame help="arranging-and-arrays#quick-nest" title={t("rest.nestTitle", { what: plural(region === "object" ? p.ids.length - 1 : p.ids.length, "part") })} p={p} busy={busy} error={error} ok={t("rest.nest")} onOk={() => void ok()}>
      <Select
        label={t("ed.method")}
        value={mode}
        onChange={setMode}
        options={shapesOk ? [["shapes", t("ed.trueShapePartsInterlockFillHoles")], ["boxes", t("ed.quickNestBoundingBoxes")]] : [["boxes", t("ed.quickNestBoundingBoxes")]]}
      />
      <Select
        label={t("ed.into")}
        value={region}
        onChange={setRegion}
        options={[
          ...(p.doc.stock ? ([["stock", t("ed.theStock")]] as ["stock", string][]) : []),
          ["field", t("ed.theWholeField")],
          ...(p.ids.length >= 2 && targetObj ? ([["object", t("ed.theLastSelectedShapeV1", { v1: targetObj.name || target })]] as ["object", string][]) : []),
        ]}
      />
      <Num label={t("ed.spacing")} unit="mm" value={spacing} onChange={setSpacing} />
      <Num label={t("ed.edgeGap")} unit="mm" value={edge} onChange={setEdge} />
      <Select
        label={t("ed.rotations")}
        value={rot}
        onChange={setRot}
        options={[
          ["0", t("rest.rotNone")],
          ["90", t("rest.rot90")],
          ["all", "0°, 90°, 180°, 270°"],
          ["45", t("rest.rot45")],
        ]}
      />
      {mode === "shapes" && <Num label={t("ed.gridCell")} unit="mm" value={res} onChange={setRes} title={t("ed.smallestCellOfTheSearchGrid")} />}
      <Check label={t("ed.nestMultipage")} checked={multipage} onChange={setMultipage} title={t("ed.nestMultipageTitle")} />
      {multipage && (
        <label className="field">
          <span className="field-label">{t("ed.nestMaxPages")}</span>
          <span className="field-input">
            <input type="number" min={1} max={100} step={1} value={maxPages} onChange={(e) => setMaxPages(e.target.value)} />
          </span>
        </label>
      )}
      {report && <p className="warn-text">{report}</p>}
      <p className="hint">
        {mode === "shapes"
          ? t("ed.placesEachPartByItsReal")
          : t("ed.packsPartsByTheirBoundingBoxes")}{" "}
        {t("ed.aGroupKeepsItsLayout")}
      </p>
    </Frame>
  );
}

/** LightBurn's Optimize Selected Shapes: fit to lines, fit to arcs and smooth, each with its own tolerance. */
function OptimizeDialog({ p }: { p: Props }) {
  const t = useT();
  const [lines, setLines] = useState(true);
  const [tol, setTol] = useState("0.01");
  const [arcs, setArcs] = useState(false);
  const [arcTol, setArcTol] = useState("0.05");
  const [smooth, setSmooth] = useState(false);
  const [smoothTol, setSmoothTol] = useState("0.05");
  const { busy, error, submit } = useSubmit(p);
  const lu = usePrefs().units;
  const ok = () => {
    const vals: { tolerance_mm?: number; arcs_mm?: number; smooth_mm?: number } = {};
    for (const [on, txt, key] of [
      [lines, tol, "tolerance_mm"],
      [arcs, arcTol, "arcs_mm"],
      [smooth, smoothTol, "smooth_mm"],
    ] as const) {
      if (!on) continue;
      const v = num(txt);
      if (v === null || v < 0 || v > 10) return submit(t("ed.eachToleranceMustBeFrom0", { v1: formatLength(10, lu) }));
      vals[key] = v;
    }
    if (!lines && !arcs && !smooth) return submit(t("ed.chooseAtLeastOneOption"));
    void submit(G.optimize(p.ids, vals));
  };
  return (
    <Frame help="nodes-and-paths#path-tools" title={t("rest.optimizeTitle", { what: plural(p.ids.length, "shape") })} p={p} busy={busy} error={error} ok={t("rest.optimize")} onOk={ok}>
      <Check label={t("ed.fitShapesToLines")} checked={lines} onChange={setLines} title={t("ed.removeRedundantNodesAndZeroLength")} />
      {lines && <Num label={t("ed.lineTolerance")} unit="mm" value={tol} onChange={setTol} title={t("ed.howFarTheResultMayStray")} />}
      <Check label={t("ed.fitShapesToArcs")} checked={arcs} onChange={setArcs} title={t("ed.runsOf4OrMoreShort")} />
      {arcs && <Num label={t("ed.arcTolerance")} unit="mm" value={arcTol} onChange={setArcTol} />}
      <Check label={t("ed.smoothSourceShapes")} checked={smooth} onChange={setSmooth} title={t("ed.runsOfLinesBecomeSmoothCurves")} />
      {smooth && <Num label={t("ed.smoothing")} unit="mm" value={smoothTol} onChange={setSmoothTol} title={t("ed.largerSmoothsMoreAndRoundsSmall")} />}
      <p className="hint">{t("ed.theyRunInTheOrderSmooth")}</p>
    </Frame>
  );
}

function ToleranceDialog({ p, kind }: { p: Props; kind: "close" | "autojoin" }) {
  const t = useT();
  const [tol, setTol] = useState(kind === "autojoin" ? "0.05" : "0.5");
  const [mode, setMode] = useState<"line" | "move_ends">("line");
  const { busy, error, submit } = useSubmit(p);
  const max = kind === "close" ? 1000 : 10;
  const lu = usePrefs().units;
  const ok = () => {
    const tolMm = num(tol);
    if (tolMm === null || tolMm < 0 || tolMm > max) return submit(t("ed.toleranceMustBeFrom0To", { v1: formatLength(max, lu) }));
    if (kind === "autojoin") return void submit(G.autoJoin(p.ids, tolMm));
    void submit(G.closePaths(p.ids, { tolerance_mm: tolMm, mode }));
  };
  const title = kind === "autojoin" ? t("rest.autoJoin") : t("ed.closePathsWithTolerance");
  return (
    <Frame title={title} p={p} busy={busy} error={error} ok={kind === "autojoin" ? t("rest.join") : t("rest.close")} onOk={ok}>
      <Num
        label={t("ed.tolerance")}
        unit="mm"
        value={tol}
        onChange={setTol}
        title={kind === "autojoin" ? t("ed.largestGapBetweenEndsThatStill") : t("ed.onlyPathsWhoseEndsAreThis")}
      />
      {kind === "close" && <Select label={t("ed.closeBy")} value={mode} onChange={setMode} options={[["line", t("ed.addingALine")], ["move_ends", t("ed.movingTheEndsTogether")]]} />}
      <p className="hint">
        {kind === "autojoin" ? t("ed.joinsOpenPathsWhoseEndsLie") : t("ed.closesOpenPathsWhoseTwoEnds")}
      </p>
    </Frame>
  );
}
