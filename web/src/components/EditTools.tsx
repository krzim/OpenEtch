// Editing tools on the canvas, following LightBurn's behaviour and hotkeys with Fusion-like visuals:
// - Edit Nodes (Ctrl+`): the selected shape's nodes and control handles (node.list). Drag nodes, handles or a segment
//   (one node.move per node on release, drawn live meanwhile), click a segment to add a node, marquee or Shift-click to
//   select nodes. Keys act on the node or segment under the pointer, else on the selected nodes, as LightBurn's do:
//   Delete/D delete, B break, J join two end nodes, S smooth (a line: make it a curve), C corner, L line, K curve,
//   I insert at the pointer, M insert at the middle, T trim. Rectangles and ellipses offer "Convert to path" first.
// - Line (Ctrl+L): click points, double-click or Enter finishes, C (or a click on the first point) closes, Backspace
//   removes the last point, Shift keeps 15° steps. Pen: the same, dragging a point makes it a smooth curve node.
// - Polygon / star: drag from the centre to a corner (Shift: 15° steps); sides, star and corner radius in the bar.
// - Measure: click two points or drag (snapped): distance, ΔX, ΔY, angle. Trim: click the piece to remove.
// - Point picks for the dialogs (circular array centre, mirror line, two-point rotate/scale; Ctrl+2).
// - Rulers along the top and left (mm), and guides dragged out of them (saved in the document with guide.add / move /
//   delete; double-click one, or Arrange > Add guide, to type its position; drag a guide back onto a ruler to remove
//   it). Guides an older version kept in this browser move into the document once. Engines without guide commands keep
//   them per project in this browser (guides.ts). Guides are snap targets.
// - The Modify and Arrange dropdowns (booleans, offset, path tools, draw order, lock, make same size, transform,
//   arrays, nesting) and their hotkeys (Ctrl+W weld, Alt++ / Alt+- / Alt+* booleans, Alt+C cut shapes, Alt+O offset,
//   Alt+B break apart, Alt+J auto-join, Alt+D delete duplicates, Ctrl+Shift+C convert, Alt+Shift+O optimize,
//   PgUp/PgDn and Ctrl+PgUp/PgDn draw order).
// Every edit is an engine command. Live tool state sits in a small store that only the canvas layer and the overlay
// subscribe to, so moving the pointer does not re-render the whole app.

import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { execute, type Invocation } from "../api/client.ts";
import * as G from "../api/geomCommands.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import type { CanvasCtx, CanvasTool, RenderCtx } from "../canvasTool.ts";
import {
  cleanPoints,
  constrainAngle,
  measure,
  measureText,
  penHasCurves,
  penNode,
  penPathData,
  polygonFromDrag,
  polygonPoints,
  type PenNode,
} from "../drawTools.ts";
import { constrainDelta, constraintLine, roundMm, type Mods } from "../editing.ts";
import { boxFromPoints, geometryCmds, toPathData } from "../geometry.ts";
import { getPrefs, handleScale } from "../workspace/prefs.ts";
import { usePrefs, useT, useTD } from "../workspace/usePrefs.ts";
import { fixedLength, fixedNumber, formatLength, lengthDigits, lengthFromDisplay, lengthToDisplay, trimNumber } from "../workspace/units.ts";
import { guideLabel, guideSnapPart, loadGuidesFor, newGuideId, onGuidesChanged, saveGuidesFor, type Guide } from "../guides.ts";
import { DCMD, extend as extendCmd, guideAdd, guideDelete, guideMove } from "../design/designApi.ts";
import { GuideDialog } from "../design/DesignDialogs.tsx";
import { docGuides, guidesToMigrate } from "../design/model.ts";
import {
  clickNode,
  handlesShown,
  hitNodes,
  joinPair,
  moveHandle,
  moveNodes,
  nodePathData,
  nodesInBox,
  parseNodeList,
  segmentPathData,
  segmentsBetween,
  type NodeHit,
  type NodeList,
} from "../nodes.ts";
import { unitsOf, type Selection } from "../selection.ts";
import type { MenuItem } from "./ContextMenu.tsx";
import { EditDialogs, type CircularValues, type EditDialog, type PickKind } from "./EditDialogs.tsx";
import { EditToolbar } from "./EditMenus.tsx";
import { RULER_PX, Rulers } from "./Rulers.tsx";
import type { Tool } from "./Viewport.tsx";
import "./edit.css";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export interface EditToolsProps {
  doc: GDocument | null;
  selection: Selection;
  setSelection: (s: Selection) => void;
  selectIds: (ids: string[]) => void;
  tool: Tool;
  setTool: (t: Tool) => void;
  activeLayer: string | null;
  workspaceMm: Vec2;
  /** The Design workspace is shown (tools and hotkeys only work there). */
  design: boolean;
  /** Another dialog of the app is open: hotkeys stay off. */
  blocked: boolean;
  has: (cmd: string) => boolean;
  run: Run;
  notify: (msg: string, kind?: "info" | "error") => void;
  report: (e: unknown) => void;
  /** Open the app's grid array dialog. */
  onGridArray: () => void;
  /** More entries for the Arrange dropdown (src/design/). */
  arrangeExtra?: MenuItem[];
  /**
   * Which project the guides belong to (`guideScope`), or null while the document and project are still loading
   * (guides are then neither loaded nor stored).
   */
  guideScope: string | null;
}

export interface EditTools {
  canvas: CanvasTool;
  toolbar: ReactNode;
  dialogs: ReactNode;
  /** Items for the canvas context menu. */
  menuItems: MenuItem[];
  /** Workspace hotkey handlers (workspace/hotkeys.ts ids): LightBurn's Mirror Across Line (Ctrl+Shift+M). */
  handlers: Record<string, () => boolean | void>;
}

// ---- live state ------------------------------------------------------------------------------------------------------

interface Pick {
  title: string;
  labels: string[];
  pts: Vec2[];
  /** How the picked points are previewed. */
  preview: "point" | "line" | "two-point";
  done: (pts: Vec2[]) => void;
  cancel: () => void;
}

interface Live {
  nl: NodeList | null;
  /** Why there are no nodes (nothing selected, an image, an error). */
  nlNote: string | null;
  nodeSel: ReadonlySet<number>;
  /** The node list as a drag (or a sent edit) will leave it, until the engine's list arrives. */
  preview: NodeList | null;
  hover: NodeHit | null;
  nodeBox: { a: Vec2; b: Vec2 } | null;
  pen: PenNode[];
  rubber: Vec2 | null;
  poly: { c: Vec2; p: Vec2; shift: boolean } | null;
  meas: { a: Vec2; b: Vec2; done: boolean } | null;
  trimHover: string | null;
  pick: Pick | null;
  pickHover: Vec2 | null;
  guideDrag: { id: string | null; axis: "x" | "y"; v: number; remove: boolean } | null;
}

const INITIAL: Live = {
  nl: null,
  nlNote: null,
  nodeSel: new Set(),
  preview: null,
  hover: null,
  nodeBox: null,
  pen: [],
  rubber: null,
  poly: null,
  meas: null,
  trimHover: null,
  pick: null,
  pickHover: null,
  guideDrag: null,
};

interface Store<T> {
  get: () => T;
  set: (patch: Partial<T>) => void;
  sub: (f: () => void) => () => void;
}

function createStore<T>(init: T): Store<T> {
  let s = init;
  const subs = new Set<() => void>();
  return {
    get: () => s,
    set: (patch) => {
      s = { ...s, ...patch };
      for (const f of subs) f();
    },
    sub: (f) => {
      subs.add(f);
      return () => subs.delete(f);
    },
  };
}

function useLive(store: Store<Live>): Live {
  return useSyncExternalStore(store.sub, store.get);
}

type Gesture =
  | { kind: "none" }
  | { kind: "guide"; id: string | null; axis: "x" | "y" }
  | { kind: "nodes"; start: Vec2; base: Vec2; indices: number[]; startScreen: Vec2; moved: boolean; d: Vec2; tapped: number }
  | { kind: "handle"; index: number; side: "in" | "out"; startScreen: Vec2; moved: boolean; to: Vec2 }
  | { kind: "segment"; seg: number; t: number; start: Vec2; startScreen: Vec2; moved: boolean; d: Vec2 }
  | { kind: "nbox"; a: Vec2; startScreen: Vec2; moved: boolean; shift: boolean }
  | { kind: "pen"; p: Vec2; startScreen: Vec2; moved: boolean }
  | { kind: "poly"; c: Vec2 }
  | { kind: "measure"; startScreen: Vec2; moved: boolean };

/** Screen px a press may wander before it becomes a drag. */
const SLOP = 4;
/** Node and handle pick radius, screen px. */
const NODE_TOL_PX = 7;
/** Two presses this close in time (ms) and space (px) are a double-click. */
const DBL_MS = 350;
/** A click this close (px) to the first point closes the line. */
const CLOSE_PX = 8;

export interface PolyOpts {
  sides: number;
  star: boolean;
  /** Inner corner radius as a fraction of the outer one. */
  ratio: number;
  cornerMm: number;
}

const POLY_KEY = "openetch.polygon";
const DEFAULT_POLY: PolyOpts = { sides: 6, star: false, ratio: 0.5, cornerMm: 0 };

function loadPolyOpts(): PolyOpts {
  try {
    const v = JSON.parse(localStorage.getItem(POLY_KEY) ?? "null") as Partial<PolyOpts> | null;
    if (!v || typeof v !== "object") return DEFAULT_POLY;
    const sides = Number.isInteger(v.sides) && (v.sides as number) >= 3 && (v.sides as number) <= 1000 ? (v.sides as number) : 6;
    const ratio = typeof v.ratio === "number" && v.ratio > 0 && v.ratio < 1 ? v.ratio : 0.5;
    const cornerMm = typeof v.cornerMm === "number" && v.cornerMm >= 0 ? v.cornerMm : 0;
    return { sides, star: v.star === true, ratio, cornerMm };
  } catch {
    return DEFAULT_POLY;
  }
}

const dist = (a: Vec2, b: Vec2) => Math.hypot(a[0] - b[0], a[1] - b[1]);
const rp = (p: Vec2): Vec2 => [roundMm(p[0]), roundMm(p[1])];
const r6 = (v: number) => Math.round(v * 1e6) / 1e6;

function typingIn(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

/** The canvas or nothing in particular has the keyboard (LightBurn's single-letter keys are canvas keys). */
function canvasFocused(): boolean {
  const a = document.activeElement;
  return !a || a === document.body || !!a.closest?.(".viewport");
}

/** The display length unit, read when a readout is written (Preferences can change between drags). */
const lu = () => getPrefs().units;

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const SHAPE_WORD: Record<string, string> = { rect: "rectangle", ellipse: "ellipse", path: "path" };

// ---- the hook --------------------------------------------------------------------------------------------------------

export function useEditTools(p: EditToolsProps): EditTools {
  const t = useT();
  const beginnerMode = usePrefs().beginnerMode;
  // The verb in "Select a shape to {what}" / "This engine cannot {what} yet", translated (keys "et.do.<english>").
  const td = useTD();
  const doWhat = (what: string) => td(`et.do.${what}`, what);
  const P = useRef(p);
  P.current = p;
  const store = useMemo(() => createStore<Live>(INITIAL), []);
  const gest = useRef<Gesture>({ kind: "none" });
  const lastCtx = useRef<CanvasCtx | null>(null);
  const cursor = useRef<Vec2 | null>(null);
  const lastTap = useRef<{ t: number; s: Vec2 } | null>(null);
  const [dialog, setDialog] = useState<EditDialog | null>(null);
  const [polyOpts, setPolyOptsRaw] = useState<PolyOpts>(loadPolyOpts);
  const [nodeTick, setNodeTick] = useState(0);
  const setPolyOpts = (o: PolyOpts) => {
    setPolyOptsRaw(o);
    try {
      localStorage.setItem(POLY_KEY, JSON.stringify(o));
    } catch {
      // Storage unavailable: the options last for this visit.
    }
  };

  // ---- guides: in the document (guide.* commands); per project in this browser on older engines (guides.ts) ----
  const gscope = p.guideScope;
  const docMode = p.has(DCMD.guideAdd);
  const [browserGuides, setGuides] = useState<Guide[]>([]);
  const docG = p.doc?.guides;
  const guides = useMemo(() => (docMode ? docGuides(P.current.doc) : browserGuides), [docMode, docG, browserGuides]);
  const guidesRef = useRef(guides);
  guidesRef.current = guides;
  const [guideDlg, setGuideDlg] = useState<{ id: string | null; axis: "x" | "y"; v: number } | null>(null);
  const lastGuideTap = useRef<{ id: string; t: number } | null>(null);
  const gscopeRef = useRef(gscope);
  const docNameRef = useRef(p.doc?.name);
  docNameRef.current = p.doc?.name;
  useEffect(() => {
    gscopeRef.current = gscope;
    setGuides(gscope === null ? [] : loadGuidesFor(gscope, docNameRef.current));
  }, [gscope]);
  // A save carried the guides to the project's slot (or New cleared the unsaved one): reload when it is ours.
  useEffect(
    () =>
      onGuidesChanged((scope) => {
        if (scope === gscopeRef.current) setGuides(loadGuidesFor(scope, docNameRef.current));
      }),
    [],
  );
  const commitGuides = (next: Guide[]) => {
    setGuides(next);
    if (gscopeRef.current !== null) saveGuidesFor(gscopeRef.current, next);
  };
  const guideOps = {
    add: (axis: "x" | "y", v: number) =>
      docMode ? void exec(guideAdd(axis, v)) : commitGuides([...guidesRef.current, { id: newGuideId(guidesRef.current), axis, v }]),
    move: (id: string, v: number, axis?: "x" | "y") =>
      docMode ? void exec(guideMove(id, v, axis)) : commitGuides(guidesRef.current.map((x) => (x.id === id ? { ...x, v, axis: axis ?? x.axis } : x))),
    remove: (id: string) => (docMode ? void exec(guideDelete([id])) : commitGuides(guidesRef.current.filter((x) => x.id !== id))),
    clear: () => {
      if (!docMode) commitGuides([]);
      else if (guidesRef.current.length) void exec(guideDelete(null));
    },
  };
  // Guides an older version kept in this browser for this project move into the document, once.
  const hasDoc = !!p.doc;
  useEffect(() => {
    if (!docMode || !hasDoc || gscope === null) return;
    const local = loadGuidesFor(gscope, docNameRef.current);
    if (!local.length) return;
    saveGuidesFor(gscope, []);
    const add = guidesToMigrate(local, docGuides(P.current.doc));
    if (add.length)
      void exec(
        add.map((g) => guideAdd(g.axis, g.v)),
        () => notify(t(add.length === 1 ? "et.guidesMoved1" : "et.guidesMovedN", { n: add.length })),
      );
  }, [docMode, hasDoc, gscope]);
  const [wsW, wsH] = p.workspaceMm;
  const snapParts = useMemo(() => {
    const ext = Math.max(wsW, wsH) * 2;
    const part = guideSnapPart(guides, { x0: -ext, y0: -ext, x1: ext, y1: ext });
    return part ? [part] : [];
  }, [guides, wsW, wsH]);

  // ---- helpers ----
  const notify = (m: string, k: "info" | "error" = "info") => P.current.notify(m, k);
  const exec = (inv: Invocation | Invocation[], after?: (r: unknown) => void): Promise<void> =>
    P.current
      .run(inv)
      .then((r) => after?.(r))
      .catch(P.current.report);
  const execSelect = (inv: Invocation | Invocation[]) =>
    exec(inv, (r) => {
      const ids = G.resultIds(r);
      if (ids.length) P.current.selectIds(ids);
    });
  const layerFor = (): string | null => P.current.activeLayer ?? P.current.doc?.layers[0]?.id ?? null;
  const units = (): string[] => (P.current.doc ? unitsOf(P.current.doc, P.current.selection.objects) : []);
  const selectOnly = (id: string | null) => {
    const d = P.current.doc;
    const layer = id ? (d?.objects.find((o) => o.id === id)?.layer ?? null) : null;
    P.current.setSelection(id ? { objects: [id], layer, stock: false, lastAdded: [id], reference: null } : { objects: [], layer: null, stock: false });
  };
  const clearSnap = () => {
    lastCtx.current?.showSnap(null);
    lastCtx.current?.readout(null);
  };

  // ---- tool changes reset the live state ----
  const tool = p.tool;
  useEffect(() => {
    gest.current = { kind: "none" };
    lastTap.current = null;
    const pk = store.get().pick;
    store.set({ pen: [], rubber: null, poly: null, meas: null, trimHover: null, hover: null, nodeBox: null, pickHover: null });
    if (tool !== "pick" && pk) {
      store.set({ pick: null });
      pk.cancel();
    }
    if (tool !== "node") store.set({ nl: null, nlNote: null, nodeSel: new Set(), preview: null });
    clearSnap();
  }, [tool, store]);

  // ---- node list of the shape being edited ----
  const target = tool === "node" && p.doc && p.selection.objects.length === 1 ? p.selection.objects[0] : null;
  const many = tool === "node" && p.selection.objects.length > 1;
  const rev = p.doc?.revision ?? -1;
  const canNodes = p.has(G.GEO.nodeList);
  useEffect(() => {
    if (tool !== "node") return;
    if (!canNodes) {
      store.set({ nl: null, preview: null, nlNote: t("et.thisEngineHasNoNodeEditing") });
      return;
    }
    if (!target) {
      store.set({ nl: null, preview: null, nlNote: many ? t("et.clickOneShape") : t("et.clickAShapeToEditIts") });
      return;
    }
    let live = true;
    execute(G.nodeList(target))
      .then((r) => {
        if (!live) return;
        const nl = parseNodeList(r.result);
        const sel = store.get().nodeSel;
        const keep = nl ? new Set([...sel].filter((i) => i < nl.nodes.length)) : new Set<number>();
        store.set({ nl, preview: null, nodeSel: keep, nlNote: nl ? null : t("et.noNodes") });
      })
      .catch((e: unknown) => {
        if (live) store.set({ nl: null, preview: null, nlNote: errText(e) });
      });
    return () => {
      live = false;
    };
  }, [tool, target, many, rev, nodeTick, canNodes, store]);
  useEffect(() => store.set({ nodeSel: new Set(), hover: null }), [target, store]);

  /** Run a node edit unless the shape still has to be converted; refetch the nodes afterwards. */
  const nodeEdit = (build: (id: string, nl: NodeList) => Invocation | Invocation[] | string | null, after?: (r: unknown) => void) => {
    const { nl } = store.get();
    if (!nl) return;
    if (nl.converts_on_edit) {
      notify(t("et.convertFirst", { kind: td(`et.kind.${nl.kind}`, SHAPE_WORD[nl.kind] ?? t("et.shape")) }), "error");
      store.set({ preview: null });
      return;
    }
    const inv = build(nl.id, nl);
    if (inv === null) {
      store.set({ preview: null });
      return;
    }
    if (typeof inv === "string") {
      notify(inv, "error");
      store.set({ preview: null });
      return;
    }
    P.current
      .run(inv)
      .then((r) => after?.(r))
      .catch((e: unknown) => {
        store.set({ preview: null });
        P.current.report(e);
      })
      .finally(() => setNodeTick((t) => t + 1));
  };

  // ---- line / pen ----
  const finishPen = (closed: boolean) => {
    const pts = store.get().pen;
    const nodes: PenNode[] = [];
    for (const n of pts) {
      const q = nodes[nodes.length - 1];
      if (q && dist(q.p, n.p) < 1e-6) nodes[nodes.length - 1] = { p: q.p, hin: q.hin, hout: n.hout ?? q.hout };
      else nodes.push(n);
    }
    store.set({ pen: [], rubber: null });
    lastTap.current = null;
    clearSnap();
    if (nodes.length < 2) {
      if (pts.length) notify(t("et.aLineNeedsAtLeastTwo"), "error");
      return;
    }
    const layer = layerFor();
    const curved = penHasCurves(nodes, closed);
    if (!curved) {
      const points = cleanPoints(nodes.map((n) => rp(n.p)));
      if (!P.current.has(G.GEO.line)) {
        notify(t("et.thisEngineCannotDrawLinesYet"), "error");
        return;
      }
      void execSelect(G.line(points, closed && points.length >= 3, layer));
      return;
    }
    if (!P.current.has(G.GEO.path)) {
      notify(t("et.thisEngineCannotAddPaths"), "error");
      return;
    }
    const round = (q: Vec2 | null | undefined) => (q ? rp(q) : q);
    const rounded = nodes.map((n) => ({ p: rp(n.p), hin: round(n.hin), hout: round(n.hout) }));
    void execSelect(G.pathShape(penPathData(rounded, closed), layer, "Path"));
  };

  // ---- polygon ----
  const finishPolygon = (scale: number) => {
    const st = store.get().poly;
    store.set({ poly: null });
    clearSnap();
    if (!st) return;
    const pp = P.current;
    if (!pp.has(G.GEO.polygon)) {
      notify(t("et.thisEngineCannotDrawPolygonsYet"), "error");
      return;
    }
    let { radius, rotationDeg } = polygonFromDrag(st.c, st.p, st.shift);
    if (radius * scale < SLOP) {
      radius = 10;
      rotationDeg = 0;
    }
    const o = polyOpts;
    void execSelect(
      G.polygon({
        sides: o.sides,
        radius_mm: Math.max(roundMm(radius), 0.01),
        center: rp(st.c),
        rotation_deg: r6(rotationDeg),
        star_inner_ratio: o.star ? o.ratio : undefined,
        corner_radius_mm: o.cornerMm > 0 ? o.cornerMm : undefined,
        layer: layerFor(),
        // Engines with Shape Properties keep it a polygon whose sides, star and corners stay editable.
        parametric: pp.has(DCMD.shapeUpdate) ? true : undefined,
      }),
    );
    pp.setTool("select");
  };

  // ---- picks ----
  const startPick = (pk: Omit<Pick, "pts">) => {
    store.set({ pick: { ...pk, pts: [] }, pickHover: null });
    P.current.setTool("pick");
  };

  /** A snapped point for drawing, constrained to 15° steps from `from` while Shift is held (with its guide line). */
  const drawPoint = (f: Vec2, m: Mods, ctx: CanvasCtx, from: Vec2 | null): Vec2 => {
    if (m.shift && from) {
      const q = constrainAngle(from, f);
      ctx.showSnap(null);
      ctx.showConstraint(constraintLine(from, [q[0] - from[0], q[1] - from[1]]));
      return q;
    }
    const sp = ctx.snap(f, m);
    ctx.showSnap(sp.hit);
    return sp.p;
  };

  // ---- canvas tool -----------------------------------------------------------------------------------------------

  const down = (f: Vec2, m: Mods, info: { target: Element | null; screen: Vec2; time: number }, ctx: CanvasCtx): boolean => {
    lastCtx.current = ctx;
    cursor.current = f;
    const pp = P.current;
    if (!pp.design) return false;
    // Guides move with the Select tool only; in the drawing tools they are snap targets to click on.
    const gEl = pp.tool === "select" ? info.target?.closest?.("[data-guide]") : null;
    if (gEl) {
      const id = gEl.getAttribute("data-guide");
      const gd = guidesRef.current.find((x) => x.id === id);
      if (gd) {
        const lt = lastGuideTap.current;
        lastGuideTap.current = { id: gd.id, t: info.time };
        if (lt && lt.id === gd.id && info.time - lt.t < DBL_MS) {
          // Double-click: type the guide's position.
          lastGuideTap.current = null;
          gest.current = { kind: "none" };
          setGuideDlg({ id: gd.id, axis: gd.axis, v: gd.v });
          return true;
        }
        gest.current = { kind: "guide", id: gd.id, axis: gd.axis };
        store.set({ guideDrag: { id: gd.id, axis: gd.axis, v: gd.v, remove: false } });
        return true;
      }
    }
    const st = store.get();
    switch (pp.tool) {
      case "node": {
        const nl = st.nl;
        if (!nl) {
          selectOnly(ctx.hitObject(f));
          return true;
        }
        const tol = (NODE_TOL_PX * handleScale()) / ctx.scale;
        const h = hitNodes(nl, f, tol, handlesShown(nl, st.nodeSel));
        if (h?.kind === "node") {
          const sel = clickNode(st.nodeSel, h.index, m.shift);
          store.set({ nodeSel: sel });
          if (!sel.has(h.index)) {
            gest.current = { kind: "none" };
            return true;
          }
          gest.current = { kind: "nodes", start: f, base: nl.nodes[h.index].point, indices: [...sel], startScreen: info.screen, moved: false, d: [0, 0], tapped: h.index };
        } else if (h?.kind === "handle") {
          gest.current = { kind: "handle", index: h.index, side: h.side, startScreen: info.screen, moved: false, to: f };
        } else if (h?.kind === "segment") {
          gest.current = { kind: "segment", seg: h.index, t: h.t, start: f, startScreen: info.screen, moved: false, d: [0, 0] };
        } else {
          const other = ctx.hitObject(f);
          if (other && other !== nl.id) {
            selectOnly(other);
            gest.current = { kind: "none" };
            return true;
          }
          gest.current = { kind: "nbox", a: f, startScreen: info.screen, moved: false, shift: m.shift };
        }
        return true;
      }
      case "line":
      case "pen": {
        const pts = st.pen;
        const last = pts[pts.length - 1]?.p ?? null;
        const q = drawPoint(f, m, ctx, last);
        const lt = lastTap.current;
        lastTap.current = { t: info.time, s: info.screen };
        if (lt && info.time - lt.t < DBL_MS && dist(lt.s, info.screen) < 6 && pts.length >= 2) {
          finishPen(false);
          gest.current = { kind: "none" };
          return true;
        }
        if (pts.length >= (pp.tool === "line" ? 3 : 2) && dist(ctx.toScreen(pts[0].p), ctx.toScreen(q)) <= CLOSE_PX) {
          finishPen(true);
          gest.current = { kind: "none" };
          return true;
        }
        store.set({ pen: [...pts, { p: q }], rubber: q });
        gest.current = pp.tool === "pen" ? { kind: "pen", p: q, startScreen: info.screen, moved: false } : { kind: "none" };
        return true;
      }
      case "polygon": {
        const c = drawPoint(f, m, ctx, null);
        store.set({ poly: { c, p: c, shift: m.shift } });
        gest.current = { kind: "poly", c };
        return true;
      }
      case "measure": {
        const prev = st.meas;
        if (prev && !prev.done) {
          const b = drawPoint(f, m, ctx, prev.a);
          store.set({ meas: { a: prev.a, b, done: true } });
          ctx.readout(measureText(measure(prev.a, b), lu()));
          gest.current = { kind: "none" };
          return true;
        }
        const a = drawPoint(f, m, ctx, null);
        store.set({ meas: { a, b: a, done: false } });
        gest.current = { kind: "measure", startScreen: info.screen, moved: false };
        return true;
      }
      case "trim": {
        const id = ctx.hitObject(f);
        gest.current = { kind: "none" };
        if (!id) return true;
        if (!pp.has(G.GEO.trim)) {
          notify(t("et.thisEngineCannotTrimYet"), "error");
          return true;
        }
        void exec(G.trim(id, rp(f), r6(Math.max(0.05, 10 / ctx.scale))));
        return true;
      }
      case "pick": {
        const pk = st.pick;
        gest.current = { kind: "none" };
        if (!pk) return true;
        const q = drawPoint(f, m, ctx, pk.pts[pk.pts.length - 1] ?? null);
        const pts = [...pk.pts, q];
        if (pts.length >= pk.labels.length) {
          store.set({ pick: null, pickHover: null });
          clearSnap();
          pp.setTool("select");
          pk.done(pts);
        } else store.set({ pick: { ...pk, pts } });
        return true;
      }
      default:
        return false;
    }
  };

  const drag = (f: Vec2, m: Mods, ctx: CanvasCtx) => {
    lastCtx.current = ctx;
    cursor.current = f;
    const g = gest.current;
    const s = ctx.toScreen(f);
    const st = store.get();
    switch (g.kind) {
      case "guide": {
        const remove = (g.axis === "x" ? s[0] : s[1]) < RULER_PX;
        const sp = ctx.snap(f, m);
        ctx.showSnap(remove ? null : sp.hit);
        const v = roundMm(g.axis === "x" ? sp.p[0] : sp.p[1]);
        store.set({ guideDrag: { id: g.id, axis: g.axis, v, remove } });
        ctx.readout(remove ? t("et.releaseRemove") : guideLabel({ axis: g.axis, v }));
        break;
      }
      case "nodes": {
        if (!st.nl) return;
        if (!g.moved && dist(s, g.startScreen) < SLOP) return;
        g.moved = true;
        const raw: Vec2 = [f[0] - g.start[0], f[1] - g.start[1]];
        let d = raw;
        if (m.shift) {
          d = constrainDelta(raw);
          ctx.showSnap(null);
          ctx.showConstraint(constraintLine(g.base, d));
        } else {
          const sp = ctx.snap([g.base[0] + raw[0], g.base[1] + raw[1]], m);
          ctx.showSnap(sp.hit);
          d = [sp.p[0] - g.base[0], sp.p[1] - g.base[1]];
        }
        g.d = d;
        store.set({ preview: moveNodes(st.nl, new Set(g.indices), d) });
        ctx.readout(`Δ ${fixedNumber(d[0], lu())}, ${fixedLength(d[1], lu())} · ${g.indices.length} node${g.indices.length === 1 ? "" : "s"}`);
        break;
      }
      case "handle": {
        const nl = st.nl;
        if (!nl) return;
        if (!g.moved && dist(s, g.startScreen) < SLOP) return;
        g.moved = true;
        const node = nl.nodes[g.index];
        const to = drawPoint(f, m, ctx, node?.point ?? null);
        g.to = to;
        store.set({ preview: moveHandle(nl, g.index, g.side, to) });
        if (node) ctx.readout(`Handle ${fixedLength(dist(node.point, to), lu())} · ${measure(node.point, to).angleDeg.toFixed(1)}°`);
        break;
      }
      case "segment": {
        const nl = st.nl;
        const seg = nl?.segments[g.seg];
        if (!nl || !seg) return;
        if (!g.moved && dist(s, g.startScreen) < SLOP) return;
        g.moved = true;
        let d: Vec2 = [f[0] - g.start[0], f[1] - g.start[1]];
        if (m.shift) d = constrainDelta(d);
        ctx.showConstraint(m.shift ? constraintLine(g.start, d) : null);
        g.d = d;
        store.set({ preview: moveNodes(nl, new Set([seg.from, seg.to]), d) });
        ctx.readout(`Δ ${fixedNumber(d[0], lu())}, ${fixedLength(d[1], lu())} · segment`);
        break;
      }
      case "nbox": {
        if (!g.moved && dist(s, g.startScreen) < SLOP) return;
        g.moved = true;
        store.set({ nodeBox: { a: g.a, b: f } });
        break;
      }
      case "pen": {
        if (!g.moved && dist(s, g.startScreen) < SLOP) return;
        g.moved = true;
        const handle = m.shift ? constrainAngle(g.p, f) : f;
        ctx.showConstraint(m.shift ? constraintLine(g.p, [handle[0] - g.p[0], handle[1] - g.p[1]]) : null);
        const pts = st.pen.slice();
        if (pts.length) pts[pts.length - 1] = penNode(g.p, handle);
        store.set({ pen: pts, rubber: null });
        ctx.readout(`Handle ${fixedLength(dist(g.p, handle), lu())}`);
        break;
      }
      case "poly": {
        const q = drawPoint(f, { ...m, shift: false }, ctx, null);
        if (m.shift) {
          // Shift turns the polygon in 15° steps: the guide runs through the centre and the first corner.
          const c = constrainAngle(g.c, q);
          ctx.showConstraint(constraintLine(g.c, [c[0] - g.c[0], c[1] - g.c[1]]));
        }
        store.set({ poly: { c: g.c, p: q, shift: m.shift } });
        const r = polygonFromDrag(g.c, q, m.shift);
        ctx.readout(`R ${fixedLength(r.radius, lu())} · ${r.rotationDeg.toFixed(1)}°`);
        break;
      }
      case "measure": {
        const prev = st.meas;
        if (!prev) return;
        if (!g.moved && dist(s, g.startScreen) < SLOP) return;
        g.moved = true;
        const b = drawPoint(f, m, ctx, prev.a);
        store.set({ meas: { a: prev.a, b, done: false } });
        ctx.readout(measureText(measure(prev.a, b), lu()));
        break;
      }
    }
  };

  const up = (f: Vec2, m: Mods, ctx: CanvasCtx) => {
    lastCtx.current = ctx;
    const g = gest.current;
    gest.current = { kind: "none" };
    const st = store.get();
    switch (g.kind) {
      case "guide": {
        const gd = st.guideDrag;
        store.set({ guideDrag: null });
        clearSnap();
        if (!gd || !g.id) return;
        const before = guidesRef.current.find((x) => x.id === g.id);
        if (gd.remove) guideOps.remove(g.id);
        else if (before && Math.abs(before.v - gd.v) > 1e-9) guideOps.move(g.id, gd.v);
        return;
      }
      case "nodes": {
        clearSnap();
        if (!g.moved) {
          if (!m.shift && st.nodeSel.size > 1) store.set({ nodeSel: new Set([g.tapped]) });
          return;
        }
        const d = g.d;
        if (Math.abs(d[0]) < 1e-9 && Math.abs(d[1]) < 1e-9) {
          store.set({ preview: null });
          return;
        }
        nodeEdit((id, nl) => {
          const invs = g.indices.filter((i) => nl.nodes[i]).map((i) => G.nodeMove(id, i, rp([nl.nodes[i].point[0] + d[0], nl.nodes[i].point[1] + d[1]])));
          return invs.length === 0 ? null : invs.length === 1 ? invs[0] : invs;
        });
        return;
      }
      case "handle": {
        clearSnap();
        if (!g.moved) return;
        nodeEdit((id) => G.nodeMove(id, g.index, rp(g.to), g.side));
        return;
      }
      case "segment": {
        clearSnap();
        if (!g.moved) {
          // A click on a segment adds a node there (LightBurn: I / double-click inserts).
          nodeEdit(
            (id) => G.nodeInsert(id, g.seg, Math.min(0.999999, Math.max(0.000001, r6(g.t)))),
            (r) => {
              const i = (r as { index?: unknown } | null)?.index;
              if (typeof i === "number") store.set({ nodeSel: new Set([i]) });
            },
          );
          return;
        }
        const d = g.d;
        nodeEdit((id, nl) => {
          const seg = nl.segments[g.seg];
          if (!seg) return null;
          const idx = [...new Set([seg.from, seg.to])];
          const invs = idx.map((i) => G.nodeMove(id, i, rp([nl.nodes[i].point[0] + d[0], nl.nodes[i].point[1] + d[1]])));
          return invs.length === 1 ? invs[0] : invs;
        });
        return;
      }
      case "nbox": {
        store.set({ nodeBox: null });
        if (!st.nl) return;
        if (!g.moved) {
          if (!g.shift) store.set({ nodeSel: new Set() });
          return;
        }
        const inside = nodesInBox(st.nl, boxFromPoints(g.a, f));
        store.set({ nodeSel: new Set(g.shift ? [...st.nodeSel, ...inside] : inside) });
        return;
      }
      case "pen":
        if (g.moved) store.set({ rubber: null });
        return;
      case "poly":
        finishPolygon(ctx.scale);
        return;
      case "measure": {
        if (g.moved && st.meas) store.set({ meas: { ...st.meas, done: true } });
        return;
      }
    }
  };

  const cancel = () => {
    const g = gest.current;
    gest.current = { kind: "none" };
    if (g.kind === "pen") {
      const pts = store.get().pen;
      store.set({ pen: pts.slice(0, -1) });
    }
    store.set({ preview: null, nodeBox: null, guideDrag: null, poly: null });
    clearSnap();
  };

  const hover = (f: Vec2, m: Mods, ctx: CanvasCtx): boolean => {
    lastCtx.current = ctx;
    cursor.current = f;
    const pp = P.current;
    if (!pp.design) return false;
    const st = store.get();
    switch (pp.tool) {
      case "node": {
        ctx.showSnap(null);
        const nl = st.nl;
        const h = nl ? hitNodes(nl, f, (NODE_TOL_PX * handleScale()) / ctx.scale, handlesShown(nl, st.nodeSel)) : null;
        if (!sameHit(h, st.hover)) store.set({ hover: h });
        return true;
      }
      case "line":
      case "pen": {
        const last = st.pen[st.pen.length - 1]?.p ?? null;
        const q = drawPoint(f, m, ctx, last);
        if (last) {
          store.set({ rubber: q });
          const ms = measure(last, q);
          ctx.readout(`${fixedLength(ms.dist, lu())} · ${ms.angleDeg.toFixed(1)}°`);
        }
        return true;
      }
      case "polygon":
        drawPoint(f, m, ctx, null);
        return true;
      case "measure": {
        const prev = st.meas;
        if (prev && !prev.done) {
          const b = drawPoint(f, m, ctx, prev.a);
          store.set({ meas: { a: prev.a, b, done: false } });
          ctx.readout(measureText(measure(prev.a, b), lu()));
        } else drawPoint(f, m, ctx, null);
        return true;
      }
      case "trim": {
        ctx.showSnap(null);
        const id = ctx.hitObject(f);
        if (id !== st.trimHover) store.set({ trimHover: id });
        return true;
      }
      case "pick": {
        const pk = st.pick;
        const q = drawPoint(f, m, ctx, pk?.pts[pk.pts.length - 1] ?? null);
        store.set({ pickHover: q });
        return true;
      }
      default:
        return false;
    }
  };

  // ---- guides dragged out of the rulers ----
  const rulerDrag = (axis: "x" | "y", screen: Vec2, r: RenderCtx, phase: "move" | "end" | "cancel") => {
    if (phase === "cancel") {
      store.set({ guideDrag: null });
      return;
    }
    const f = r.toField(screen);
    const remove = (axis === "x" ? screen[0] : screen[1]) < RULER_PX;
    let v = axis === "x" ? f[0] : f[1];
    // Snap with the render context (the document as drawn now): lastCtx is from the last canvas press and goes
    // stale when the document changes without one (undo, menus, other clients).
    const snapFn = r.snap ?? lastCtx.current?.snap;
    if (snapFn && !remove) {
      const sp = snapFn(f, { shift: false, ctrl: false });
      v = axis === "x" ? sp.p[0] : sp.p[1];
    }
    v = roundMm(v);
    if (phase === "move") {
      store.set({ guideDrag: { id: null, axis, v, remove } });
      return;
    }
    store.set({ guideDrag: null });
    if (!remove) guideOps.add(axis, v);
  };

  // ---- actions (menus and hotkeys) ---------------------------------------------------------------------------------

  const need = (n: number, what: string): string[] | null => {
    const u = units();
    if (u.length < n) {
      notify(n === 1 ? t("et.selectOne", { what: doWhat(what) }) : t("et.selectN", { n, what: doWhat(what) }), "error");
      return null;
    }
    return u;
  };
  const cmd = (id: string, what: string): boolean => {
    if (P.current.has(id)) return true;
    notify(t("et.cannot", { what: doWhat(what) }), "error");
    return false;
  };
  const act = {
    boolean: (op: G.BooleanOp) => {
      const ids = need(2, op === "xor" ? "combine (XOR)" : op);
      if (ids && cmd(G.GEO.boolean, "combine shapes")) void execSelect(G.pathBoolean(ids, op));
    },
    weld: () => {
      const ids = need(1, "weld");
      if (ids && cmd(G.GEO.weld, "weld")) void execSelect(G.weld(ids));
    },
    cutShapes: () => {
      const ids = need(2, "cut (the last one selected cuts the others)");
      if (ids && cmd(G.GEO.cutShapes, "cut shapes")) void execSelect(G.cutShapes(ids));
    },
    breakApart: () => {
      const ids = need(1, "break apart");
      if (ids && cmd(G.GEO.breakApart, "break shapes apart")) void execSelect(G.breakApart(ids));
    },
    autoJoin: () => {
      const ids = need(1, "join");
      if (ids && cmd(G.GEO.autoJoin, "auto-join")) void execSelect(G.autoJoin(ids));
    },
    close: () => {
      const ids = need(1, "close");
      if (ids && cmd(G.GEO.close, "close paths")) void execSelect(G.closePaths(ids));
    },
    deleteDuplicates: () => {
      if (!cmd(G.GEO.deleteDuplicates, "delete duplicates")) return;
      const ids = units();
      void exec(G.deleteDuplicates(ids.length ? ids : null), (r) => {
        const n = Array.isArray((r as { deleted?: unknown } | null)?.deleted) ? (r as { deleted: unknown[] }).deleted.length : null;
        notify(n === null ? t("et.dupsDeleted") : t(ids.length ? (n === 1 ? "et.dupDeletedSel1" : "et.dupDeletedSelN") : n === 1 ? "et.dupDeleted1" : "et.dupDeletedN", { n }));
      });
    },
    convert: () => {
      const ids = need(1, "convert to a path");
      if (ids && cmd(G.GEO.convert, "convert to paths")) void execSelect(G.convertToPath(ids));
    },
    reverse: () => {
      const ids = need(1, "reverse");
      if (ids && cmd(G.GEO.reverse, "reverse paths")) void exec(G.reverse(ids), () => notify(t("et.directionReversed")));
    },
    order: (to: G.OrderTo) => {
      const ids = need(1, "reorder");
      if (ids && cmd(G.GEO.order, "change the draw order")) void exec(G.order(ids, to));
    },
    lock: (locked: boolean) => {
      const ids = need(1, locked ? "lock" : "unlock");
      if (ids && cmd(G.GEO.lock, "lock shapes")) void exec(G.lock(ids, locked), () => notify(locked ? t("et.lockedTheShapesCannotBeMoved") : "Unlocked."));
    },
    makeSame: (dim: "width" | "height" | "both") => {
      const ids = need(2, "match (the last one selected is the reference)");
      if (ids && cmd(G.GEO.makeSame, "match sizes")) void exec(G.makeSame(ids, dim));
    },
    dialog: (d: EditDialog, n: number, what: string, id: string) => {
      if (n > 0 && !need(n, what)) return;
      if (cmd(id, what)) setDialog(d);
    },
    nodeTool: () => P.current.setTool(P.current.tool === "node" ? "select" : "node"),
    pick: (kind: PickKind, values?: CircularValues) => startPickFor(kind, values),
  };

  const startPickFor = (kind: PickKind, values?: CircularValues) => {
    const ids = units();
    if (kind === "circularCenter") {
      const v = values as CircularValues;
      startPick({
        title: t("et.circularArrayCentre"),
        labels: [t("et.pickCircleCentre")],
        preview: "point",
        done: (pts) => setDialog({ kind: "circular", values: { ...v, cx: String(roundMm(pts[0][0])), cy: String(roundMm(pts[0][1])) } }),
        cancel: () => setDialog({ kind: "circular", values: v }),
      });
      return;
    }
    if (!ids.length) {
      notify(t("et.selectTheShapesFirst"), "error");
      return;
    }
    if (kind === "unfillet") {
      // LightBurn's Radius tool on a rounded corner: click corners of the one selected shape until Esc.
      if (!cmd(G.GEO.unfillet, "make corners sharp")) return;
      if (ids.length !== 1) return void notify(t("et.unfilletOneShape"), "error");
      const again = () =>
        startPick({
          title: t("et.unfilletTitle"),
          labels: [t("et.unfilletPick")],
          preview: "point",
          done: (pts) =>
            void P.current
              .run(G.unfillet(ids, rp(pts[0]), 2))
              .then(() => again())
              .catch((e: unknown) => {
                P.current.report(e);
                again();
              }),
          cancel: () => {},
        });
      again();
      return;
    }
    if (kind === "mirror" || kind === "mirrorCopy") {
      if (!cmd(G.GEO.transform, "mirror")) return;
      const keep = kind === "mirrorCopy";
      startPick({
        title: keep ? t("et.mirrorKeepTitle") : t("et.mirrorTitle"),
        labels: [t("et.pickMirror1"), t("et.pickMirror2")],
        preview: "line",
        done: (pts) => {
          if (dist(pts[0], pts[1]) < 1e-6) return notify(t("et.pickTwoDifferentPoints"), "error");
          const inv = G.transformEx(ids, { mirror_line: { a: rp(pts[0]), b: rp(pts[1]) }, ...(keep ? { keep_copy: true } : {}) });
          void (keep ? execSelect(inv) : exec(inv));
        },
        cancel: () => {},
      });
      return;
    }
    const scale = kind === "scale2" || kind === "scale2rot";
    if (!cmd(scale ? G.GEO.scaleTo : G.GEO.rotateTo, scale ? "two-point scale" : "two-point rotate")) return;
    startPick({
      title: scale ? t("et.twoPointScale") : t("et.twoPointRotate"),
      labels: [t("et.pickPivot"), t("et.pickRef"), scale ? t("et.pickScaleTo") : t("et.pickRotateTo")],
      preview: "two-point",
      done: ([pv, from, to]) => {
        if (dist(pv, from) < 1e-6 || dist(pv, to) < 1e-6) return notify(t("et.theReferenceAndTargetMustDiffer"), "error");
        void exec(scale ? G.scaleTo(ids, rp(pv), rp(from), rp(to), kind === "scale2rot") : G.rotateTo(ids, rp(pv), rp(from), rp(to)));
      },
      cancel: () => {},
    });
  };

  const nodeSelNow = (): { id: string; corners: number[] } | null => {
    const st = store.get();
    return P.current.tool === "node" && st.nl && st.nodeSel.size ? { id: st.nl.id, corners: [...st.nodeSel].sort((a, b) => a - b) } : null;
  };

  // ---- node-mode keys ----
  const nodeKey = (e: KeyboardEvent, atPointer = true): boolean => {
    const st = store.get();
    const nl = st.nl;
    if (!nl) return false;
    const k = e.key.toLowerCase();
    // Keys act on what is under the pointer (LightBurn); the bar's buttons act on the selection only.
    const hov = atPointer ? st.hover : null;
    const sel = st.nodeSel;
    const hovNode = hov?.kind === "node" ? hov.index : null;
    const hovSeg = hov?.kind === "segment" ? hov : null;
    const nodes = (): number[] => (hovNode !== null ? [hovNode] : [...sel]);
    const mod = e.ctrlKey || e.metaKey;
    if (mod && !e.altKey && !e.shiftKey && k === "a") {
      store.set({ nodeSel: new Set(nl.nodes.map((n) => n.index)) });
      return true;
    }
    if (mod || e.altKey) return false;
    switch (e.key === "Delete" || e.key === "Backspace" ? "delete" : k) {
      case "escape":
        if (!sel.size) return false;
        store.set({ nodeSel: new Set() });
        return true;
      case "delete":
      case "d": {
        const idx = k === "d" ? nodes() : sel.size ? [...sel] : hovNode !== null ? [hovNode] : [];
        if (!idx.length) {
          notify(hovSeg ? t("et.pointNodeDelete") : t("et.selectNodesDelete"), "error");
          return true;
        }
        nodeEdit((id) => G.nodeDelete(id, idx), () => store.set({ nodeSel: new Set() }));
        return true;
      }
      case "b": {
        const i = hovNode ?? (sel.size === 1 ? [...sel][0] : null);
        if (i === null) notify(t("et.pointAtOrSelectOneNode"), "error");
        else nodeEdit((id) => G.nodeBreak(id, i), () => store.set({ nodeSel: new Set() }));
        return true;
      }
      case "j": {
        const pair = joinPair(nl, sel);
        if (typeof pair === "string") notify(pair, "error");
        else nodeEdit((id) => G.nodeJoin(id, pair[0], pair[1]), (r) => {
          const i = (r as { index?: unknown } | null)?.index;
          store.set({ nodeSel: new Set(typeof i === "number" ? [i] : []) });
        });
        return true;
      }
      case "s":
      case "c": {
        if (k === "s" && hovSeg) {
          nodeEdit((id) => G.nodeSegmentType(id, hovSeg.index, "curve"));
          return true;
        }
        const idx = nodes();
        if (!idx.length) notify(t("et.pointAtOrSelectNodesFirst"), "error");
        else nodeEdit((id) => G.nodeSetType(id, idx, k === "s" ? "smooth" : "corner"));
        return true;
      }
      case "l":
      case "k": {
        const segs = hovSeg ? [hovSeg.index] : segmentsBetween(nl, sel);
        if (!segs.length) notify(t("et.pointAtASegmentOrSelect"), "error");
        else nodeEdit((id) => {
          const invs = segs.map((s) => G.nodeSegmentType(id, s, k === "l" ? "line" : "curve"));
          return invs.length === 1 ? invs[0] : invs;
        });
        return true;
      }
      case "i":
      case "m": {
        if (!hovSeg) notify(t("et.pointAtASegmentToAdd"), "error");
        else
          nodeEdit(
            (id) => G.nodeInsert(id, hovSeg.index, k === "m" ? 0.5 : Math.min(0.999999, Math.max(0.000001, r6(hovSeg.t)))),
            (r) => {
              const i = (r as { index?: unknown } | null)?.index;
              if (typeof i === "number") store.set({ nodeSel: new Set([i]) });
            },
          );
        return true;
      }
      case "t": {
        const at = cursor.current;
        if (!at) return true;
        if (cmd(G.GEO.trim, "trim")) nodeEdit((id) => G.trim(id, rp(at), r6(Math.max(0.05, 10 / (lastCtx.current?.scale ?? 4)))));
        return true;
      }
      case "e": {
        // Extend the open end nearest the pointer straight on to the nearest line (LightBurn's E).
        const at = cursor.current;
        if (!at) return true;
        if (cmd(DCMD.extend, "extend lines")) nodeEdit((id) => extendCmd(id, rp(at), r6(Math.max(0.5, 12 / (lastCtx.current?.scale ?? 4)))));
        return true;
      }
    }
    return false;
  };

  // ---- hotkeys (capture phase, ahead of the app's own) ----
  const keyRef = useRef<(e: KeyboardEvent) => void>(() => {});
  keyRef.current = (e: KeyboardEvent) => {
    const pp = P.current;
    const stop = () => {
      e.preventDefault();
      e.stopImmediatePropagation();
    };
    if (dialog || guideDlg) {
      // Our dialog is modal: Esc closes it, and keys outside it never reach the app's shortcuts.
      if (e.key === "Escape") {
        setDialog(null);
        setGuideDlg(null);
        stop();
      } else if (!(e.target as Element | null)?.closest?.(".modal")) e.stopImmediatePropagation();
      return;
    }
    if (!pp.design || pp.blocked || typingIn(e.target) || e.repeat) return;
    const mod = e.ctrlKey || e.metaKey;
    const code = e.code;
    const k = e.key.toLowerCase();
    const st = store.get();
    // Tool-specific keys first.
    if (pp.tool === "node" && canvasFocused() && nodeKey(e)) return stop();
    if ((pp.tool === "line" || pp.tool === "pen") && !mod && !e.altKey) {
      const pts = st.pen;
      if (e.key === "Enter" && pts.length) return stop(), finishPen(false);
      if (e.key === "Escape" && pts.length) return stop(), store.set({ pen: [], rubber: null }), clearSnap();
      if (e.key === "Backspace" && pts.length) return stop(), store.set({ pen: pts.slice(0, -1) });
      if (k === "c" && pts.length) return stop(), finishPen(true);
    }
    if (pp.tool === "measure" && e.key === "Escape" && st.meas) return stop(), store.set({ meas: null }), clearSnap();
    // Global editing hotkeys (LightBurn defaults).
    if (mod && !e.altKey && !e.shiftKey) {
      if (code === "Backquote" || e.key === "`") return stop(), act.nodeTool();
      if (k === "l") return stop(), pp.setTool(pp.tool === "line" ? "select" : "line");
      if (k === "2" || code === "Digit2") return stop(), act.pick("rotate2");
      if (k === "w") return stop(), act.weld();
      if (e.key === "PageUp") return stop(), act.order("front");
      if (e.key === "PageDown") return stop(), act.order("back");
    }
    if (mod && e.shiftKey && !e.altKey && code === "KeyC") return stop(), act.convert();
    if (e.altKey && !mod) {
      if (e.shiftKey && code === "KeyO") return stop(), act.dialog({ kind: "optimize" }, 1, "optimize", G.GEO.optimize);
      if (code === "NumpadAdd" || e.key === "+" || (code === "Equal" && e.shiftKey)) return stop(), act.boolean("union");
      if (code === "NumpadSubtract" || (code === "Minus" && !e.shiftKey)) return stop(), act.boolean("subtract");
      if (code === "NumpadMultiply" || e.key === "*" || (code === "Digit8" && e.shiftKey)) return stop(), act.boolean("intersect");
      if (e.shiftKey) return;
      switch (code) {
        case "KeyB":
          return stop(), act.breakApart();
        case "KeyJ":
          return stop(), act.autoJoin();
        case "KeyD":
          return stop(), act.deleteDuplicates();
        case "KeyC":
          return stop(), act.cutShapes();
        case "KeyO":
          return stop(), act.dialog({ kind: "offset" }, 1, "offset", G.GEO.offset);
      }
    }
    if (!mod && !e.altKey && !e.shiftKey && canvasFocused() && pp.selection.objects.length) {
      if (e.key === "PageUp") return stop(), act.order("forward");
      if (e.key === "PageDown") return stop(), act.order("backward");
    }
  };
  useEffect(() => {
    const on = (e: KeyboardEvent) => keyRef.current(e);
    window.addEventListener("keydown", on, true);
    return () => window.removeEventListener("keydown", on, true);
  }, []);

  // ---- menus ----
  const doc = p.doc;
  const u = doc ? unitsOf(doc, p.selection.objects) : [];
  const n = u.length;
  const has = p.has;
  const lockedList = (doc as unknown as { locked?: unknown } | null)?.locked;
  const lockedIds = new Set(Array.isArray(lockedList) ? lockedList.filter((x): x is string => typeof x === "string") : []);
  const anyLocked = p.selection.objects.some((id) => lockedIds.has(id));
  const allLocked = p.selection.objects.length > 0 && p.selection.objects.every((id) => lockedIds.has(id));
  const modifyItems: MenuItem[] = [
    { label: t("et.weld"), hint: "Ctrl+W", disabled: n < 1 || !has(G.GEO.weld), action: act.weld },
    { label: t("et.union"), hint: "Alt++", disabled: n < 2 || !has(G.GEO.boolean), action: () => act.boolean("union") },
    { label: t("et.subtractLastSelected"), hint: "Alt+-", disabled: n < 2 || !has(G.GEO.boolean), action: () => act.boolean("subtract") },
    { label: t("et.intersect"), hint: "Alt+*", disabled: n < 2 || !has(G.GEO.boolean), action: () => act.boolean("intersect") },
    { label: t("et.exclusiveOrXor"), disabled: n < 2 || !has(G.GEO.boolean), action: () => act.boolean("xor") },
    { label: t("et.cutShapesLastSelectedCuts"), hint: "Alt+C", disabled: n < 2 || !has(G.GEO.cutShapes), action: act.cutShapes },
    { sep: true },
    { label: t("et.offsetShapes"), hint: "Alt+O", disabled: n < 1 || !has(G.GEO.offset), action: () => act.dialog({ kind: "offset" }, 1, "offset", G.GEO.offset) },
    {
      label: t("et.filletCorners"),
      disabled: n < 1 || !has(G.GEO.fillet),
      action: () => {
        const ns = nodeSelNow();
        act.dialog({ kind: "fillet", nodeId: ns?.id ?? null, corners: ns?.corners ?? [] }, 1, "fillet", G.GEO.fillet);
      },
    },
    { sep: true },
    { label: t("et.breakApart"), hint: "Alt+B", disabled: n < 1 || !has(G.GEO.breakApart), action: act.breakApart },
    { label: t("et.autoJoin"), hint: "Alt+J", disabled: n < 1 || !has(G.GEO.autoJoin), action: act.autoJoin },
    { label: t("et.autoJoinWithTolerance"), disabled: n < 1 || !has(G.GEO.autoJoin), action: () => act.dialog({ kind: "autojoin" }, 1, "join", G.GEO.autoJoin) },
    { label: t("et.closePath"), disabled: n < 1 || !has(G.GEO.close), action: act.close },
    { label: t("et.closePathsWithTolerance"), disabled: n < 1 || !has(G.GEO.close), action: () => act.dialog({ kind: "close" }, 1, "close", G.GEO.close) },
    { label: t("et.deleteDuplicates"), hint: "Alt+D", disabled: !doc?.objects.length || !has(G.GEO.deleteDuplicates), action: act.deleteDuplicates },
    { label: t("et.convertToPath"), hint: "Ctrl+Shift+C", disabled: n < 1 || !has(G.GEO.convert), action: act.convert },
    { label: t("et.optimizeShapes"), hint: "Alt+Shift+O", disabled: n < 1 || !has(G.GEO.optimize), action: () => act.dialog({ kind: "optimize" }, 1, "optimize", G.GEO.optimize) },
    { label: t("et.reverseDirection"), disabled: n < 1 || !has(G.GEO.reverse), action: act.reverse },
    { sep: true },
    { label: tool === "node" ? t("et.stopNodes") : t("et.editNodes"), hint: "Ctrl+`", disabled: !has(G.GEO.nodeList), action: act.nodeTool },
    { label: t("et.trimClickPieces"), disabled: !has(G.GEO.trim), action: () => p.setTool("trim") },
  ];
  const arrangeItems: MenuItem[] = [
    { label: t("et.bringToFront"), hint: "Ctrl+PgUp", disabled: n < 1 || !has(G.GEO.order), action: () => act.order("front") },
    { label: t("et.bringForward"), hint: "PgUp", disabled: n < 1 || !has(G.GEO.order), action: () => act.order("forward") },
    { label: t("et.sendBackward"), hint: "PgDn", disabled: n < 1 || !has(G.GEO.order), action: () => act.order("backward") },
    { label: t("et.sendToBack"), hint: "Ctrl+PgDn", disabled: n < 1 || !has(G.GEO.order), action: () => act.order("back") },
    { sep: true },
    { label: t("et.lock"), disabled: n < 1 || allLocked || !has(G.GEO.lock), action: () => act.lock(true) },
    { label: t("et.unlock"), disabled: !anyLocked || !has(G.GEO.lock), action: () => act.lock(false) },
    { sep: true },
    { label: t("et.makeSameWidth"), disabled: n < 2 || !has(G.GEO.makeSame), action: () => act.makeSame("width") },
    { label: t("et.makeSameHeight"), disabled: n < 2 || !has(G.GEO.makeSame), action: () => act.makeSame("height") },
    { label: t("et.makeSameSize"), disabled: n < 2 || !has(G.GEO.makeSame), action: () => act.makeSame("both") },
    { sep: true },
    { label: t("et.transform"), disabled: n < 1 || !has(G.GEO.transform), action: () => act.dialog({ kind: "transform" }, 1, "transform", G.GEO.transform) },
    { label: t("et.twoPointRotate"), hint: "Ctrl+2", disabled: n < 1 || !has(G.GEO.rotateTo), action: () => act.pick("rotate2") },
    { label: t("et.twoPointScale"), disabled: n < 1 || !has(G.GEO.scaleTo), action: () => act.pick("scale2") },
    // Beginner Mode hides Mirror Across Line, as LightBurn's does.
    ...(beginnerMode
      ? []
      : [
          { label: t("et.mirrorAcrossLine"), disabled: n < 1 || !has(G.GEO.transform), action: () => act.pick("mirror") },
          { label: t("et.mirrorAcrossLineKeepOriginal"), disabled: n < 1 || !has(G.GEO.transform), action: () => act.pick("mirrorCopy") },
        ]),
    { sep: true },
    { label: t("et.gridArray"), disabled: n < 1, action: p.onGridArray },
    {
      label: t("et.circularArray"),
      disabled: n < 1 || !has(G.GEO.circularArray),
      action: () => act.dialog({ kind: "circular", values: null }, 1, "copy in a circle", G.GEO.circularArray),
    },
    { label: t("et.copyAlongPathPathSelectedLast"), disabled: n < 2 || !has(G.GEO.alongPath), action: () => act.dialog({ kind: "along" }, 2, "copy along a path", G.GEO.alongPath) },
    { label: t("et.quickNest"), disabled: n < 1 || !has(G.GEO.nest), action: () => act.dialog({ kind: "nest" }, 1, "nest", G.GEO.nest) },
    ...(p.arrangeExtra ?? []),
    { sep: true },
    { label: t("et.addGuide"), action: () => setGuideDlg({ id: null, axis: "x", v: 0 }) },
    { label: guides.length ? t("et.clearGuidesN", { n: guides.length }) : t("et.clearGuides"), disabled: !guides.length, action: guideOps.clear },
  ];

  const menuItems: MenuItem[] = [
    { label: t("et.editNodes"), hint: "Ctrl+`", disabled: n !== 1 || !has(G.GEO.nodeList), action: () => p.setTool("node") },
    { label: t("et.weld"), hint: "Ctrl+W", disabled: n < 1 || !has(G.GEO.weld), action: act.weld },
    { label: t("et.breakApart"), hint: "Alt+B", disabled: n < 1 || !has(G.GEO.breakApart), action: act.breakApart },
    { label: t("et.convertToPath"), hint: "Ctrl+Shift+C", disabled: n < 1 || !has(G.GEO.convert), action: act.convert },
    { label: t("et.bringToFront"), hint: "Ctrl+PgUp", disabled: n < 1 || !has(G.GEO.order), action: () => act.order("front") },
    { label: t("et.sendToBack"), hint: "Ctrl+PgDn", disabled: n < 1 || !has(G.GEO.order), action: () => act.order("back") },
    allLocked
      ? { label: t("et.unlock"), disabled: !has(G.GEO.lock), action: () => act.lock(false) }
      : { label: t("et.lock"), disabled: n < 1 || !has(G.GEO.lock), action: () => act.lock(true) },
  ];

  const toolbar = p.design ? (
    <EditToolbar
      tool={tool}
      onTool={(t) => p.setTool(t === tool ? "select" : t)}
      can={{ line: has(G.GEO.line), pen: has(G.GEO.path), polygon: has(G.GEO.polygon), node: has(G.GEO.nodeList), trim: has(G.GEO.trim) }}
      modify={modifyItems}
      arrange={arrangeItems}
    />
  ) : null;

  const guideDialog = guideDlg ? (
    <GuideDialog
      guide={guideDlg.id ? { axis: guideDlg.axis, v: guideDlg.v } : null}
      onClose={() => setGuideDlg(null)}
      onOk={(axis, v) => {
        const g = guideDlg;
        setGuideDlg(null);
        if (g.id) guideOps.move(g.id, v, axis !== g.axis ? axis : undefined);
        else guideOps.add(axis, v);
      }}
      onDelete={
        guideDlg.id
          ? () => {
              const id = guideDlg.id as string;
              setGuideDlg(null);
              guideOps.remove(id);
            }
          : undefined
      }
    />
  ) : null;
  const dialogs =
    guideDialog ??
    (dialog && doc ? (
      <EditDialogs
        dialog={dialog}
        doc={doc}
        ids={u}
        run={p.run}
        has={has}
        onClose={() => setDialog(null)}
        onDone={(ids) => ids.length && p.selectIds(ids)}
        onPick={(kind, values) => {
          setDialog(null);
          startPickFor(kind, values);
        }}
      />
    ) : null);

  const canvas: CanvasTool = {
    down,
    drag,
    up,
    cancel,
    hover,
    snapParts,
    render: (r) => <EditLayer store={store} r={r} tool={tool} guides={guides} doc={doc} polyOpts={polyOpts} design={p.design} />,
    overlay: (r) => (
      <EditOverlay
        store={store}
        r={r}
        tool={tool}
        doc={doc}
        polyOpts={polyOpts}
        onPolyOpts={setPolyOpts}
        onRulerDrag={(axis, s, phase) => rulerDrag(axis, s, r, phase)}
        guides={guides.length}
        onClearGuides={guideOps.clear}
        onConvert={(id) => cmd(G.GEO.convert, "convert to paths") && void exec(G.convertToPath([id]), () => setNodeTick((t) => t + 1))}
        onNodeKey={(key) => {
          nodeKey(new KeyboardEvent("keydown", { key }), false);
        }}
        onFinishLine={(closed) => finishPen(closed)}
        onCancelPick={() => p.setTool("select")}
      />
    ),
  };

  const handlers: Record<string, () => boolean | void> = {
    "tool.mirrorLine": () => (p.design && p.selection.objects.length > 0 && has(G.GEO.transform) ? act.pick("mirror") : false),
  };
  return { canvas, toolbar, dialogs, menuItems, handlers };
}

function sameHit(a: NodeHit | null, b: NodeHit | null): boolean {
  if (a === b) return true;
  if (!a || !b || a.kind !== b.kind || a.index !== b.index) return false;
  if (a.kind === "handle" && b.kind === "handle") return a.side === b.side;
  if (a.kind === "segment" && b.kind === "segment") return Math.abs(a.t - b.t) < 1e-3;
  return true;
}

// ---- drawing ---------------------------------------------------------------------------------------------------------

function EditLayer(props: { store: Store<Live>; r: RenderCtx; tool: Tool; guides: Guide[]; doc: GDocument | null; polyOpts: PolyOpts; design: boolean }) {
  const st = useLive(props.store);
  const { r, tool } = props;
  const px = r.px;
  const [x0, y1] = r.toField([0, 0]);
  const [x1, y0] = r.toField(r.size);
  const out: ReactNode[] = [];

  // Guides span the visible canvas; the wide transparent line is what a press grabs.
  for (const g of props.guides) {
    if (st.guideDrag?.id === g.id) continue;
    const d = g.axis === "x" ? `M${g.v},${y0}V${y1}` : `M${x0},${g.v}H${x1}`;
    out.push(
      <g key={`guide-${g.id}`} className={`et-guide-g et-guide-${g.axis}`}>
        <path className="et-guide" d={d} strokeWidth={px(1)} />
        {props.design && tool === "select" && <path className="et-guide-hit" data-guide={g.id} d={d} strokeWidth={px(9)} />}
      </g>,
    );
  }
  if (st.guideDrag) {
    const g = st.guideDrag;
    const d = g.axis === "x" ? `M${g.v},${y0}V${y1}` : `M${x0},${g.v}H${x1}`;
    out.push(<path key="guide-drag" className={`et-guide dragging${g.remove ? " remove" : ""}`} d={d} strokeWidth={px(1.3)} pointerEvents="none" />);
  }

  if (tool === "node" && st.nl) {
    const nl = st.preview ?? st.nl;
    const shown = handlesShown(nl, st.nodeSel);
    out.push(<path key="np" className="et-node-path" d={nodePathData(nl)} strokeWidth={px(1.6)} pointerEvents="none" />);
    if (st.hover?.kind === "segment") {
      const s = nl.segments[st.hover.index];
      if (s) out.push(<path key="nh" className="et-seg-hover" d={segmentPathData(nl, s)} strokeWidth={px(4)} pointerEvents="none" />);
    }
    const hs = px(3.5 * handleScale());
    for (const n of nl.nodes) {
      if (!shown.has(n.index)) continue;
      for (const [side, h] of [
        ["in", n.handle_in],
        ["out", n.handle_out],
      ] as const) {
        if (!h) continue;
        const hot = st.hover?.kind === "handle" && st.hover.index === n.index && st.hover.side === side;
        out.push(
          <g key={`h-${n.index}-${side}`} pointerEvents="none">
            <path className="et-handle-line" d={`M${n.point[0]},${n.point[1]}L${h[0]},${h[1]}`} strokeWidth={px(1)} />
            <circle className={`et-handle${hot ? " hot" : ""}`} cx={h[0]} cy={h[1]} r={hs} strokeWidth={px(1.2)} />
          </g>,
        );
      }
    }
    const ns = px(4 * handleScale());
    for (const n of nl.nodes) {
      const sel = st.nodeSel.has(n.index);
      const hot = st.hover?.kind === "node" && st.hover.index === n.index;
      const cls = `et-node${sel ? " sel" : ""}${hot ? " hot" : ""}${n.endpoint ? " end" : ""}`;
      out.push(
        n.type === "corner" ? (
          <rect key={`n-${n.index}`} className={cls} x={n.point[0] - ns} y={n.point[1] - ns} width={2 * ns} height={2 * ns} strokeWidth={px(1.2)} pointerEvents="none" />
        ) : (
          <circle key={`n-${n.index}`} className={cls} cx={n.point[0]} cy={n.point[1]} r={ns * 1.1} strokeWidth={px(1.2)} pointerEvents="none" />
        ),
      );
    }
    if (st.nodeBox) {
      const b = boxFromPoints(st.nodeBox.a, st.nodeBox.b);
      out.push(<rect key="nb" className="et-node-box" x={b.x0} y={b.y0} width={b.x1 - b.x0} height={b.y1 - b.y0} strokeWidth={px(1)} strokeDasharray={`${px(4)} ${px(3)}`} pointerEvents="none" />);
    }
  }

  if ((tool === "line" || tool === "pen") && st.pen.length) {
    out.push(<path key="pen" className="et-pen" d={penPathData(st.pen, false)} strokeWidth={px(1.4)} pointerEvents="none" />);
    const last = st.pen[st.pen.length - 1];
    if (st.rubber) {
      const d = last.hout ? `M${last.p[0]},${last.p[1]}C${last.hout[0]},${last.hout[1]} ${st.rubber[0]},${st.rubber[1]} ${st.rubber[0]},${st.rubber[1]}` : `M${last.p[0]},${last.p[1]}L${st.rubber[0]},${st.rubber[1]}`;
      out.push(<path key="rub" className="et-rubber" d={d} strokeWidth={px(1.2)} strokeDasharray={`${px(5)} ${px(3)}`} pointerEvents="none" />);
    }
    st.pen.forEach((n, i) => {
      if (n.hin && n.hout)
        out.push(
          <g key={`ph-${i}`} pointerEvents="none">
            <path className="et-handle-line" d={`M${n.hin[0]},${n.hin[1]}L${n.hout[0]},${n.hout[1]}`} strokeWidth={px(1)} />
            <circle className="et-handle" cx={n.hout[0]} cy={n.hout[1]} r={px(3)} strokeWidth={px(1)} />
            <circle className="et-handle" cx={n.hin[0]} cy={n.hin[1]} r={px(3)} strokeWidth={px(1)} />
          </g>,
        );
      out.push(<rect key={`pn-${i}`} className={`et-node${i === 0 ? " first" : ""}`} x={n.p[0] - px(3.5)} y={n.p[1] - px(3.5)} width={px(7)} height={px(7)} strokeWidth={px(1.2)} pointerEvents="none" />);
    });
  }

  if (tool === "polygon" && st.poly) {
    const { radius, rotationDeg } = polygonFromDrag(st.poly.c, st.poly.p, st.poly.shift);
    if (radius > 0) {
      const o = props.polyOpts;
      const pts = polygonPoints(st.poly.c, o.sides, radius, rotationDeg, o.star ? o.ratio : null);
      out.push(<path key="poly" className="rubber" d={`M${pts.map((q) => `${q[0]},${q[1]}`).join("L")}Z`} strokeWidth={px(1.2)} pointerEvents="none" />);
      out.push(<path key="polyr" className="rubber-radius" d={`M${st.poly.c[0]},${st.poly.c[1]}L${pts[0][0]},${pts[0][1]}`} strokeWidth={px(1)} strokeDasharray={`${px(4)} ${px(3)}`} pointerEvents="none" />);
    }
  }

  if (tool === "measure" && st.meas) {
    const { a, b } = st.meas;
    out.push(
      <g key="meas" className="et-measure" pointerEvents="none">
        <path d={`M${a[0]},${a[1]}L${b[0]},${b[1]}`} strokeWidth={px(1.4)} />
        <path className="et-measure-dxdy" d={`M${a[0]},${a[1]}H${b[0]}V${b[1]}`} strokeWidth={px(1)} strokeDasharray={`${px(3)} ${px(3)}`} />
        <circle cx={a[0]} cy={a[1]} r={px(3)} strokeWidth={px(1.2)} />
        <circle cx={b[0]} cy={b[1]} r={px(3)} strokeWidth={px(1.2)} />
      </g>,
    );
  }

  if (tool === "trim" && st.trimHover && props.doc) {
    const o = props.doc.objects.find((x) => x.id === st.trimHover);
    if (o && o.geometry.kind !== "image") out.push(<path key="trim" className="et-trim-hover" d={toPathData(geometryCmds(o.geometry), o.transform)} strokeWidth={px(2.5)} pointerEvents="none" />);
  }

  if (tool === "pick" && st.pick) {
    const pts = [...st.pick.pts];
    const hov = st.pickHover;
    const all = hov ? [...pts, hov] : pts;
    if (st.pick.preview === "line" && all.length === 2) out.push(<path key="pl" className="et-pick-line" d={`M${all[0][0]},${all[0][1]}L${all[1][0]},${all[1][1]}`} strokeWidth={px(1.4)} pointerEvents="none" />);
    if (st.pick.preview === "two-point" && all.length >= 2) {
      const pv = all[0];
      for (let i = 1; i < all.length; i++)
        out.push(<path key={`pl${i}`} className={i === 1 ? "et-pick-line ref" : "et-pick-line"} d={`M${pv[0]},${pv[1]}L${all[i][0]},${all[i][1]}`} strokeWidth={px(1.4)} pointerEvents="none" />);
    }
    pts.forEach((q, i) => out.push(<path key={`pp${i}`} className="et-pick-pt" d={`M${q[0] - px(6)},${q[1]}H${q[0] + px(6)}M${q[0]},${q[1] - px(6)}V${q[1] + px(6)}`} strokeWidth={px(1.6)} pointerEvents="none" />));
  }

  return <g className="et-layer">{out}</g>;
}

// ---- screen overlay: rulers and the tool options bar -------------------------------------------------------------

function EditOverlay(props: {
  store: Store<Live>;
  r: RenderCtx;
  tool: Tool;
  doc: GDocument | null;
  polyOpts: PolyOpts;
  onPolyOpts: (o: PolyOpts) => void;
  onRulerDrag: (axis: "x" | "y", screen: Vec2, phase: "move" | "end" | "cancel") => void;
  guides: number;
  onClearGuides: () => void;
  onConvert: (id: string) => void;
  onNodeKey: (key: string) => void;
  onFinishLine: (closed: boolean) => void;
  onCancelPick: () => void;
}) {
  const t = useT();
  const st = useLive(props.store);
  const { tool, r } = props;
  const units = usePrefs().units;
  let bar: ReactNode = null;
  const btn = (label: string, onClick: () => void, title?: string, disabled?: boolean) => (
    <button key={label} className="btn btn-small" onClick={onClick} title={title} disabled={disabled}>
      {label}
    </button>
  );
  switch (tool) {
    case "node": {
      const nl = st.nl;
      const o = nl && props.doc?.objects.find((x) => x.id === nl.id);
      if (!nl) bar = <span>{st.nlNote ?? t("et.clickAShapeToEditIts")}</span>;
      else if (nl.converts_on_edit)
        bar = (
          <>
            <span>
              <b>{o?.name ?? nl.id}</b> is a {SHAPE_WORD[nl.kind] ?? nl.kind}. Convert it to a path to edit its nodes.
            </span>
            {btn(t("et.convertToPath"), () => props.onConvert(nl.id), "Ctrl+Shift+C")}
          </>
        );
      else
        bar = (
          <>
            <span>
              <b>{o?.name ?? nl.id}</b> · {t("et.nNodes", { n: nl.nodes.length })}{st.nodeSel.size ? ` · ${t("et.nSelected", { n: st.nodeSel.size })}` : ""}
            </span>
            {btn(t("et.delete"), () => props.onNodeKey("Delete"), t("et.tipDelete"), !st.nodeSel.size)}
            {btn(t("et.break"), () => props.onNodeKey("b"), t("et.tipBreak"), st.nodeSel.size !== 1)}
            {btn(t("et.join"), () => props.onNodeKey("j"), t("et.tipJoin"), st.nodeSel.size !== 2)}
            {btn(t("et.smooth"), () => props.onNodeKey("s"), t("et.tipSmooth"), !st.nodeSel.size)}
            {btn(t("et.corner"), () => props.onNodeKey("c"), t("et.tipCorner"), !st.nodeSel.size)}
            {btn(t("et.line"), () => props.onNodeKey("l"), t("et.tipLine"), st.nodeSel.size < 2)}
            {btn(t("et.curve"), () => props.onNodeKey("k"), t("et.tipCurve"), st.nodeSel.size < 2)}
            <span className="et-hint">{t("et.nodeHint")}</span>
          </>
        );
      break;
    }
    case "line":
    case "pen":
      bar = (
        <>
          <span>
            <b>{tool === "line" ? t("et.lineTool") : t("et.penTool")}</b>: {tool === "line" ? t("et.lineHow") : t("et.penHow")} · {t("et.lineKeys")}
          </span>
          {btn(t("et.finish"), () => props.onFinishLine(false), "Enter", st.pen.length < 2)}
          {btn(t("et.close"), () => props.onFinishLine(true), "C", st.pen.length < (tool === "line" ? 3 : 2))}
        </>
      );
      break;
    case "polygon": {
      const o = props.polyOpts;
      bar = (
        <>
          <b>{o.star ? t("shape.Star") : t("shape.Polygon")}</b>
          <label className="et-opt">
            {o.star ? t("et.points") : t("et.sides")}
            <input
              type="number"
              min={3}
              max={1000}
              step={1}
              value={o.sides}
              onChange={(e) => {
                const v = Math.round(Number(e.target.value));
                if (v >= 3 && v <= 1000) props.onPolyOpts({ ...o, sides: v });
              }}
            />
          </label>
          <label className="et-opt">
            <input type="checkbox" checked={o.star} onChange={(e) => props.onPolyOpts({ ...o, star: e.target.checked })} />
            {t("shape.Star")}
          </label>
          {o.star && (
            <label className="et-opt">
              {t("et.inner")}
              <input
                type="number"
                min={1}
                max={99}
                step={1}
                value={Math.round(o.ratio * 100)}
                onChange={(e) => {
                  const v = Number(e.target.value);
                  if (v > 0 && v < 100) props.onPolyOpts({ ...o, ratio: v / 100 });
                }}
              />
              %
            </label>
          )}
          <label className="et-opt">
            {t("et.cornerRadius")}
            <input
              type="number"
              min={0}
              step={0.1}
              value={trimNumber(lengthToDisplay(o.cornerMm, units), lengthDigits(units, 3))}
              onChange={(e) => {
                const v = Number(e.target.value);
                if (v >= 0 && Number.isFinite(v)) props.onPolyOpts({ ...o, cornerMm: lengthFromDisplay(v, units) });
              }}
            />
            {units}
          </label>
          <span className="et-hint">{t("et.polyHint", { size: formatLength(20, units) })}</span>
        </>
      );
      break;
    }
    case "measure":
      bar = (
        <span>
          <b>{t("et.measure")}</b>: {st.meas ? measureText(measure(st.meas.a, st.meas.b), units) : t("et.measureHow")}
        </span>
      );
      break;
    case "trim":
      bar = (
        <span>
          <b>{t("et.trim")}</b>: {t("et.trimHow")}
        </span>
      );
      break;
    case "pick": {
      const pk = st.pick;
      if (pk)
        bar = (
          <>
            <span>
              <b>{pk.title}</b>: {pk.labels[pk.pts.length]} ({pk.pts.length + 1}/{pk.labels.length}) · {t("et.pickKeys")}
            </span>
            {btn(t("et.cancel"), props.onCancelPick)}
          </>
        );
      break;
    }
  }
  const meas = tool === "measure" && st.meas && st.meas.done ? st.meas : null;
  let label: ReactNode = null;
  if (meas) {
    const m = measure(meas.a, meas.b);
    const s = r.toScreen([(meas.a[0] + meas.b[0]) / 2, (meas.a[1] + meas.b[1]) / 2]);
    label = (
      <div className="vp-overlay et-measure-label" style={{ left: s[0] + 10, top: s[1] + 10 }}>
        {fixedLength(m.dist, units)}
        <br />
        ΔX {fixedNumber(m.dx, units)} · ΔY {fixedNumber(m.dy, units)} · {m.angleDeg.toFixed(1)}°
      </div>
    );
  }
  return (
    <>
      <Rulers r={r} guideDrag={st.guideDrag} onDrag={props.onRulerDrag} guides={props.guides} onClearGuides={props.onClearGuides} />
      {bar && (
        <div className="vp-overlay et-bar" role="toolbar" aria-label={t("et.toolOptions")}>
          {bar}
        </div>
      )}
      {label}
    </>
  );
}
