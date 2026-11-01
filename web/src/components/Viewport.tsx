// The centre canvas: an SVG in field millimetres with +Y up. One group carries the view transform (pan/zoom and the
// y-flip); everything inside it is drawn in field mm. Stroke widths are divided by the zoom so they stay a constant
// number of screen pixels.
//
// Interaction uses Pointer Events only (mouse, pen and touch alike), following LightBurn:
// - click an object (its line, or inside a closed shape) to select it; Shift adds, Ctrl toggles, Ctrl+Shift removes;
//   a group selects as a whole;
// - drag on empty space draws a selection box: left to right encloses (solid), right to left crosses (dashed green);
// - drag a selected object to move it (Shift constrains to H/V/45°, Ctrl turns snapping off, Alt shows alignment
//   guides); drag the corner/edge handles to scale (Shift frees the aspect, Ctrl scales about the centre) and the
//   round handles outside the corners to rotate (Shift 15°, Ctrl 5°); Esc before release cancels;
// - drag the stock to move it while it is selected (click its outline to select it);
// - middle-drag pans, the wheel zooms (Ctrl+wheel is a trackpad pinch, sideways scrolling pans), touch: one finger on
//   empty space pans, two fingers pan and pinch;
// - with the Rectangle or Ellipse tool, drag to draw (a tap places a 20 mm shape).
// Every edit is a command sent on pointerup; until the engine answers, the change is drawn as a transient transform.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { createdIds } from "../api/drop.ts";
import { energyOpacity, layerEnergy } from "../api/laser.ts";
import type { Affine, GDocument, Layer, Preview, Vec2 } from "../api/types.ts";
import {
  HANDLES,
  constrainDelta,
  grabbedKeypoint,
  handlePoint,
  rotateAbout,
  rotateFromHandle,
  rotatedRectTargets,
  roundMm,
  scaleAbout,
  scaleFromHandle,
  signedScaleParams,
  snapMove,
  targetsOf,
  type Handle,
  type Mods,
  type SnapTarget,
} from "../editing.ts";
import {
  boxCenter,
  boxFromPoints,
  cmdsBounds,
  distToPolylines,
  flattenCmds,
  geometryCmds,
  mul,
  stockCmds,
  toPathData,
  unionBoxes,
  type Box,
} from "../geometry.ts";
import {
  applySelect,
  boxHits,
  buildShapeIndex,
  hitTest,
  selectOpFor,
  unitOf,
  unitsOf,
  type SelectOp,
  type Selection,
} from "../selection.ts";
import { Icon } from "./ui.tsx";

export type Tool = "select" | "rect" | "ellipse";

/** View transform: field point (cx, cy) at the canvas centre, `scale` screen px per mm. */
interface View {
  cx: number;
  cy: number;
  scale: number;
}

export interface SnapSettings {
  objects: boolean;
  grid: boolean;
  gridMm: number;
}

export type ViewStyle = "filled" | "wireframe";

/** Preview display options (Laser workspace). */
export interface PreviewOptions {
  /** Layers hidden from the preview display (display only; output is unchanged). */
  hidden: ReadonlySet<string>;
  /** Draw only the first N segments (draw-order animation), or null for all. */
  upTo: number | null;
  /** Shade marks by energy (darker = more) instead of the layer colour. */
  shade: boolean;
  /** Show travel moves. */
  jumps: boolean;
}

type Gesture =
  | { kind: "none" }
  | { kind: "pan"; start: Vec2; view: View; moved: boolean; clearOnTap: boolean }
  | { kind: "marquee"; start: Vec2; startScreen: Vec2; op: SelectOp; moved: boolean }
  | {
      kind: "move";
      startField: Vec2;
      startScreen: Vec2;
      moved: boolean;
      ids: string[];
      stock: boolean;
      box: Box;
      grab: Vec2 | null;
      targets: SnapTarget[];
      /** Selection to narrow to when this turns out to be a click without a drag. */
      tapSelect: string | null;
    }
  | { kind: "scale"; handle: Handle; box: Box; ids: string[]; start: Vec2 }
  | { kind: "rotate"; center: Vec2; start: Vec2; ids: string[] }
  | { kind: "draw"; start: Vec2; tool: "rect" | "ellipse"; targets: SnapTarget[] }
  | { kind: "pinch"; dist: number; mid: Vec2; view: View }
  | { kind: "idle" };

/** A transform drawn on top of the document until the engine answers. */
interface Transient {
  m: Affine;
  stock: boolean;
}

const MIN_SCALE = 0.2;
const MAX_SCALE = 400;
/** Screen px a pointer may wander before a tap becomes a drag. */
const TAP_SLOP = 4;
/** Click tolerance, screen px (LightBurn "Click Selection Tolerance"). */
const CLICK_TOL_PX = 5;
/** Object snap distance, screen px (LightBurn "Object Snap Distance"); doubled while Alt is held. */
const SNAP_PX = 10;

export interface ViewportProps {
  doc: GDocument | null;
  fieldMm: number;
  selection: Selection;
  tool: Tool;
  /** Layer that new shapes go on. */
  activeLayer: string | null;
  preview: Preview | null;
  showPreview: boolean;
  previewOptions: PreviewOptions;
  viewStyle: ViewStyle;
  snap: SnapSettings;
  /** Objects outside the galvo field, drawn in red. */
  outside: ReadonlySet<string>;
  has: (cmd: string) => boolean;
  onSelection: (s: Selection) => void;
  onToolDone: () => void;
  /** Right-click: the app shows its context menu at client (x, y). */
  onContextMenu: (x: number, y: number) => void;
  /** Field point under the pointer (for paste), or null when it is outside the canvas. */
  onCursor?: (p: Vec2 | null) => void;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  notify: (msg: string, kind?: "info" | "error") => void;
  report: (e: unknown) => void;
}

export function Viewport(props: ViewportProps) {
  const { doc, fieldMm, selection, tool, preview, showPreview, previewOptions, viewStyle, snap, outside } = props;
  const wrapRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const [size, setSize] = useState<Vec2>([1, 1]);
  const [view, setView] = useState<View>({ cx: 0, cy: 0, scale: 3 });
  const [cursor, setCursor] = useState<Vec2 | null>(null);
  const [transient, setTransient] = useState<Transient | null>(null);
  const [draw, setDraw] = useState<{ a: Vec2; b: Vec2; tool: "rect" | "ellipse" } | null>(null);
  const [marquee, setMarquee] = useState<{ a: Vec2; b: Vec2 } | null>(null);
  const [snapShown, setSnapShown] = useState<{ target: SnapTarget | null; guides: { a: Vec2; b: Vec2 }[] } | null>(null);
  const [readout, setReadout] = useState<string | null>(null);
  /** Transient kept on screen after pointerup until the document revision changes, so nothing snaps back. */
  const [settling, setSettling] = useState<(Transient & { rev: number }) | null>(null);
  const pointers = useRef(new Map<number, Vec2>());
  const gesture = useRef<Gesture>({ kind: "none" });
  /** Last pointer position and modifiers, so pressing or releasing a modifier mid-drag updates the drag. */
  const lastMove = useRef<{ s: Vec2; m: Mods } | null>(null);
  /** The view follows the field (refit on resize) until the user pans or zooms. */
  const autoFit = useRef(true);
  const fittedField = useRef<number | null>(null);

  // Track canvas size.
  useLayoutEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      setSize([Math.max(1, Math.round(r.width)), Math.max(1, Math.round(r.height))]);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const fitBox = useCallback(
    (b: Box) => {
      const w = Math.max(b.x1 - b.x0, 1);
      const h = Math.max(b.y1 - b.y0, 1);
      const scale = clamp(Math.min(size[0] / w, size[1] / h) * 0.9, MIN_SCALE, MAX_SCALE);
      const [cx, cy] = boxCenter(b);
      setView({ cx, cy, scale });
    },
    [size],
  );
  const fieldBox = useMemo<Box>(() => ({ x0: -fieldMm / 2, y0: -fieldMm / 2, x1: fieldMm / 2, y1: fieldMm / 2 }), [fieldMm]);
  const fitField = useCallback(() => {
    autoFit.current = true;
    fitBox(fieldBox);
  }, [fitBox, fieldBox]);

  // Fit the field once the canvas has a real size, again when the field size changes, and on every resize until the
  // user pans or zooms (so the first fit never leaves the field clipped).
  useEffect(() => {
    if (size[0] <= 20 || size[1] <= 20) return;
    if (autoFit.current || fittedField.current !== fieldMm) {
      fittedField.current = fieldMm;
      fitBox(fieldBox);
    }
  }, [size, fitBox, fieldBox, fieldMm]);

  useEffect(() => {
    if (settling && doc && doc.revision !== settling.rev) setSettling(null);
  }, [doc, settling]);

  const toField = useCallback(
    (sx: number, sy: number, v: View = view): Vec2 => [(sx - size[0] / 2) / v.scale + v.cx, -(sy - size[1] / 2) / v.scale + v.cy],
    [view, size],
  );

  const local = (e: { clientX: number; clientY: number }): Vec2 => {
    const r = svgRef.current?.getBoundingClientRect();
    return r ? [e.clientX - r.left, e.clientY - r.top] : [e.clientX, e.clientY];
  };

  // Wheel zooms about the pointer. Sideways scrolling (two-finger trackpad) pans; Ctrl+wheel is a trackpad pinch.
  // A non-passive listener is needed to keep the page from scrolling or zooming.
  useEffect(() => {
    const el = svgRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      autoFit.current = false;
      const s = local(e);
      const pixel = e.deltaMode === 0;
      if (!e.ctrlKey && pixel && e.deltaX !== 0) {
        setView((v) => ({ ...v, cx: v.cx + e.deltaX / v.scale, cy: v.cy - e.deltaY / v.scale }));
        return;
      }
      const dy = pixel ? e.deltaY : e.deltaY * 40;
      const factor = Math.exp(-clamp(dy, -300, 300) * (e.ctrlKey ? 0.01 : 0.0015));
      setView((v) => zoomAtPure(v, size, s, factor));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [size]);

  const selectedSet = useMemo(() => new Set(selection.objects), [selection.objects]);
  const layerById = useMemo(() => new Map((doc?.layers ?? []).map((l) => [l.id, l])), [doc]);
  const shapeIndex = useMemo(() => (doc ? buildShapeIndex(doc) : []), [doc]);
  const boundsById = useMemo(() => new Map(shapeIndex.map((s) => [s.id, s.box])), [shapeIndex]);

  const live = transient ?? settling;
  const liveObjects = live && !live.stock ? live.m : null;
  const liveStock = live && live.stock ? live.m : null;

  const selBoundsRaw = useMemo(() => unionBoxes(selection.objects.map((id) => boundsById.get(id) ?? null)), [selection.objects, boundsById]);
  const selBounds = useMemo(() => (selBoundsRaw && liveObjects ? mapBox(selBoundsRaw, liveObjects) : selBoundsRaw), [selBoundsRaw, liveObjects]);

  const stock = doc?.stock ? stockCmds(doc.stock) : null;
  const stockBox = stock ? cmdsBounds(stock.cmds, stock.transform) : null;

  /** Snap targets for a drag: other objects (groups as one box), the stock and the field. */
  const snapTargets = useCallback(
    (exclude: ReadonlySet<string>, includeStock: boolean): SnapTarget[] => {
      if (!doc) return [];
      const out: SnapTarget[] = [];
      const units = new Map<string, Box[]>();
      for (const s of shapeIndex) {
        if (exclude.has(s.id)) continue;
        const u = unitOf(doc, s.id);
        units.set(u, [...(units.get(u) ?? []), s.box]);
      }
      for (const [u, boxes] of units) {
        const b = unionBoxes(boxes);
        if (!b) continue;
        const obj = doc.objects.find((o) => o.id === u);
        out.push(...targetsOf(b, obj ? `"${obj.name || obj.id}"` : "group"));
      }
      if (includeStock && doc.stock) {
        const s = doc.stock;
        if (s.shape.kind === "rect") out.push(...rotatedRectTargets(s.center, s.shape.width_mm, s.shape.height_mm, s.rotation_deg, "stock"));
        else out.push(...rotatedRectTargets(s.center, s.shape.diameter_mm, s.shape.diameter_mm, 0, "stock"));
      }
      out.push({ p: [0, 0], kind: "center", label: "field centre" });
      out.push(...targetsOf(fieldBox, "field").filter((t) => t.kind !== "center"));
      return out;
    },
    [doc, shapeIndex, fieldBox],
  );

  const snapOn = (m: Mods) => (m.ctrl ? !snap.objects : snap.objects);
  const gridMm = (m: Mods) => (snap.grid && !m.ctrl ? snap.gridMm : 0);

  /** A point snapped for drawing (N6): object/stock/field keypoints, else the grid. */
  const snapPoint = (p: Vec2, targets: SnapTarget[], m: Mods): { p: Vec2; t: SnapTarget | null } => {
    const r = (SNAP_PX * (m.alt ? 2 : 1)) / view.scale;
    if (snapOn(m)) {
      let best: SnapTarget | null = null;
      let bd = r;
      for (const t of targets) {
        const d = Math.hypot(t.p[0] - p[0], t.p[1] - p[1]);
        if (d <= bd) {
          bd = d;
          best = t;
        }
      }
      if (best) return { p: best.p, t: best };
    }
    const g = gridMm(m);
    return { p: g > 0 ? [Math.round(p[0] / g) * g, Math.round(p[1] / g) * g] : p, t: null };
  };

  const cancelGesture = useCallback(() => {
    const g = gesture.current;
    gesture.current = pointers.current.size ? { kind: "idle" } : { kind: "none" };
    setTransient(null);
    setDraw(null);
    setMarquee(null);
    setSnapShown(null);
    setReadout(null);
    return g.kind !== "none" && g.kind !== "idle";
  }, []);

  // Esc during a drag cancels it (and nothing else: the app's Esc must not also switch tools or clear the selection).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const g = gesture.current.kind;
      if (g === "none" || g === "idle" || g === "pinch" || g === "pan") return;
      cancelGesture();
      e.preventDefault();
      e.stopImmediatePropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [cancelGesture]);

  // Pressing or releasing Shift/Ctrl/Alt mid-drag re-evaluates the drag at the last pointer position.
  useEffect(() => {
    const onMod = (e: KeyboardEvent) => {
      if (!["Shift", "Control", "Alt", "Meta"].includes(e.key)) return;
      const lm = lastMove.current;
      const g = gesture.current.kind;
      if (!lm || g === "none" || g === "idle") return;
      if (e.key === "Alt") e.preventDefault(); // keep Alt from focusing the browser menu mid-drag
      update(lm.s, { shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey, alt: e.altKey });
    };
    window.addEventListener("keydown", onMod);
    window.addEventListener("keyup", onMod);
    return () => {
      window.removeEventListener("keydown", onMod);
      window.removeEventListener("keyup", onMod);
    };
  });

  const modsOf = (e: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean; altKey: boolean }): Mods => ({
    shift: e.shiftKey,
    ctrl: e.ctrlKey || e.metaKey,
    alt: e.altKey,
  });

  const onPointerDown = (e: React.PointerEvent<SVGSVGElement>) => {
    if (!doc) return;
    const mouse = e.pointerType === "mouse";
    if (mouse && e.button === 2) return; // context menu
    if (mouse && e.button !== 0 && e.button !== 1) return;
    e.preventDefault(); // no text selection, no middle-click autoscroll
    wrapRef.current?.focus({ preventScroll: true });
    const s = local(e);
    pointers.current.set(e.pointerId, s);
    try {
      svgRef.current?.setPointerCapture(e.pointerId);
    } catch {
      // Capture can fail for synthetic events; harmless.
    }
    if (pointers.current.size === 2) {
      // Second finger: abandon the single-pointer gesture and pinch instead.
      cancelGesture();
      const [p, q] = [...pointers.current.values()] as [Vec2, Vec2];
      gesture.current = { kind: "pinch", dist: dist(p, q), mid: mid(p, q), view };
      return;
    }
    if (pointers.current.size > 2) return;
    const m = modsOf(e);
    lastMove.current = { s, m };
    if (mouse && e.button === 1) {
      gesture.current = { kind: "pan", start: s, view, moved: false, clearOnTap: false };
      return;
    }
    const f = toField(s[0], s[1]);
    if (tool !== "select") {
      const targets = snapTargets(new Set(), true);
      const sp = snapPoint(f, targets, m).p;
      gesture.current = { kind: "draw", start: sp, tool, targets };
      setDraw({ a: sp, b: sp, tool });
      return;
    }

    // Handles of the current selection come first.
    const handleEl = (e.target as Element).closest?.("[data-handle]");
    if (handleEl && selBoundsRaw) {
      const h = handleEl.getAttribute("data-handle") ?? "";
      if (h.startsWith("rot")) {
        gesture.current = { kind: "rotate", center: boxCenter(selBoundsRaw), start: f, ids: selection.objects };
      } else {
        gesture.current = { kind: "scale", handle: h as Handle, box: selBoundsRaw, ids: selection.objects, start: f };
      }
      return;
    }

    const tol = CLICK_TOL_PX / view.scale;
    const hit = hitTest(shapeIndex, f, tol);
    const op = selectOpFor(m);
    if (hit) {
      const isSel = selectedSet.has(hit);
      let next = selection;
      if (op === "replace") {
        if (!isSel) next = applySelect(selection, doc, [hit], "replace");
      } else {
        next = applySelect(selection, doc, [hit], op);
      }
      if (next !== selection) props.onSelection(next);
      // Toggling or removing an object never starts a move of the old selection.
      if (!next.objects.includes(hit)) {
        gesture.current = { kind: "idle" };
        return;
      }
      const ids = next.objects;
      const box = unionBoxes(ids.map((id) => boundsById.get(id) ?? null));
      if (!box) return;
      gesture.current = {
        kind: "move",
        startField: f,
        startScreen: s,
        moved: false,
        ids,
        stock: false,
        box,
        grab: grabbedKeypoint(box, f, SNAP_PX / view.scale),
        targets: snapTargets(new Set(ids), true),
        tapSelect: op === "replace" && isSel && selection.objects.length > 1 ? hit : null,
      };
      return;
    }

    // The stock: its outline selects it; while it is selected, dragging anywhere on it moves it.
    if (stock && doc.stock) {
      const polys = flattenCmds(stock.cmds, stock.transform);
      const onLine = distToPolylines(f, polys) <= tol;
      const inside = stockBox !== null && insideStock(f, doc.stock);
      if (onLine && !selection.stock) {
        props.onSelection({ objects: [], layer: null, stock: true });
      }
      if (onLine || (selection.stock && inside)) {
        gesture.current = {
          kind: "move",
          startField: f,
          startScreen: s,
          moved: false,
          ids: [],
          stock: true,
          box: stockBox ?? { x0: f[0], y0: f[1], x1: f[0], y1: f[1] },
          grab: stockBox ? grabbedKeypoint(stockBox, f, SNAP_PX / view.scale) : null,
          targets: snapTargets(new Set(), false),
          tapSelect: null,
        };
        return;
      }
    }

    if (e.pointerType === "touch") {
      // One finger on empty space pans; a tap clears the selection.
      gesture.current = { kind: "pan", start: s, view, moved: false, clearOnTap: op === "replace" };
      return;
    }
    gesture.current = { kind: "marquee", start: f, startScreen: s, op, moved: false };
  };

  /** Advance the current drag to screen point `s` with modifiers `m`. */
  const update = (s: Vec2, m: Mods) => {
    const g = gesture.current;
    const f = toField(s[0], s[1]);
    switch (g.kind) {
      case "pan": {
        const dx = s[0] - g.start[0];
        const dy = s[1] - g.start[1];
        if (!g.moved && Math.hypot(dx, dy) < TAP_SLOP) return;
        g.moved = true;
        autoFit.current = false;
        setView({ ...g.view, cx: g.view.cx - dx / g.view.scale, cy: g.view.cy + dy / g.view.scale });
        break;
      }
      case "marquee": {
        if (!g.moved && Math.hypot(s[0] - g.startScreen[0], s[1] - g.startScreen[1]) < TAP_SLOP) return;
        g.moved = true;
        setMarquee({ a: g.start, b: f });
        break;
      }
      case "move": {
        if (!g.moved && Math.hypot(s[0] - g.startScreen[0], s[1] - g.startScreen[1]) < TAP_SLOP) return;
        g.moved = true;
        let d: Vec2 = [f[0] - g.startField[0], f[1] - g.startField[1]];
        if (m.shift) d = constrainDelta(d);
        let target: SnapTarget | null = null;
        let guides: { a: Vec2; b: Vec2 }[] = [];
        if (!m.shift && (snapOn(m) || gridMm(m) > 0)) {
          const r = snapMove(g.box, d, snapOn(m) ? g.targets : [], {
            radius: (SNAP_PX * (m.alt ? 2 : 1)) / view.scale,
            grab: g.grab,
            guides: !!m.alt && snapOn(m),
            grid: gridMm(m),
          });
          d = r.delta;
          target = r.target;
          guides = r.guides;
        }
        setSnapShown(target || guides.length ? { target, guides } : null);
        setTransient({ m: [1, 0, 0, 1, d[0], d[1]], stock: g.stock });
        setReadout(`Δ ${d[0].toFixed(2)}, ${d[1].toFixed(2)} mm${target ? ` · ${target.label}` : ""}`);
        break;
      }
      case "scale": {
        const sc = scaleFromHandle(g.box, g.handle, handleTarget(g, f), m);
        setTransient({ m: scaleAbout(sc.sx, sc.sy, sc.about), stock: false });
        const w = (g.box.x1 - g.box.x0) * Math.abs(sc.sx);
        const h = (g.box.y1 - g.box.y0) * Math.abs(sc.sy);
        const [px, py] = [Math.abs(sc.sx) * 100, Math.abs(sc.sy) * 100];
        const flipped = sc.sx < 0 && sc.sy < 0 ? " · mirrored both ways" : sc.sx < 0 ? " · mirrored horizontally" : sc.sy < 0 ? " · mirrored vertically" : "";
        setReadout(`${w.toFixed(2)} × ${h.toFixed(2)} mm (${px.toFixed(1)}%${px !== py ? ` × ${py.toFixed(1)}%` : ""})${flipped}`);
        break;
      }
      case "rotate": {
        const deg = rotateFromHandle(g.center, g.start, f, m);
        setTransient({ m: rotateAbout(deg, g.center), stock: false });
        setReadout(`Rotate ${Math.abs(deg).toFixed(1)}° ${deg < 0 ? "clockwise" : "counter-clockwise"}`);
        break;
      }
      case "draw": {
        const sp = snapPoint(f, g.targets, m);
        setSnapShown(sp.t ? { target: sp.t, guides: [] } : null);
        setDraw({ a: g.start, b: sp.p, tool: g.tool });
        break;
      }
    }
  };

  const onPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const s = local(e);
    const f = toField(s[0], s[1]);
    setCursor(f);
    props.onCursor?.(f);
    if (!pointers.current.has(e.pointerId)) return;
    pointers.current.set(e.pointerId, s);
    const g = gesture.current;
    if (g.kind === "pinch") {
      const pts = [...pointers.current.values()];
      if (pts.length < 2) return;
      const [p, q] = pts as [Vec2, Vec2];
      const m = mid(p, q);
      const factor = dist(p, q) / Math.max(g.dist, 1);
      autoFit.current = false;
      // Zoom about the starting midpoint, then pan by the midpoint's travel.
      const z = zoomAtPure(g.view, size, g.mid, factor);
      setView({ ...z, cx: z.cx - (m[0] - g.mid[0]) / z.scale, cy: z.cy + (m[1] - g.mid[1]) / z.scale });
      return;
    }
    const m = modsOf(e);
    lastMove.current = { s, m };
    update(s, m);
  };

  const commit = (inv: Invocation, t: Transient, done?: (result: unknown) => void) => {
    setSettling({ ...t, rev: doc?.revision ?? -1 });
    props
      .run(inv)
      .then((r) => done?.(r))
      .catch((err) => {
        setSettling(null);
        props.report(err);
      });
  };

  const endPointer = (e: React.PointerEvent<SVGSVGElement>, cancelled: boolean) => {
    if (!pointers.current.has(e.pointerId)) return;
    pointers.current.delete(e.pointerId);
    const g = gesture.current;
    if (g.kind === "pinch") {
      // Lifting one finger of a pinch ends the gesture; the remaining finger does nothing until lifted.
      if (pointers.current.size === 0) gesture.current = { kind: "none" };
      return;
    }
    if (pointers.current.size === 0) gesture.current = { kind: "none" };
    const t = transient;
    setTransient(null);
    setSnapShown(null);
    setReadout(null);
    if (cancelled || !doc) {
      setDraw(null);
      setMarquee(null);
      return;
    }
    const s = local(e);
    const f = toField(s[0], s[1]);
    switch (g.kind) {
      case "pan":
        if (!g.moved && g.clearOnTap) props.onSelection({ objects: [], layer: null, stock: false });
        break;
      case "marquee": {
        setMarquee(null);
        if (!g.moved) {
          if (g.op === "replace") props.onSelection({ objects: [], layer: null, stock: false });
          break;
        }
        const box = boxFromPoints(g.start, f);
        const crossing = f[0] < g.start[0];
        props.onSelection(applySelect(selection, doc, boxHits(doc, shapeIndex, box, crossing), g.op));
        break;
      }
      case "move": {
        if (!g.moved || !t) {
          if (g.tapSelect) props.onSelection(applySelect(selection, doc, [g.tapSelect], "replace"));
          break;
        }
        const d: Vec2 = [roundMm(t.m[4]), roundMm(t.m[5])];
        if (d[0] === 0 && d[1] === 0) break;
        if (g.stock) {
          const st = doc.stock;
          if (!st) break;
          const inv = props.has(C.OPT.stockMove)
            ? C.stockMove({ translate: d })
            : C.stockDefine({ ...st, center: [roundMm(st.center[0] + d[0]), roundMm(st.center[1] + d[1])] });
          commit(inv, { m: [1, 0, 0, 1, d[0], d[1]], stock: true });
        } else {
          commit(C.transform(g.ids, { translate: d }), { m: [1, 0, 0, 1, d[0], d[1]], stock: false });
        }
        break;
      }
      case "scale": {
        if (!t) break;
        const sc = scaleFromHandle(g.box, g.handle, handleTarget(g, f), modsOf(e));
        if (Math.abs(sc.sx - 1) < 1e-6 && Math.abs(sc.sy - 1) < 1e-6) break;
        const about: Vec2 = [roundMm(sc.about[0]), roundMm(sc.about[1])];
        // Past the opposite side the selection mirrors (LightBurn); the engine takes that as a mirror, not a factor < 0.
        const sp = signedScaleParams(round6(sc.sx), round6(sc.sy));
        commit(C.transform(g.ids, { ...sp, about }), { m: scaleAbout(sc.sx, sc.sy, about), stock: false });
        break;
      }
      case "rotate": {
        if (!t) break;
        const deg = rotateFromHandle(g.center, g.start, f, modsOf(e));
        if (Math.abs(deg) < 1e-6) break;
        const about: Vec2 = [roundMm(g.center[0]), roundMm(g.center[1])];
        commit(C.transform(g.ids, { rotate_deg: round6(deg), about }), { m: rotateAbout(deg, about), stock: false });
        break;
      }
      case "draw": {
        setDraw(null);
        const end = snapPoint(f, g.targets, modsOf(e)).p;
        finishDraw(g.start, end, g.tool);
        break;
      }
    }
  };

  const finishDraw = (a: Vec2, b: Vec2, shape: "rect" | "ellipse") => {
    if (!doc) return;
    const layer = props.activeLayer ?? doc.layers[0]?.id;
    if (!layer) {
      props.notify("No layer to draw on.", "error");
      return;
    }
    let w = Math.abs(b[0] - a[0]);
    let h = Math.abs(b[1] - a[1]);
    let center: Vec2 = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    if (w * view.scale < TAP_SLOP && h * view.scale < TAP_SLOP) {
      // A tap: drop a default-size shape centred on it.
      w = h = 20;
      center = a;
    }
    center = [roundMm(center[0]), roundMm(center[1])];
    w = Math.max(roundMm(w), 0.01);
    h = Math.max(roundMm(h), 0.01);
    const inv = shape === "ellipse" ? C.shapeEllipse(layer, center, w / 2, h / 2) : C.shapeRect(layer, center, w, h);
    props
      .run(inv)
      .then((r) => {
        const ids = createdIds(r);
        if (ids.length && doc) props.onSelection({ objects: ids, layer, stock: false, lastAdded: ids, reference: null });
      })
      .catch(props.report);
    props.onToolDone();
  };

  // --- rendering -------------------------------------------------------------------------------------------------
  const px = (n: number) => n / view.scale;
  const half = fieldMm / 2;
  const matrix = `matrix(${view.scale},0,0,${-view.scale},${size[0] / 2 - view.scale * view.cx},${size[1] / 2 + view.scale * view.cy})`;

  const grid = useMemo(() => {
    const minor = [0.5, 1, 2, 5, 10, 20, 50].find((st) => st * view.scale >= 10) ?? 50;
    const major = minor * (minor === 2 || minor === 20 ? 5 : minor === 0.5 || minor === 5 || minor === 50 ? 2 : 10);
    let dMinor = "";
    let dMajor = "";
    const n = Math.floor(half / minor);
    for (let i = -n; i <= n; i++) {
      const v = i * minor;
      const line = `M${v},${-half}V${half}M${-half},${v}H${half}`;
      if (Math.abs(v / major - Math.round(v / major)) < 1e-9) dMajor += line;
      else dMinor += line;
    }
    return { dMinor, dMajor };
  }, [half, view.scale]);

  /** Object path data in the field frame, cached per document. */
  const paths = useMemo(() => {
    const m = new Map<string, string>();
    for (const o of doc?.objects ?? []) m.set(o.id, toPathData(geometryCmds(o.geometry), o.transform));
    return m;
  }, [doc]);

  // Filled view (LightBurn "Filled"): fill and fill+line layers are drawn as even-odd regions in their layer colour.
  const fills = useMemo(() => {
    if (!doc || viewStyle !== "filled") return [];
    const out: { layer: Layer; d: string }[] = [];
    for (const l of doc.layers) {
      if (!l.visible || l.settings.mode === "line") continue;
      const d = doc.objects
        .filter((o) => o.layer === l.id && o.geometry.kind !== "image" && !(liveObjects && selectedSet.has(o.id)))
        .map((o) => paths.get(o.id) ?? "")
        .join(" ");
      if (d) out.push({ layer: l, d });
    }
    return out;
  }, [doc, viewStyle, paths, liveObjects, selectedSet]);

  const previewPaths = useMemo(() => {
    if (!preview || !showPreview) return null;
    const marks = new Map<string, string>();
    let jumps = "";
    const segs = preview.segments ?? [];
    const n = previewOptions.upTo === null ? segs.length : Math.min(segs.length, previewOptions.upTo);
    let head: Vec2 | null = null;
    for (let i = 0; i < n; i++) {
      const s = segs[i];
      head = s.to;
      if (s.kind === "mark") {
        const k = s.layer ?? "";
        if (previewOptions.hidden.has(k)) continue;
        marks.set(k, (marks.get(k) ?? "") + `M${r4(s.from[0])},${r4(s.from[1])}L${r4(s.to[0])},${r4(s.to[1])}`);
      } else if (previewOptions.jumps) jumps += `M${r4(s.from[0])},${r4(s.from[1])}L${r4(s.to[0])},${r4(s.to[1])}`;
    }
    const maxE = Math.max(0, ...(doc?.layers ?? []).map((l) => layerEnergy(l.settings)));
    return { marks: [...marks.entries()], jumps, head: previewOptions.upTo !== null && n < segs.length ? head : null, maxE };
  }, [preview, showPreview, previewOptions, doc]);

  const drawBox = draw ? boxFromPoints(draw.a, draw.b) : null;
  const marqueeBox = marquee ? boxFromPoints(marquee.a, marquee.b) : null;
  const crossing = marquee ? marquee.b[0] < marquee.a[0] : false;

  // Groups among the selection get a dot-dot-dash outline around their bounds.
  const selectedGroups = useMemo(() => {
    if (!doc) return [];
    return unitsOf(doc, selection.objects)
      .filter((u) => !doc.objects.some((o) => o.id === u && !o.group))
      .map((u) => unionBoxes(doc.objects.filter((o) => selectedSet.has(o.id) && unitOf(doc, o.id) === u).map((o) => boundsById.get(o.id) ?? null)))
      .filter((b): b is Box => b !== null)
      .map((b) => (liveObjects ? mapBox(b, liveObjects) : b));
  }, [doc, selection.objects, selectedSet, boundsById, liveObjects]);

  const showHandles = tool === "select" && selBounds && selection.objects.length > 0 && !showPreview;
  const liveM = liveObjects ? `matrix(${liveObjects.join(",")})` : undefined;
  const outsideCount = outside.size;

  return (
    <div
      className={`viewport tool-${tool}`}
      ref={wrapRef}
      tabIndex={-1}
      onContextMenu={(e) => {
        e.preventDefault();
        if (!doc || tool !== "select") return;
        const s = local(e);
        const hit = hitTest(shapeIndex, toField(s[0], s[1]), CLICK_TOL_PX / view.scale);
        if (hit && !selectedSet.has(hit)) props.onSelection(applySelect(selection, doc, [hit], "replace"));
        props.onContextMenu(e.clientX, e.clientY);
      }}
    >
      <svg
        ref={svgRef}
        width={size[0]}
        height={size[1]}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={(e) => endPointer(e, false)}
        onPointerCancel={(e) => endPointer(e, true)}
        onPointerLeave={() => {
          setCursor(null);
          props.onCursor?.(null);
        }}
        onAuxClick={(e) => e.preventDefault()}
      >
        <g transform={matrix}>
          {/* galvo field */}
          <rect className="field-bg" x={-half} y={-half} width={fieldMm} height={fieldMm} />
          <path className="grid-minor" d={grid.dMinor} strokeWidth={px(0.6)} />
          <path className="grid-major" d={grid.dMajor} strokeWidth={px(0.9)} />
          <path className="axis-x" d={`M${-half},0H${half}`} strokeWidth={px(1.2)} />
          <path className="axis-y" d={`M0,${-half}V${half}`} strokeWidth={px(1.2)} />
          <rect className="field-edge" x={-half} y={-half} width={fieldMm} height={fieldMm} strokeWidth={px(1.5)} />

          {stock && (
            <path
              className={`stock${selection.stock ? " selected" : ""}`}
              d={toPathData(stock.cmds, liveStock ? mul(liveStock, stock.transform) : stock.transform)}
              strokeWidth={px(selection.stock ? 2 : 1.5)}
              strokeDasharray={`${px(6)} ${px(3)}`}
            />
          )}

          <g className={showPreview ? "objects dimmed" : "objects"}>
            {fills.map(({ layer, d }) => (
              <path key={`fill-${layer.id}`} className="obj-fill" d={d} fill={layer.color} fillOpacity={layer.output ? 0.35 : 0.15} fillRule="evenodd" />
            ))}
            {doc?.objects.map((o) => {
              const layer = layerById.get(o.layer);
              if (layer && !layer.visible) return null;
              const d = paths.get(o.id) ?? "";
              const sel = selectedSet.has(o.id);
              const moving = sel && !!liveM;
              const out = outside.has(o.id);
              const fillHere = moving && viewStyle === "filled" && layer && layer.settings.mode !== "line" && o.geometry.kind !== "image";
              return (
                <g key={o.id} data-oid={o.id} transform={moving ? liveM : undefined} className={layer && !layer.output ? "no-output" : undefined}>
                  {o.geometry.kind === "image" && <ImageObject g={o.geometry} t={o.transform} />}
                  {fillHere && <path className="obj-fill" d={d} fill={layer.color} fillOpacity={0.35} fillRule="evenodd" />}
                  <path className="obj" d={d} stroke={layer?.color ?? "#333"} strokeWidth={px(1.3)} />
                  {sel && <path className="obj-selected" d={d} strokeWidth={px(1.6)} strokeDasharray={`${px(5)} ${px(4)}`} />}
                  {out && <path className="obj-outside" d={d} strokeWidth={px(3)} />}
                </g>
              );
            })}
          </g>

          {previewPaths && (
            <g className="toolpath" pointerEvents="none">
              {previewPaths.jumps && <path className="tp-jump" d={previewPaths.jumps} strokeWidth={px(0.8)} strokeDasharray={`${px(4)} ${px(3)}`} />}
              {previewPaths.marks.map(([layerId, d]) => {
                const l = layerById.get(layerId);
                const shade = previewOptions.shade;
                return (
                  <path
                    key={layerId}
                    className="tp-mark"
                    d={d}
                    stroke={shade ? "#111" : (l?.color ?? "#d22")}
                    strokeOpacity={shade ? energyOpacity(layerEnergy(l?.settings), previewPaths.maxE) : 1}
                    strokeWidth={px(1.2)}
                  />
                );
              })}
              {previewPaths.head && <circle className="tp-head" cx={previewPaths.head[0]} cy={previewPaths.head[1]} r={px(4)} strokeWidth={px(1.5)} />}
            </g>
          )}

          {selectedGroups.map((b, i) => (
            <rect
              key={`grp-${i}`}
              className="group-bounds"
              x={b.x0 - px(3)}
              y={b.y0 - px(3)}
              width={b.x1 - b.x0 + px(6)}
              height={b.y1 - b.y0 + px(6)}
              strokeWidth={px(1.2)}
              strokeDasharray={`${px(8)} ${px(3)} ${px(1.5)} ${px(3)} ${px(1.5)} ${px(3)}`}
              pointerEvents="none"
            />
          ))}

          {showHandles && selBounds && (
            <g className="sel-bounds">
              <rect
                className="sel-box"
                x={selBounds.x0 - px(6)}
                y={selBounds.y0 - px(6)}
                width={selBounds.x1 - selBounds.x0 + px(12)}
                height={selBounds.y1 - selBounds.y0 + px(12)}
                strokeWidth={px(1)}
                pointerEvents="none"
              />
              {!live &&
                (["rot-nw", "rot-ne", "rot-se", "rot-sw"] as const).map((h) => {
                  const p = handlePoint(pad(selBounds, px(6)), h.slice(4) as Handle);
                  const o = px(14);
                  const cx = p[0] + (h.endsWith("e") ? o : -o);
                  const cy = p[1] + (h.includes("n") ? o : -o);
                  return (
                    <g key={h} data-handle={h} className="rot-handle">
                      <circle cx={cx} cy={cy} r={px(9)} className="handle-hit" />
                      <circle cx={cx} cy={cy} r={px(4.5)} strokeWidth={px(1.2)} />
                    </g>
                  );
                })}
              {!live &&
                HANDLES.map((h) => {
                  const [x, y] = handlePoint(pad(selBounds, px(6)), h);
                  return (
                    <g key={h} data-handle={h} className={`scale-handle h-${h}`}>
                      <rect x={x - px(8)} y={y - px(8)} width={px(16)} height={px(16)} className="handle-hit" />
                      <rect x={x - px(4)} y={y - px(4)} width={px(8)} height={px(8)} strokeWidth={px(1)} className="handle" />
                    </g>
                  );
                })}
              <circle className="sel-center" cx={(selBounds.x0 + selBounds.x1) / 2} cy={(selBounds.y0 + selBounds.y1) / 2} r={px(2.5)} pointerEvents="none" />
            </g>
          )}

          {snapShown?.guides.map((g, i) => (
            <path key={`guide-${i}`} className="snap-guide" d={`M${g.a[0]},${g.a[1]}L${g.b[0]},${g.b[1]}`} strokeWidth={px(1)} strokeDasharray={`${px(3)} ${px(3)}`} />
          ))}
          {snapShown?.target && (
            <g className={`snap-marker snap-${snapShown.target.kind}`} pointerEvents="none">
              {snapShown.target.kind === "center" ? (
                <circle cx={snapShown.target.p[0]} cy={snapShown.target.p[1]} r={px(6)} strokeWidth={px(1.6)} />
              ) : snapShown.target.kind === "midpoint" ? (
                <path
                  d={`M${snapShown.target.p[0]},${snapShown.target.p[1] + px(7)}L${snapShown.target.p[0] + px(7)},${snapShown.target.p[1] - px(5)}L${snapShown.target.p[0] - px(7)},${snapShown.target.p[1] - px(5)}Z`}
                  strokeWidth={px(1.6)}
                />
              ) : (
                <rect x={snapShown.target.p[0] - px(6)} y={snapShown.target.p[1] - px(6)} width={px(12)} height={px(12)} strokeWidth={px(1.6)} />
              )}
            </g>
          )}

          {marqueeBox && (
            <rect
              className={`marquee ${crossing ? "crossing" : "enclosing"}`}
              x={marqueeBox.x0}
              y={marqueeBox.y0}
              width={marqueeBox.x1 - marqueeBox.x0}
              height={marqueeBox.y1 - marqueeBox.y0}
              strokeWidth={px(1.2)}
              strokeDasharray={crossing ? `${px(5)} ${px(3)}` : undefined}
            />
          )}

          {drawBox &&
            draw &&
            (draw.tool === "ellipse" ? (
              <ellipse
                className="rubber"
                cx={(drawBox.x0 + drawBox.x1) / 2}
                cy={(drawBox.y0 + drawBox.y1) / 2}
                rx={(drawBox.x1 - drawBox.x0) / 2}
                ry={(drawBox.y1 - drawBox.y0) / 2}
                strokeWidth={px(1.2)}
              />
            ) : (
              <rect className="rubber" x={drawBox.x0} y={drawBox.y0} width={drawBox.x1 - drawBox.x0} height={drawBox.y1 - drawBox.y0} strokeWidth={px(1.2)} />
            ))}
        </g>
      </svg>

      <div className="vp-overlay vp-top-right">
        <button className="icon-btn" title="Fit field" aria-label="Fit field" onClick={fitField}>
          <Icon name="fit" />
        </button>
        <button
          className="icon-btn"
          title="Zoom to selection"
          aria-label="Zoom to selection"
          disabled={!selBounds && !stockBox}
          onClick={() => {
            const b = selBounds ?? stockBox;
            if (b) {
              autoFit.current = false;
              fitBox(pad(b, 3));
            }
          }}
        >
          <Icon name="place" />
        </button>
      </div>
      {outsideCount > 0 && (
        <div className="vp-overlay vp-warn" role="status">
          {outsideCount} object{outsideCount === 1 ? "" : "s"} outside the galvo field (red). The laser cannot reach {outsideCount === 1 ? "it" : "them"}.
        </div>
      )}
      <div className="vp-overlay vp-bottom-left">
        {cursor ? `X ${cursor[0].toFixed(2)}  Y ${cursor[1].toFixed(2)} mm` : `Field ${fieldMm} mm`}
        {drawBox && `  ·  ${(drawBox.x1 - drawBox.x0).toFixed(2)} × ${(drawBox.y1 - drawBox.y0).toFixed(2)} mm`}
        {readout && `  ·  ${readout}`}
      </div>
      {tool !== "select" && (
        <div className="vp-overlay vp-top-left hint">
          Drag to draw {tool === "rect" ? "a rectangle" : "an ellipse"} · tap for 20 mm · Esc to cancel
        </div>
      )}
    </div>
  );
}

/** A bitmap drawn in its object frame (centred on the local origin; SVG images are y-down, the field is y-up). */
function ImageObject({ g, t }: { g: { width_mm: number; height_mm: number; mime: string; data_base64: string }; t: Affine }) {
  const href = useMemo(() => `data:${g.mime};base64,${g.data_base64}`, [g.mime, g.data_base64]);
  return (
    <g transform={`matrix(${t.join(",")})`} pointerEvents="none">
      <image
        href={href}
        x={-g.width_mm / 2}
        y={-g.height_mm / 2}
        width={g.width_mm}
        height={g.height_mm}
        preserveAspectRatio="none"
        transform="scale(1,-1)"
        className="obj-image"
      />
    </g>
  );
}

/** Where the dragged handle is: its start point moved by the pointer's travel (the grab is a few px off the handle). */
function handleTarget(g: { box: Box; handle: Handle; start: Vec2 }, f: Vec2): Vec2 {
  const h = handlePoint(g.box, g.handle);
  return [h[0] + f[0] - g.start[0], h[1] + f[1] - g.start[1]];
}

/** Whether field point `p` is inside the stock outline. */
function insideStock(p: Vec2, s: NonNullable<GDocument["stock"]>): boolean {
  const r = (-s.rotation_deg * Math.PI) / 180;
  const dx = p[0] - s.center[0];
  const dy = p[1] - s.center[1];
  const x = Math.cos(r) * dx - Math.sin(r) * dy;
  const y = Math.sin(r) * dx + Math.cos(r) * dy;
  if (s.shape.kind === "circle") return Math.hypot(x, y) <= s.shape.diameter_mm / 2;
  return Math.abs(x) <= s.shape.width_mm / 2 && Math.abs(y) <= s.shape.height_mm / 2;
}

/** Bounds of a box mapped through an affine. */
function mapBox(b: Box, m: Affine): Box {
  const pts: Vec2[] = [
    [b.x0, b.y0],
    [b.x1, b.y0],
    [b.x1, b.y1],
    [b.x0, b.y1],
  ];
  const q = pts.map((p) => [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]] as Vec2);
  return {
    x0: Math.min(...q.map((p) => p[0])),
    y0: Math.min(...q.map((p) => p[1])),
    x1: Math.max(...q.map((p) => p[0])),
    y1: Math.max(...q.map((p) => p[1])),
  };
}

/** Zoom `v` by `factor`, keeping the field point under screen point `s` fixed. */
function zoomAtPure(v: View, size: Vec2, s: Vec2, factor: number): View {
  const scale = clamp(v.scale * factor, MIN_SCALE, MAX_SCALE);
  const fx = (s[0] - size[0] / 2) / v.scale + v.cx;
  const fy = -(s[1] - size[1] / 2) / v.scale + v.cy;
  return { scale, cx: fx - (s[0] - size[0] / 2) / scale, cy: fy + (s[1] - size[1] / 2) / scale };
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}
function dist(a: Vec2, b: Vec2): number {
  return Math.hypot(a[0] - b[0], a[1] - b[1]);
}
function mid(a: Vec2, b: Vec2): Vec2 {
  return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
}
function round6(v: number): number {
  return Math.round(v * 1e6) / 1e6;
}
function r4(v: number): number {
  return Math.round(v * 1e4) / 1e4;
}
function pad(b: Box, m: number): Box {
  return { x0: b.x0 - m, y0: b.y0 - m, x1: b.x1 + m, y1: b.y1 + m };
}
