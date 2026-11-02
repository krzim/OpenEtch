// The centre canvas, in field millimetres with +Y up, drawn as stacked layers that each carry the same view transform
// (pan/zoom and the y-flip): the field, grid, axes and stock (2D canvas, vectorScene.ts paintBack), the canvas tool's
// underlay (SVG), images (canvas), object outlines and fills (WebGL2, VectorLayer.tsx), the toolpath preview
// (WebGL2), and on top the interaction SVG (handles, marquee, rubber bands, the canvas tool, snap glyphs). Every
// scene layer is raster, repainted for each view from caches built per document or preview, never per view
// (vectorScene.ts, sceneLayers.ts, components/SceneCanvas.tsx): a zoom or pan costs a GPU redraw of a few
// milliseconds, never a re-rasterization of SVG (which showed blurry low-resolution tiles while zooming). Wheel,
// pinch and pan updates are coalesced to one per animation frame, and the layers repaint in that frame's commit, so
// they stay in step with the interaction SVG. Hit-testing is geometric (selection.ts hitTest), never by element.
// Overlay strokes are divided by the zoom so they stay a constant number of screen px.
//
// Interaction uses Pointer Events only (mouse, pen and touch alike), following LightBurn:
// - click an object (its line, or inside a closed shape) to select it; Shift adds, Ctrl toggles, Ctrl+Shift removes;
//   a group selects as a whole;
// - drag on empty space draws a selection box: left to right encloses (solid), right to left crosses (dashed green);
// - drag a selected object to move it (Shift constrains to H/V/45°, Ctrl turns snapping off, Alt snaps to alignment
//   guides only); drag the corner/edge handles to scale (Shift frees the aspect, Ctrl scales about the centre; a lone
//   circle stays round while "Keep round" is on) and the round handles outside the corners to rotate (Shift 15°,
//   Ctrl 5°); Esc before release cancels;
// - drag the stock to move it while it is selected (click its outline to select it);
// - middle-drag or Space+drag pans, the wheel zooms (Ctrl+wheel is a trackpad pinch, sideways and Shift+wheel
//   scrolling pans; with Preferences > Trackpad on, the default on a Mac, two-finger scrolling pans), and the pan-or-zoom
//   decision holds for a whole wheel stream (touch/wheelIntent.ts); Safari's pinch gesture events zoom the canvas;
// - touch (docs/ux/ui-plan-2026-11.md 1.2, touch/gesture.ts): one finger is the left mouse button (a drag on empty
//   space draws the same marquee; Preferences can make it pan instead), two fingers pan and pinch, a second finger
//   cancels a drag, a long-press is the right-click, two- and three-finger taps undo and redo, and while a pen draws
//   a resting palm is ignored. Tolerances, handles and snapping follow the pointer (pointer.ts TOL); a tap on a
//   selected object switches the touch handles between scale and rotate (LightBurn's click-again rotate). The touch
//   modifier bar's virtual keys (touch/virtualMods.ts) count as held keys;
// - with the Rectangle or Ellipse tool, drag to draw (Shift: square or circle); the Circle tool draws from the centre
//   (a tap places a 20 mm shape);
// - snapping is Fusion 360-style on real geometry (snapping.ts): centres, quadrants, vertices, midpoints,
//   intersections and the nearest point on outlines, each with its own glyph and a label, plus dotted alignment
//   guides and the grid. A dragged selection snaps by its own snap point nearest to where it was grabbed.
// - while Shift constrains a drag (moves, node and segment drags, line/pen/measure angles), a guide line along the
//   locked axis or angle crosses the canvas through the drag's start (LightBurn), in screen px like the snap overlay.
// Every edit is a command sent on pointerup; until the engine answers, the change is drawn as a transient transform.

import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { flushSync } from "react-dom";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { createdIds } from "../api/drop.ts";
import type { Affine, GDocument, Layer, Preview, Vec2 } from "../api/types.ts";
import type { CanvasCtx, CanvasTool, CanvasToolId, RenderCtx } from "../canvasTool.ts";
import { circleLocked } from "../circleLock.ts";
import { dragCorners } from "../drawTools.ts";
import {
  HANDLES,
  constrainDelta,
  constraintLine,
  constraintSegment,
  handlePoint,
  keepRound,
  rotateAbout,
  rotateFromHandle,
  roundMm,
  scaleAbout,
  scaleFromHandle,
  signedScaleParams,
  type ConstraintLine,
  type Handle,
  type Mods,
} from "../editing.ts";
import {
  boxCenter,
  boxFromPoints,
  cmdsBounds,
  distToPolylines,
  flattenCmds,
  mul,
  stockCmds,
  unionBoxes,
  type Box,
} from "../geometry.ts";
import {
  anyGrouped,
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
import {
  TOL,
  capturePointer,
  isApplePlatform,
  isMacContextClick,
  modifierReplay,
  modsOf as modsOfKeys,
  pointerClass,
  type KeyMods,
  type PointerClass,
} from "../pointer.ts";
import { GestureArbiter, type Effect } from "../touch/gesture.ts";
import { publishHud } from "../touch/hudState.ts";
import { runTouchAction } from "../touch/actions.ts";
import { clearLatched, getVirtualMods, subscribeVirtualMods } from "../touch/virtualMods.ts";
import { newWheelLatch, wheelIntent, wheelPanDelta } from "../touch/wheelIntent.ts";
import { ContextStripPortal } from "../workspace/contextStrip.tsx";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import "../touch/touch.css";
import { useDitheredHref } from "./ImageSettings.tsx";
import { CanvasLayer, ToolpathLayer, originalHref, paintImages, useBitmaps, useDpr, type ImageItem, type PaintArgs } from "./SceneCanvas.tsx";
import { maskClips } from "../files/fileFormats.ts";
import { buildToolpath, frameCoalescer, type ToolpathStyle, type View } from "../sceneLayers.ts";
import { buildVectorScene, paintBack, segmentFlags } from "../vectorScene.ts";
import { VectorLayer, useCanvasTokens } from "./VectorLayer.tsx";
import {
  SNAP_TOL_PX,
  buildSnapIndex,
  fieldSnaps,
  findSnap,
  isRound,
  objectSnaps,
  referencePoint,
  snapLabel,
  stockSnaps,
  type SnapHit,
  type SnapIndex,
  type SnapKind,
  type SnapOptions,
  type SnapPart,
  type SnapPoint,
} from "../snapping.ts";
import { Icon } from "./ui.tsx";
import { TOOLPATH_INK } from "../workspace/theme.ts";
import { usePrefs, useResolvedTheme } from "../workspace/usePrefs.ts";
import { fixedLength, fixedNumber, formatLength, lengthDigits, lengthToDisplay, trimNumber } from "../workspace/units.ts";
import { onRevealSelection, onViewCommand, publishViewStatus, revealShift } from "../workspace/viewStatus.ts";
import { useT } from "../workspace/usePrefs.ts";
import { HeadMarker } from "./HeadMarker.tsx";

export type Tool = "select" | "rect" | "ellipse" | "circle" | CanvasToolId;

/** Tools that draw a shape here (the canvas tools in components/EditTools.tsx draw lines, pens and polygons). */
type DrawTool = "rect" | "ellipse" | "circle";
const isDrawTool = (t: Tool): t is DrawTool => t === "rect" || t === "ellipse" || t === "circle";

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
  /** Passes (main settings or sub-layers, by `passKey`) hidden from the preview display. */
  hiddenPasses: ReadonlySet<string>;
  /** Draw only the first N segments (draw-order animation), or null for all. */
  upTo: number | null;
  /** Shade marks by energy (darker = more) instead of the layer colour (each pass of a layer its own shade). */
  shade: boolean;
  /** Show travel moves. */
  jumps: boolean;
}

type Gesture =
  | { kind: "none" }
  | { kind: "pan"; start: Vec2; view: View; moved: boolean; clearOnTap: boolean; slop: number }
  | { kind: "marquee"; start: Vec2; startScreen: Vec2; op: SelectOp; moved: boolean; slop: number }
  | {
      kind: "move";
      startField: Vec2;
      startScreen: Vec2;
      moved: boolean;
      ids: string[];
      stock: boolean;
      /** The selection's own snap point that snaps (nearest to the grab position). */
      ref: SnapPoint;
      /** Snap candidates without the moving things. */
      index: SnapIndex;
      /** Selection to narrow to when this turns out to be a click without a drag. */
      tapSelect: string | null;
      /** Screen px before the press becomes a drag (the pointer's slop). */
      slop: number;
      /** A touch tap on an object that was already selected: switches the handles between scale and rotate. */
      rotToggle: boolean;
    }
  /** `index`: snap candidates without the selection; `target`: the dragged handle's snapped position (smart guides). */
  | { kind: "scale"; handle: Handle; box: Box; ids: string[]; start: Vec2; round: boolean; index: SnapIndex; target: Vec2 | null }
  | { kind: "rotate"; center: Vec2; start: Vec2; ids: string[] }
  | { kind: "draw"; start: Vec2; tool: DrawTool; index: SnapIndex; slop: number }
  | { kind: "pinch"; dist: number; mid: Vec2; view: View }
  /** A gesture the canvas tool (EditTools) took. */
  | { kind: "ext" }
  | { kind: "idle" };

/** A transform drawn on top of the document until the engine answers. */
interface Transient {
  m: Affine;
  stock: boolean;
}

const MIN_SCALE = 0.2;
const MAX_SCALE = 400;
// Screen px a pointer may wander before a tap becomes a drag, and the click tolerance (LightBurn "Click Selection
// Tolerance"), per pointer class: pointer.ts TOL (the mouse row is the old TAP_SLOP 4 and CLICK_TOL_PX 5).
/** Default size of a shape placed with a tap, mm. */
const TAP_SIZE_MM = 20;

export interface ViewportProps {
  doc: GDocument | null;
  /**
   * The active device's workspace (W x H mm), centred on the field origin: a galvo's square field, or a gantry
   * device's bed (LightBurn coordinates minus W/2, H/2).
   */
  workspaceMm: Vec2;
  selection: Selection;
  tool: Tool;
  /** Layer that new shapes go on. */
  activeLayer: string | null;
  preview: Preview | null;
  showPreview: boolean;
  previewOptions: PreviewOptions;
  viewStyle: ViewStyle;
  snap: SnapSettings;
  /** Objects outside the workspace, drawn in red. */
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
  /** Node editing, line/pen/polygon, measure, trim, picks, rulers and guides (components/EditTools.tsx). */
  canvasTool?: CanvasTool | null;
}

export function Viewport(props: ViewportProps) {
  const { doc, workspaceMm, selection, tool, preview, showPreview, previewOptions, viewStyle, snap, outside } = props;
  const [wsW, wsH] = workspaceMm;
  const galvo = Math.abs(wsW - wsH) < 1e-3;
  // Display preferences (workspace/prefs.ts): grid, wheel direction, snap distance and kinds, crosshair; and the theme.
  const uiPrefs = usePrefs();
  const t = useT();
  /** Display length unit for readouts (fields convert on their own). */
  const lu = uiPrefs.units;
  const theme = useResolvedTheme();
  const wheelSign = useRef(1);
  wheelSign.current = uiPrefs.invertWheel ? -1 : 1;
  const wrapRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const [size, setSize] = useState<Vec2>([1, 1]);
  const [view, setViewState] = useState<View>({ cx: 0, cy: 0, scale: 3 });
  /** The newest view asked for; the drawn one (`view`) catches up once per animation frame. */
  const viewRef = useRef<View>(view);
  const frames = useRef<ReturnType<typeof frameCoalescer<View>> | null>(null);
  frames.current ??= frameCoalescer<View>(
    (cb) => requestAnimationFrame(cb),
    (v) => flushSync(() => setViewState(v)),
  );
  /** Pan/zoom from wheel, pinch, drags and hotkeys: composes with updates not drawn yet, drawn on the next frame. */
  const queueView = useCallback((next: View | ((v: View) => View)) => {
    viewRef.current = typeof next === "function" ? next(viewRef.current) : next;
    frames.current?.push(viewRef.current);
  }, []);
  /** A coalesced view update waits for the next frame (the vector layer's ants then leave that frame's draw to it). */
  const viewPending = useCallback(() => frames.current?.pending() ?? false, []);
  /** Set the view now (fit, zoom to selection). */
  const setView = useCallback((v: View) => {
    viewRef.current = v;
    setViewState(v);
  }, []);
  const dpr = useDpr();
  const [cursor, setCursor] = useState<Vec2 | null>(null);
  const [transient, setTransient] = useState<Transient | null>(null);
  const [draw, setDraw] = useState<{ a: Vec2; b: Vec2; tool: DrawTool } | null>(null);
  const [marquee, setMarquee] = useState<{ a: Vec2; b: Vec2 } | null>(null);
  const [snapShown, setSnapShown] = useState<SnapHit | null>(null);
  /** The Shift constraint guide of the current drag, or null. */
  const [constraint, setConstraint] = useState<ConstraintLine | null>(null);
  const [readout, setReadout] = useState<string | null>(null);
  /** Transient kept on screen after pointerup until the document revision changes, so nothing snaps back. */
  const [settling, setSettling] = useState<(Transient & { rev: number }) | null>(null);
  const pointers = useRef(new Map<number, Vec2>());
  const gesture = useRef<Gesture>({ kind: "none" });
  /** Last pointer position and modifiers, so pressing or releasing a modifier mid-drag updates the drag. */
  const lastMove = useRef<{ s: Vec2; m: Mods } | null>(null);
  /** The last hover position (screen px, mouse or pen, no button down), or null off the canvas. */
  const lastHover = useRef<Vec2 | null>(null);
  /** The gesture arbiter: touch presses, pinch, long-press, palm rejection (touch/gesture.ts). */
  const arbRef = useRef<GestureArbiter | null>(null);
  arbRef.current ??= new GestureArbiter();
  const arb = arbRef.current;
  /** Per pointer: what its down event said (with touch the press starts later, past the slop or on lift). */
  const downInfo = useRef(new Map<number, { target: Element | null; button: number; time: number; cls: PointerClass }>());
  /** The keyboard modifiers of the last pointer event. */
  const lastKeys = useRef<KeyMods>({ shiftKey: false, ctrlKey: false, metaKey: false, altKey: false });
  /** Pointer class of the gesture in progress (tolerances, snapping). */
  const clsRef = useRef<PointerClass>("mouse");
  const layout = useLayoutMode();
  /** The pointer class the handles are drawn for: the last one used on the canvas (a coarse screen starts with touch). */
  const [lastCls, setLastCls] = useState<PointerClass>(() => (layout.pointer === "coarse" ? "touch" : "mouse"));
  // Preferences > Density = Touch asks for touch-sized handles (and tap-again rotate) whatever the pointer.
  const touchDensity = layout.density === "touch";
  const handleCls: PointerClass = lastCls === "mouse" && touchDensity ? "touch" : lastCls;
  /** Touch handles show the rotate handles instead of the scale handles (a tap on the selection toggles). */
  const [rotMode, setRotMode] = useState(false);
  /** The long-press ring under a resting finger, screen px. */
  const [lpRing, setLpRing] = useState<Vec2 | null>(null);
  const lpTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Space is held: a drag pans (grab cursor). */
  const [spacePan, setSpacePan] = useState(false);
  const spaceRef = useRef(false);
  const wheelLatch = useRef(newWheelLatch());
  const trackpadRef = useRef(uiPrefs.trackpadPans);
  trackpadRef.current = uiPrefs.trackpadPans;
  /** Safari's pinch (gesturestart) is under way: its own events zoom, so the wheel stays out of it. */
  const safariPinch = useRef<{ view: View; at: Vec2 } | null>(null);
  const apple = useMemo(() => isApplePlatform(), []);
  /** Time of the last touch press, so the browser's own touch long-press menu does not open a second menu. */
  const lastTouchT = useRef(-Infinity);
  /** The view follows the field (refit on resize) until the user pans or zooms. */
  const autoFit = useRef(true);
  const fittedField = useRef<string | null>(null);

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
    [size, setView],
  );
  const fieldBox = useMemo<Box>(() => ({ x0: -wsW / 2, y0: -wsH / 2, x1: wsW / 2, y1: wsH / 2 }), [wsW, wsH]);
  const fitField = useCallback(() => {
    autoFit.current = true;
    fitBox(fieldBox);
  }, [fitBox, fieldBox]);

  // Fit the field once the canvas has a real size, again when the field size changes, and on every resize until the
  // user pans or zooms (so the first fit never leaves the field clipped). After that a resize (a dock opening or
  // closing, a rotation) keeps the view centre, and when the whole field was in view it stays in view (V9).
  const prevSize = useRef<Vec2 | null>(null);
  useEffect(() => {
    if (size[0] <= 20 || size[1] <= 20) return;
    const before = prevSize.current;
    prevSize.current = size;
    // The workspace changes with the active device: fit the new one.
    const key = `${wsW}x${wsH}`;
    if (autoFit.current || fittedField.current !== key) {
      fittedField.current = key;
      fitBox(fieldBox);
      return;
    }
    if (!before || (before[0] === size[0] && before[1] === size[1])) return;
    const v = viewRef.current;
    const fits = (sz: Vec2, sc: number) => Math.abs(v.cx) + wsW / 2 <= sz[0] / 2 / sc + 1e-6 && Math.abs(v.cy) + wsH / 2 <= sz[1] / 2 / sc + 1e-6;
    if (fits(before, v.scale) && !fits(size, v.scale)) {
      const need = Math.min(size[0] / 2 / (Math.abs(v.cx) + wsW / 2), size[1] / 2 / (Math.abs(v.cy) + wsH / 2));
      setView({ ...v, scale: clamp(need, MIN_SCALE, MAX_SCALE) });
    }
  }, [size, fitBox, fieldBox, wsW, wsH, setView]);

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

  // Wheel zooms about the pointer; Ctrl+wheel is a trackpad pinch; sideways and Shift+wheel scrolling pan, and with
  // Preferences > Trackpad on (Auto: a Mac, or once a trackpad was seen) two-finger scrolling pans. The decision holds
  // for the whole wheel stream (touch/wheelIntent.ts). Safari sends its trackpad pinch as gesture events instead.
  // Non-passive listeners keep the page from scrolling or zooming.
  useEffect(() => {
    const el = svgRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (safariPinch.current) return;
      autoFit.current = false;
      const s = local(e);
      const intent = wheelIntent(wheelLatch.current, e as WheelEvent & { wheelDeltaY?: number }, trackpadRef.current, apple);
      if (intent === "pan") {
        const [dx, dy] = wheelPanDelta(e);
        queueView((v) => ({ ...v, cx: v.cx + dx / v.scale, cy: v.cy - dy / v.scale }));
        return;
      }
      const pixel = e.deltaMode === 0;
      const dy = (pixel ? e.deltaY : e.deltaY * 40) * (e.ctrlKey ? 1 : wheelSign.current);
      const factor = Math.exp(-clamp(dy, -300, 300) * (e.ctrlKey ? 0.01 : 0.0015));
      queueView((v) => zoomAtPure(v, size, s, factor));
    };
    type GestureEv = Event & { scale?: number; clientX?: number; clientY?: number };
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      const g = e as GestureEv;
      autoFit.current = false;
      safariPinch.current = { view: viewRef.current, at: local({ clientX: g.clientX ?? 0, clientY: g.clientY ?? 0 }) };
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const p = safariPinch.current;
      const k = (e as GestureEv).scale;
      if (!p || typeof k !== "number" || !(k > 0)) return;
      queueView(zoomAtPure(p.view, size, p.at, k));
    };
    const onGestureEnd = (e: Event) => {
      e.preventDefault();
      safariPinch.current = null;
    };
    const opts = { passive: false } as AddEventListenerOptions;
    el.addEventListener("wheel", onWheel, opts);
    el.addEventListener("gesturestart", onGestureStart, opts);
    el.addEventListener("gesturechange", onGestureChange, opts);
    el.addEventListener("gestureend", onGestureEnd, opts);
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("gesturestart", onGestureStart);
      el.removeEventListener("gesturechange", onGestureChange);
      el.removeEventListener("gestureend", onGestureEnd);
    };
  }, [size, queueView, apple]);

  // Outside the canvas, Ctrl+wheel and pinch gestures must not zoom the page (the panels and bars would scale).
  useEffect(() => {
    const outside = (t: EventTarget | null) => !(t instanceof Element && t.closest(".viewport"));
    const onWheel = (e: WheelEvent) => {
      if (e.ctrlKey && outside(e.target)) e.preventDefault();
    };
    const onGesture = (e: Event) => {
      if (outside(e.target)) e.preventDefault();
    };
    const opts = { passive: false } as AddEventListenerOptions;
    window.addEventListener("wheel", onWheel, opts);
    document.addEventListener("gesturestart", onGesture, opts);
    document.addEventListener("gesturechange", onGesture, opts);
    return () => {
      window.removeEventListener("wheel", onWheel);
      document.removeEventListener("gesturestart", onGesture);
      document.removeEventListener("gesturechange", onGesture);
    };
  }, []);

  // Space held with the canvas focused: a left drag pans (Space+drag, as in most design apps).
  useEffect(() => {
    const typing = (t: EventTarget | null) => {
      const el = t as HTMLElement | null;
      return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || !!el.isContentEditable);
    };
    const canvasKeys = () => {
      const a = document.activeElement;
      return !a || a === document.body || !!a.closest?.(".viewport");
    };
    const set = (on: boolean) => {
      if (spaceRef.current === on) return;
      spaceRef.current = on;
      setSpacePan(on);
    };
    const down = (e: KeyboardEvent) => {
      if (e.code !== "Space" || e.ctrlKey || e.metaKey || e.altKey || typing(e.target) || !canvasKeys()) return;
      e.preventDefault();
      set(true);
    };
    const up = (e: KeyboardEvent) => {
      if (e.code === "Space") set(false);
    };
    const blur = () => set(false);
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", blur);
    };
  }, []);

  const selectedSet = useMemo(() => new Set(selection.objects), [selection.objects]);
  const layerById = useMemo(() => new Map((doc?.layers ?? []).map((l) => [l.id, l])), [doc]);
  const shapeIndex = useMemo(() => (doc ? buildShapeIndex(doc) : []), [doc]);
  const boundsById = useMemo(() => new Map(shapeIndex.map((s) => [s.id, s.box])), [shapeIndex]);

  const live = transient ?? settling;
  const liveObjects = live && !live.stock ? live.m : null;
  const liveStock = live && live.stock ? live.m : null;

  const selBoundsRaw = useMemo(() => unionBoxes(selection.objects.map((id) => boundsById.get(id) ?? null)), [selection.objects, boundsById]);
  const selBounds = useMemo(() => (selBoundsRaw && liveObjects ? mapBox(selBoundsRaw, liveObjects) : selBoundsRaw), [selBoundsRaw, liveObjects]);

  const docStock = doc?.stock ?? null;
  const stock = useMemo(() => (docStock ? stockCmds(docStock) : null), [docStock]);
  const stockBox = useMemo(() => (stock ? cmdsBounds(stock.cmds, stock.transform) : null), [stock]);

  /** Snap data per visible object and the stock, built lazily on the first snap after the document changes. */
  const extParts = props.canvasTool?.snapParts;
  const snapCache = useRef<{ doc: GDocument | null; field: Box; ext: unknown; parts: Map<string, SnapPart>; stock: SnapPart | null; all: SnapIndex | null } | null>(null);
  const snapData = () => {
    let c = snapCache.current;
    if (!c || c.doc !== doc || c.field !== fieldBox || c.ext !== extParts) {
      const parts = new Map<string, SnapPart>();
      if (doc) {
        const visible = new Set(shapeIndex.map((s) => s.id));
        for (const o of doc.objects) if (visible.has(o.id)) parts.set(o.id, objectSnaps(o));
      }
      c = { doc, field: fieldBox, ext: extParts, parts, stock: doc?.stock ? stockSnaps(doc.stock) : null, all: null };
      snapCache.current = c;
    }
    return c;
  };

  /** Snap index over everything but `exclude` (and the stock unless `includeStock`), plus the field points. */
  const snapIndexFor = (exclude: ReadonlySet<string>, includeStock: boolean): SnapIndex => {
    const c = snapData();
    if (exclude.size === 0 && includeStock && c.all) return c.all;
    const parts: SnapPart[] = [];
    for (const [id, p] of c.parts) if (!exclude.has(id)) parts.push(p);
    if (includeStock && c.stock) parts.push(c.stock);
    parts.push(fieldSnaps(fieldBox));
    for (const p of extParts ?? []) parts.push(p);
    const ix = buildSnapIndex(parts);
    if (exclude.size === 0 && includeStock) c.all = ix;
    return ix;
  };

  const snapOn = (m: Mods) => (m.ctrl ? !snap.objects : snap.objects);
  const gridMm = (m: Mods) => (snap.grid && !m.ctrl ? snap.gridMm : 0);

  /** Snap options for modifiers `m`: Ctrl turns snapping off (or on, when it is off), Alt snaps to guides only. */
  const snapOpts = (m: Mods, extra?: SnapPoint[]): SnapOptions => {
    const objects = snapOn(m);
    const guidesOnly = !!m.alt;
    const [x0, y1] = toField(0, 0);
    const [x1, y0] = toField(size[0], size[1]);
    // Touch and pen snap from farther away (pointer.ts TOL); the mouse keeps Preferences > Snap distance.
    const tolPx = Math.max(uiPrefs.snapTolPx || SNAP_TOL_PX, clsRef.current === "mouse" ? 0 : TOL[clsRef.current].snap);
    return {
      tol: tolPx / view.scale,
      kinds: uiPrefs.snapKinds,
      points: objects && !guidesOnly,
      nearest: objects && !guidesOnly,
      guides: objects,
      grid: guidesOnly ? 0 : gridMm(m),
      view: { x0, y0, x1, y1 },
      extra,
    };
  };

  /** A point snapped for drawing: snap points, outlines, guides, else the grid. */
  const snapPoint = (p: Vec2, index: SnapIndex, m: Mods, extra?: SnapPoint[]): { p: Vec2; hit: SnapHit | null } => {
    const hit = findSnap(index, p, snapOpts(m, extra));
    return hit ? { p: hit.p, hit } : { p, hit: null };
  };

  /** Guide sources while drawing: the circle's centre, so a radius can run level or plumb from it. */
  const drawExtra = (g: { start: Vec2; tool: DrawTool }): SnapPoint[] | undefined =>
    g.tool === "circle" ? [{ p: g.start, kind: "center", what: "Center", owner: "new circle", id: "draw" }] : undefined;

  /** Where the drag of a draw tool ends: snapped, and squared with Shift for rectangles and ellipses. */
  const drawEnd = (g: { start: Vec2; tool: DrawTool; index: SnapIndex }, f: Vec2, m: Mods): { a: Vec2; p: Vec2; hit: SnapHit | null } => {
    if (g.tool === "circle") return { a: g.start, ...snapPoint(f, g.index, m, drawExtra(g)) };
    // LightBurn: Ctrl draws from the centre (so it does not turn snapping off here), Shift makes it square.
    const sp = snapPoint(f, g.index, { ...m, ctrl: false }, drawExtra(g));
    const c = dragCorners(g.start, sp.p, { shift: m.shift, ctrl: m.ctrl });
    return { a: c.a, p: c.b, hit: m.shift ? null : sp.hit };
  };

  // Leaving a draw tool takes its hover snap glyph with it.
  useEffect(() => {
    if (tool === "select" && gesture.current.kind === "none") setSnapShown(null);
    setConstraint(null);
  }, [tool]);

  const extRef = useRef<CanvasTool | null | undefined>(null);
  extRef.current = props.canvasTool;
  const cancelGesture = useCallback(() => {
    const g = gesture.current;
    if (g.kind === "ext") extRef.current?.cancel();
    gesture.current = pointers.current.size ? { kind: "idle" } : { kind: "none" };
    setTransient(null);
    setDraw(null);
    setMarquee(null);
    setSnapShown(null);
    setConstraint(null);
    setReadout(null);
    return g.kind !== "none" && g.kind !== "idle";
  }, []);

  // Esc during a drag cancels it (and nothing else: the app's Esc must not also switch tools or clear the selection).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const g = gesture.current.kind;
      if (g === "none" || g === "idle" || g === "pinch" || g === "pan" || g === "ext") return;
      cancelGesture();
      e.preventDefault();
      e.stopImmediatePropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [cancelGesture]);

  // Pressing or releasing Shift/Ctrl/Alt mid-drag re-evaluates the drag at the last pointer position; with no button
  // down it re-evaluates the hover (the line tool's Shift angle and its guide line follow the key without a move).
  useEffect(() => {
    const onMod = (e: KeyboardEvent) => {
      const replay = modifierReplay(e.key, gesture.current.kind, lastHover.current !== null && pointers.current.size === 0);
      const m: Mods = modsOf(e);
      if (replay === "hover" && lastHover.current) {
        hoverAt(lastHover.current, m);
        return;
      }
      const lm = lastMove.current;
      if (replay !== "drag" || !lm) return;
      if (e.key === "Alt") e.preventDefault(); // keep Alt from focusing the browser menu mid-drag
      update(lm.s, m);
    };
    window.addEventListener("keydown", onMod);
    window.addEventListener("keyup", onMod);
    return () => {
      window.removeEventListener("keydown", onMod);
      window.removeEventListener("keyup", onMod);
    };
  });

  /** LightBurn's modifiers of an event: the keys OR the touch modifier bar (Cmd is Ctrl on a Mac; pointer.ts). */
  const modsOf = (e: KeyMods): Mods => modsOfKeys(e, getVirtualMods(), apple);
  const keysOf = (e: KeyMods): KeyMods => ({ shiftKey: e.shiftKey, ctrlKey: e.ctrlKey, metaKey: e.metaKey, altKey: e.altKey });

  /** Arm (or clear) the long-press timer for the arbiter's next deadline. */
  const armTimer = () => {
    if (lpTimer.current) clearTimeout(lpTimer.current);
    lpTimer.current = null;
    const due = arb.nextDeadline();
    if (due === null) return;
    lpTimer.current = setTimeout(() => {
      lpTimer.current = null;
      effectsRef.current(arb.feed({ kind: "tick", t: performance.now() }));
    }, Math.max(0, due - performance.now()));
  };
  useEffect(
    () => () => {
      if (lpTimer.current) clearTimeout(lpTimer.current);
      publishHud({ selRect: null, canvasRect: null, busy: false });
    },
    [],
  );

  const onPointerDown = (e: React.PointerEvent<SVGSVGElement>) => {
    if (!doc) return;
    const cls = pointerClass(e);
    const mouse = cls === "mouse";
    if (mouse && e.button === 2) return; // context menu
    if (isMacContextClick(e, apple)) return; // Ctrl+click on a Mac is the right-click: the context menu follows
    if (mouse && e.button !== 0 && e.button !== 1) return;
    e.preventDefault(); // no text selection, no middle-click autoscroll
    wrapRef.current?.focus({ preventScroll: true });
    const s = local(e);
    pointers.current.set(e.pointerId, s);
    capturePointer(svgRef.current, e.pointerId);
    if (cls === "touch") lastTouchT.current = performance.now();
    if (cls !== lastCls) setLastCls(cls);
    lastKeys.current = keysOf(e);
    downInfo.current.set(e.pointerId, { target: e.target as Element, button: e.button, time: e.timeStamp, cls });
    // The arbiter decides when the press starts: at once for a mouse or pen, past the slop or on lift for a finger.
    applyEffects(arb.feed({ kind: "down", id: e.pointerId, cls, p: s, t: e.timeStamp }));
    // A finger resting in the Select tool shows the long-press ring coming.
    if (arb.state === "pending" && tool === "select") setLpRing(s);
  };

  /** Start a one-pointer gesture at screen point `s` (the arbiter's press; the old pointerdown). */
  const beginPress = (id: number, s: Vec2, cls: PointerClass, longPress: boolean) => {
    if (!doc) return;
    const info = downInfo.current.get(id) ?? { target: null, button: 0, time: performance.now(), cls };
    clsRef.current = cls;
    const tl = TOL[cls];
    const m = modsOf(lastKeys.current);
    lastMove.current = { s, m };
    if (cls === "mouse" && (info.button === 1 || (spaceRef.current && info.button === 0))) {
      gesture.current = { kind: "pan", start: s, view: viewRef.current, moved: false, clearOnTap: false, slop: tl.slop };
      return;
    }
    const f = toField(s[0], s[1]);
    const ext = props.canvasTool;
    if (ext && ext.down(f, m, { target: info.target, pointerType: cls, time: info.time, screen: s, longPress }, canvasCtx())) {
      gesture.current = { kind: "ext" };
      return;
    }
    if (isDrawTool(tool)) {
      const index = snapIndexFor(new Set(), true);
      const sp = snapPoint(f, index, m).p;
      gesture.current = { kind: "draw", start: sp, tool, index, slop: tl.slop };
      setDraw({ a: sp, b: sp, tool });
      return;
    }

    // Handles of the current selection come first.
    const handleEl = info.target?.closest?.("[data-handle]");
    if (handleEl && selBoundsRaw) {
      const h = handleEl.getAttribute("data-handle") ?? "";
      if (h.startsWith("rot")) {
        gesture.current = { kind: "rotate", center: boxCenter(selBoundsRaw), start: f, ids: selection.objects };
      } else {
        const lone = selection.objects.length === 1 ? doc.objects.find((o) => o.id === selection.objects[0]) : undefined;
        const round = !!lone && isRound(lone) && circleLocked();
        gesture.current = { kind: "scale", handle: h as Handle, box: selBoundsRaw, ids: selection.objects, start: f, round, index: snapIndexFor(new Set(selection.objects), true), target: null };
      }
      return;
    }

    const tol = tl.click / view.scale;
    const hit = hitTest(shapeIndex, f, tol, cls !== "mouse");
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
      const parts = snapData().parts;
      const own = ids.flatMap((id) => parts.get(id)?.points ?? []);
      gesture.current = {
        kind: "move",
        startField: f,
        startScreen: s,
        moved: false,
        ids,
        stock: false,
        ref: referencePoint(own, f) ?? grabPoint(f),
        index: snapIndexFor(new Set(ids), true),
        tapSelect: op === "replace" && isSel && selection.objects.length > 1 ? hit : null,
        slop: tl.slop,
        rotToggle: (cls !== "mouse" || touchDensity) && op === "replace" && isSel,
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
          ref: referencePoint(snapData().stock?.points ?? [], f) ?? grabPoint(f),
          index: snapIndexFor(new Set(), false),
          tapSelect: null,
          slop: tl.slop,
          rotToggle: false,
        };
        return;
      }
    }

    if (cls === "touch" && uiPrefs.touchOneFinger === "pan") {
      // Preferences > One finger on empty canvas: Pan (the old touch behaviour); a tap clears the selection.
      gesture.current = { kind: "pan", start: s, view: viewRef.current, moved: false, clearOnTap: op === "replace", slop: tl.slop };
      return;
    }
    // Empty space: the selection box, the same for a finger as for the mouse.
    gesture.current = { kind: "marquee", start: f, startScreen: s, op, moved: false, slop: tl.slop };
  };

  /** Run the arbiter's effects (touch/gesture.ts). */
  const applyEffects = (fx: Effect[]) => {
    for (const ef of fx) {
      switch (ef.kind) {
        case "press":
          beginPress(ef.id, ef.p, ef.cls, !!ef.longPress);
          break;
        case "drag": {
          const m = modsOf(lastKeys.current);
          lastMove.current = { s: ef.p, m };
          update(ef.p, m);
          break;
        }
        case "release":
          releasePress(ef.p);
          break;
        case "abort":
          cancelGesture();
          break;
        case "navStart":
          startNav(ef.pts);
          break;
        case "nav":
          navTo(ef.pts);
          break;
        case "navEnd":
          gesture.current = pointers.current.size ? { kind: "idle" } : { kind: "none" };
          break;
        case "longPress":
          longPressAt(ef.id, ef.p);
          break;
        case "multiTap":
          // Two fingers undo, three redo (Procreate, Fusion on iPad).
          runTouchAction(ef.fingers >= 3 ? "redo" : "undo");
          break;
      }
    }
    if (arb.state !== "pending") setLpRing(null);
    const gk = gesture.current.kind;
    publishHud({ busy: gk !== "none" && gk !== "idle" });
    armTimer();
  };
  const effectsRef = useRef(applyEffects);
  effectsRef.current = applyEffects;

  /** Two fingers pinch and pan; one finger (palm mode, after a pen) pans. */
  const startNav = (pts: Vec2[]) => {
    if (pts.length >= 2) {
      const [p, q] = pts as [Vec2, Vec2];
      gesture.current = { kind: "pinch", dist: dist(p, q), mid: mid(p, q), view: viewRef.current };
    } else if (pts.length === 1) {
      gesture.current = { kind: "pan", start: pts[0], view: viewRef.current, moved: false, clearOnTap: false, slop: TOL.touch.slop };
    }
  };
  const navTo = (pts: Vec2[]) => {
    const g = gesture.current;
    if (g.kind === "pinch" && pts.length >= 2) {
      const [p, q] = pts as [Vec2, Vec2];
      const m = mid(p, q);
      const factor = dist(p, q) / Math.max(g.dist, 1);
      autoFit.current = false;
      // Zoom about the starting midpoint, then pan by the midpoint's travel.
      const z = zoomAtPure(g.view, size, g.mid, factor);
      queueView({ ...z, cx: z.cx - (m[0] - g.mid[0]) / z.scale, cy: z.cy + (m[1] - g.mid[1]) / z.scale });
    } else if (g.kind === "pan" && pts.length >= 1) update(pts[0], { shift: false, ctrl: false });
  };

  /** A finger held still: the right-click (select what is under it, then the context menu), else an ordinary press. */
  const longPressAt = (id: number, p: Vec2) => {
    setLpRing(null);
    const target = downInfo.current.get(id)?.target ?? null;
    const onControl = !!target?.closest?.("[data-guide],[data-handle]");
    if (doc && tool === "select" && !onControl) {
      const hit = hitTest(shapeIndex, toField(p[0], p[1]), TOL.touch.click / view.scale, true);
      if (hit && !selectedSet.has(hit)) props.onSelection(applySelect(selection, doc, [hit], "replace"));
      navigator.vibrate?.(10);
      const r = svgRef.current?.getBoundingClientRect();
      props.onContextMenu((r?.left ?? 0) + p[0], (r?.top ?? 0) + p[1]);
      return;
    }
    // Nothing here wants a long-press: it is a press after all (a guide grabbed on purpose, a held Line point).
    applyEffects(arb.feed({ kind: "promote", t: performance.now() }));
  };

  // A toggle on the touch modifier bar mid-drag re-evaluates the drag, as a key press does.
  useEffect(() =>
    subscribeVirtualMods(() => {
      const lm = lastMove.current;
      const g = gesture.current.kind;
      if (!lm || g === "none" || g === "idle" || g === "pinch" || g === "pan") return;
      const m = modsOf(lastKeys.current);
      lastMove.current = { s: lm.s, m };
      update(lm.s, m);
    }),
  );

  // A new selection starts with the scale handles.
  const selKey = selection.objects.join(",");
  useEffect(() => setRotMode(false), [selKey]);

  /** Advance the current drag to screen point `s` with modifiers `m`. */
  const update = (s: Vec2, m: Mods) => {
    const g = gesture.current;
    const f = toField(s[0], s[1]);
    switch (g.kind) {
      case "pan": {
        const dx = s[0] - g.start[0];
        const dy = s[1] - g.start[1];
        if (!g.moved && Math.hypot(dx, dy) < g.slop) return;
        g.moved = true;
        autoFit.current = false;
        queueView({ ...g.view, cx: g.view.cx - dx / g.view.scale, cy: g.view.cy + dy / g.view.scale });
        break;
      }
      case "marquee": {
        if (!g.moved && Math.hypot(s[0] - g.startScreen[0], s[1] - g.startScreen[1]) < g.slop) return;
        g.moved = true;
        setMarquee({ a: g.start, b: f });
        break;
      }
      case "move": {
        if (!g.moved && Math.hypot(s[0] - g.startScreen[0], s[1] - g.startScreen[1]) < g.slop) return;
        g.moved = true;
        let d: Vec2 = [f[0] - g.startField[0], f[1] - g.startField[1]];
        if (m.shift) d = constrainDelta(d);
        let hit: SnapHit | null = null;
        if (!m.shift) {
          // The reference point snaps; the whole selection follows it.
          const r = g.ref.p;
          hit = findSnap(g.index, [r[0] + d[0], r[1] + d[1]], snapOpts(m));
          if (hit) d = [hit.p[0] - r[0], hit.p[1] - r[1]];
        }
        setSnapShown(hit);
        setConstraint(m.shift ? constraintLine(g.startField, d) : null);
        setTransient({ m: [1, 0, 0, 1, d[0], d[1]], stock: g.stock });
        const from = g.ref.owner ? snapLabel(g.ref) : "";
        setReadout(`Δ ${fixedNumber(d[0], lu)}, ${fixedLength(d[1], lu)}${from ? ` · ${from}` : ""}${hit?.label ? ` → ${hit.label}` : ""}`);
        break;
      }
      case "scale": {
        // Smart guides while scaling: the dragged handle snaps to other objects' points and shows the alignment
        // guides (Ctrl only moves the pivot to the centre here, so it does not turn snapping off).
        const raw = handleTarget(g, f);
        const opts = snapOpts({ ...m, ctrl: false });
        let hit: SnapHit | null;
        if (g.handle.length === 1) {
          // An edge handle moves along one axis: only an alignment guide on that axis applies.
          const ax = g.handle === "e" || g.handle === "w" ? "x" : "y";
          const k = ax === "x" ? 0 : 1;
          const any = findSnap(g.index, raw, { ...opts, points: false, nearest: false, grid: 0 });
          const gs = (any?.guides ?? []).filter((q) => q.axis === ax);
          const at = (v: number): Vec2 => (k === 0 ? [v, raw[1]] : [raw[0], v]);
          // "Aligned · A / B" names both axes' sources; keep the one on this axis.
          const both = any?.label.startsWith("Aligned · ") ? any.label.slice("Aligned · ".length).split(" / ") : null;
          const label = both ? `${ax === "x" ? "Vertical" : "Horizontal"} · ${both[k] ?? both[0]}` : (any?.label ?? "");
          hit = any && gs.length ? { ...any, label, p: at(gs[0].to[k]), guides: gs.map((q) => ({ ...q, to: at(q.to[k]) })) } : null;
        } else hit = findSnap(g.index, raw, opts);
        g.target = hit ? hit.p : raw;
        setSnapShown(hit);
        const sc0 = scaleFromHandle(g.box, g.handle, g.target, m);
        const sc = g.round ? keepRound(sc0, g.handle) : sc0;
        setTransient({ m: scaleAbout(sc.sx, sc.sy, sc.about), stock: false });
        const w = (g.box.x1 - g.box.x0) * Math.abs(sc.sx);
        const h = (g.box.y1 - g.box.y0) * Math.abs(sc.sy);
        const [px, py] = [Math.abs(sc.sx) * 100, Math.abs(sc.sy) * 100];
        const flipped = sc.sx < 0 && sc.sy < 0 ? " · mirrored both ways" : sc.sx < 0 ? " · mirrored horizontally" : sc.sy < 0 ? " · mirrored vertically" : "";
        setReadout(`${fixedNumber(w, lu)} × ${fixedLength(h, lu)} (${px.toFixed(1)}%${px !== py ? ` × ${py.toFixed(1)}%` : ""})${flipped}`);
        break;
      }
      case "rotate": {
        const deg = rotateFromHandle(g.center, g.start, f, m);
        setTransient({ m: rotateAbout(deg, g.center), stock: false });
        setReadout(`Rotate ${Math.abs(deg).toFixed(1)}° ${deg < 0 ? "clockwise" : "counter-clockwise"}`);
        break;
      }
      case "draw": {
        const sp = drawEnd(g, f, m);
        setSnapShown(sp.hit);
        setDraw({ a: sp.a, b: sp.p, tool: g.tool });
        break;
      }
      case "ext":
        props.canvasTool?.drag(f, m, canvasCtx());
        break;
    }
  };

  /** Hover feedback at screen point `s` (mouse or pen, no button down): the canvas tool's, or where a draw tool snaps. */
  const hoverAt = (s: Vec2, m: Mods) => {
    const f = toField(s[0], s[1]);
    // Hovering with a draw tool shows where the first click would snap (Fusion shows snaps before the click).
    if (doc && props.canvasTool?.hover(f, m, canvasCtx())) return;
    if (isDrawTool(tool) && doc) setSnapShown(snapPoint(f, snapIndexFor(new Set(), true), m).hit);
  };

  const onPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const s = local(e);
    const f = toField(s[0], s[1]);
    const cls = pointerClass(e);
    // Fingers that pan or pinch skip the cursor readout: one re-render less per move (T20).
    const palm = cls === "touch" && pointers.current.has(e.pointerId) && !arb.tracks(e.pointerId);
    if (!(cls === "touch" && (arb.navigating || gesture.current.kind === "pinch" || palm))) {
      setCursor(f);
      props.onCursor?.(f);
    }
    // Also kept through a drag, so a modifier pressed after the button comes up acts where the pointer is.
    lastHover.current = cls === "touch" ? null : s;
    if (!pointers.current.has(e.pointerId)) {
      if (arb.state === "idle") clsRef.current = cls;
      if (cls !== "touch") hoverAt(s, modsOf(e));
      else if (doc) props.canvasTool?.hover(f, modsOf(e), canvasCtx());
      return;
    }
    pointers.current.set(e.pointerId, s);
    lastKeys.current = keysOf(e);
    applyEffects(arb.feed({ kind: "move", id: e.pointerId, p: s, t: e.timeStamp }));
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
    lastKeys.current = keysOf(e);
    const s = local(e);
    applyEffects(arb.feed(cancelled ? { kind: "cancel", id: e.pointerId, t: e.timeStamp } : { kind: "up", id: e.pointerId, p: s, t: e.timeStamp }));
    downInfo.current.delete(e.pointerId);
    // Lifting one finger of a pinch ends it; the remaining finger does nothing until lifted.
    if (pointers.current.size === 0 && (gesture.current.kind === "idle" || gesture.current.kind === "pinch")) gesture.current = { kind: "none" };
  };

  /** The press ended normally at screen point `s`: commit what the gesture did (the old pointerup). */
  const releasePress = (s: Vec2) => {
    const g = gesture.current;
    gesture.current = pointers.current.size ? { kind: "idle" } : { kind: "none" };
    const t = transient;
    setTransient(null);
    setSnapShown(null);
    setConstraint(null);
    setReadout(null);
    // The modifiers as the gesture ends (a latched toggle of the touch bar still counts), then the latch is used up.
    const m = modsOf(lastKeys.current);
    clearLatched();
    if (!doc) {
      if (g.kind === "ext") props.canvasTool?.cancel();
      setDraw(null);
      setMarquee(null);
      return;
    }
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
          // A click on one of several selected objects narrows the selection to it; a touch tap on the selection
          // (already narrowed) switches the handles between scale and rotate, LightBurn's click-again rotate.
          const narrowed = g.tapSelect ? applySelect(selection, doc, [g.tapSelect], "replace") : null;
          const same = !!narrowed && narrowed.objects.length === selection.objects.length && narrowed.objects.every((id) => selectedSet.has(id));
          if (narrowed && !(same && g.rotToggle)) props.onSelection(narrowed);
          else if (g.rotToggle) setRotMode((r) => !r);
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
        const sc0 = scaleFromHandle(g.box, g.handle, g.target ?? handleTarget(g, f), m);
        const sc = g.round ? keepRound(sc0, g.handle) : sc0;
        if (Math.abs(sc.sx - 1) < 1e-6 && Math.abs(sc.sy - 1) < 1e-6) break;
        const about: Vec2 = [roundMm(sc.about[0]), roundMm(sc.about[1])];
        // Past the opposite side the selection mirrors (LightBurn); the engine takes that as a mirror, not a factor < 0.
        const sp = signedScaleParams(round6(sc.sx), round6(sc.sy));
        commit(C.transform(g.ids, { ...sp, about }), { m: scaleAbout(sc.sx, sc.sy, about), stock: false });
        break;
      }
      case "rotate": {
        if (!t) break;
        const deg = rotateFromHandle(g.center, g.start, f, m);
        if (Math.abs(deg) < 1e-6) break;
        const about: Vec2 = [roundMm(g.center[0]), roundMm(g.center[1])];
        commit(C.transform(g.ids, { rotate_deg: round6(deg), about }), { m: rotateAbout(deg, about), stock: false });
        break;
      }
      case "draw": {
        setDraw(null);
        const end = drawEnd(g, f, m);
        finishDraw(end.a, end.p, g.tool, g.slop);
        break;
      }
      case "ext":
        props.canvasTool?.up(f, m, canvasCtx());
        break;
    }
  };

  /** `slop`: a drag shorter than this (screen px) is a tap, which places a default-size shape. */
  const finishDraw = (a: Vec2, b: Vec2, shape: DrawTool, slop: number) => {
    if (!doc) return;
    const layer = props.activeLayer ?? doc.layers[0]?.id;
    if (!layer) {
      props.notify(t("vp.noLayerToDrawOn"), "error");
      return;
    }
    let inv: Invocation;
    if (shape === "circle") {
      // Centre and radius (Fusion's centre-diameter circle); stored as an ellipse with equal radii.
      let r = dist(a, b);
      if (r * view.scale < slop) r = TAP_SIZE_MM / 2;
      r = Math.max(roundMm(r), 0.005);
      inv = C.shapeEllipse(layer, [roundMm(a[0]), roundMm(a[1])], r, r);
      inv.params.name = "Circle";
    } else {
      let w = Math.abs(b[0] - a[0]);
      let h = Math.abs(b[1] - a[1]);
      let center: Vec2 = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
      if (w * view.scale < slop && h * view.scale < slop) {
        // A tap: drop a default-size shape centred on it.
        w = h = TAP_SIZE_MM;
        center = a;
      }
      center = [roundMm(center[0]), roundMm(center[1])];
      w = Math.max(roundMm(w), 0.01);
      h = Math.max(roundMm(h), 0.01);
      inv = shape === "ellipse" ? C.shapeEllipse(layer, center, w / 2, h / 2) : C.shapeRect(layer, center, w, h);
      if (shape === "ellipse" && w === h) inv.params.name = "Circle";
    }
    props
      .run(inv)
      .then((r) => {
        const ids = createdIds(r);
        if (ids.length && doc) props.onSelection({ objects: ids, layer, stock: false, lastAdded: ids, reference: null });
      })
      .catch(props.report);
    props.onToolDone();
  };

  /** Screen px of a field point. */
  const toScreen = (p: Vec2): Vec2 => [(p[0] - view.cx) * view.scale + size[0] / 2, -(p[1] - view.cy) * view.scale + size[1] / 2];

  /** The context handed to the canvas tool on each pointer event. */
  const canvasCtx = (): CanvasCtx => ({
    doc: doc as GDocument,
    scale: view.scale,
    size,
    toField: (s) => toField(s[0], s[1]),
    toScreen,
    snap: (p, m, o) => snapPoint(p, snapIndexFor(new Set(o?.exclude ?? []), true), m, o?.extra),
    showSnap: (hit) => {
      setSnapShown(hit);
      setConstraint(null);
    },
    showConstraint: setConstraint,
    readout: setReadout,
    hitObject: (p) => hitTest(shapeIndex, p, TOL[clsRef.current].click / view.scale, clsRef.current !== "mouse"),
    pointerClass: clsRef.current,
    tol: TOL[clsRef.current],
  });

  // The touch HUD (touch/TouchHud.tsx) places the action bar above the selection and picks Group or Ungroup.
  const selGrouped = useMemo(() => !!doc && anyGrouped(doc, selection.objects), [doc, selection.objects]);
  useEffect(() => {
    const r = svgRef.current?.getBoundingClientRect();
    const canvasRect = r ? { left: r.left, top: r.top, right: r.right, bottom: r.bottom } : null;
    let selRect: { left: number; top: number; right: number; bottom: number } | null = null;
    if (r && selBounds && selection.objects.length && tool === "select" && !showPreview) {
      const a = toScreen([selBounds.x0, selBounds.y1]);
      const b = toScreen([selBounds.x1, selBounds.y0]);
      selRect = { left: r.left + a[0], top: r.top + a[1], right: r.left + b[0], bottom: r.top + b[1] };
    }
    publishHud({ selRect, canvasRect, grouped: selGrouped, rotateMode: rotMode && handleCls !== "mouse" });
    // toScreen follows view and size.
  }, [selBounds, view, size, selGrouped, selection.objects, rotMode, handleCls, tool, showPreview]);

  // Hotkeys (zoom in/out, fit) and the status bar talk to the canvas through workspace/viewStatus.ts.
  useEffect(
    () =>
      onViewCommand((cmd) => {
        if (cmd === "fit") return fitField();
        autoFit.current = false;
        if (cmd === "selection") {
          const b = selBounds ?? stockBox;
          if (b) fitBox(pad(b, 3));
        } else queueView((v) => zoomAtPure(v, size, [size[0] / 2, size[1] / 2], cmd === "zoomIn" ? 1.25 : 0.8));
      }),
    [fitField, fitBox, selBounds, stockBox, size, queueView],
  );
  // The phone's peek sheet opened for a selection: pan just enough that the selection shows above it.
  useEffect(
    () =>
      onRevealSelection((fraction) => {
        const b = selBounds;
        if (!b) return;
        const v = viewRef.current;
        const top = -(b.y1 - v.cy) * v.scale + size[1] / 2;
        const bottom = -(b.y0 - v.cy) * v.scale + size[1] / 2;
        const shift = revealShift(top, bottom, size[1] * fraction);
        if (shift <= 0) return;
        autoFit.current = false;
        setView({ ...v, cy: v.cy - shift / v.scale });
      }),
    [selBounds, size, setView],
  );
  useEffect(() => {
    publishViewStatus({ cursor, snap: snapShown ? { p: snapShown.p, label: snapShown.label, kind: snapShown.kind } : null, scale: view.scale });
  }, [cursor, snapShown, view.scale]);

  // --- rendering -------------------------------------------------------------------------------------------------
  const px = (n: number) => n / view.scale;
  // Selection and rotation handles follow Preferences > Handle and node size.
  const hpx = (n: number) => (n * uiPrefs.handleScale) / view.scale;
  const renderCtx: RenderCtx = {
    scale: view.scale,
    size,
    px,
    toField: (s) => toField(s[0], s[1]),
    toScreen,
    snap: (p, m, o) => snapPoint(p, snapIndexFor(new Set(o?.exclude ?? []), true), m, o?.extra),
  };
  const halfX = wsW / 2;
  const halfY = wsH / 2;
  const matrix = `matrix(${view.scale},0,0,${-view.scale},${size[0] / 2 - view.scale * view.cx},${size[1] / 2 + view.scale * view.cy})`;

  // The document's vectors as raster-layer data, built once per document; the flags follow the selection
  // (vectorScene.ts, drawn by VectorLayer.tsx). A view change or a drag only changes uniforms.
  const scene = useMemo(() => buildVectorScene(doc), [doc]);
  const flags = useMemo(() => segmentFlags(scene, selectedSet, outside), [scene, selectedSet, outside]);
  const tokens = useCanvasTokens();

  // The toolpath preview, packed by style and tile once per preview (drawn on a canvas, sceneLayers.ts).
  const { hidden: tpHidden, hiddenPasses: tpHiddenPasses, upTo: tpUpTo, jumps: tpJumps, shade: tpShade } = previewOptions;
  const toolpath = useMemo(
    () => (preview && showPreview ? buildToolpath(preview, doc, { hidden: tpHidden, hiddenPasses: tpHiddenPasses, upTo: tpUpTo, jumps: tpJumps }) : null),
    [preview, showPreview, doc, tpHidden, tpHiddenPasses, tpUpTo, tpJumps],
  );
  const toolpathStyle = useMemo<ToolpathStyle>(() => ({ layers: layerById, theme, shade: tpShade, ...TOOLPATH_INK[theme] }), [layerById, theme, tpShade]);

  // Images: drawn on a canvas under the outlines from cached bitmaps (the original, or the layer's dithered preview).
  const imageObjects = useMemo(() => (doc?.objects ?? []).filter((o) => o.geometry.kind === "image"), [doc]);
  const [dithered, setDithered] = useState<ReadonlyMap<string, string>>(() => new Map());
  const onDithered = useCallback((id: string, href: string | null) => {
    setDithered((m) => {
      if ((m.get(id) ?? null) === href) return m;
      const next = new Map(m);
      if (href) next.set(id, href);
      else next.delete(id);
      return next;
    });
  }, []);
  const imageSources = useMemo(
    () =>
      imageObjects
        .filter((o) => layerById.get(o.layer)?.visible !== false)
        .map((o) => ({ o, g: o.geometry as ImageGeom, href: dithered.get(o.id) ?? originalHref(o.geometry as ImageGeom) })),
    [imageObjects, layerById, dithered],
  );
  const imageHrefs = useMemo(() => [...new Set(imageSources.map((i) => i.href))], [imageSources]);
  const bitmapVersion = useBitmaps(imageHrefs);
  // Masked images (Apply Mask to Image) show only inside their mask (src/files/fileFormats.ts).
  const imageClips = useMemo(() => maskClips(doc), [doc]);
  const imageItems = useMemo<ImageItem[]>(
    () =>
      imageSources.map(({ o, g, href }) => ({
        id: o.id,
        href,
        m: liveObjects && selectedSet.has(o.id) ? mul(liveObjects, o.transform) : o.transform,
        widthMm: g.width_mm,
        heightMm: g.height_mm,
        clip: imageClips.get(o.id),
      })),
    [imageSources, liveObjects, selectedSet, imageClips],
  );
  const paintImageLayer = useMemo(
    () => (imageItems.length ? (ctx: CanvasRenderingContext2D, a: PaintArgs) => paintImages(ctx, a, imageItems, showPreview ? 0.3 : 1) : null),
    // A finished bitmap load (`bitmapVersion`) repaints.
    [imageItems, showPreview, bitmapVersion],
  );

  // The field, grid, axes and stock: painted per frame in device px under the images (vectorScene.ts paintBack).
  const stockM = useMemo(() => (stock ? (liveStock ? mul(liveStock, stock.transform) : stock.transform) : null), [stock, liveStock]);
  const paintBackLayer = useMemo(
    () => (ctx: CanvasRenderingContext2D, a: PaintArgs) =>
      paintBack(ctx, a, {
        half: [halfX, halfY],
        grid: { visible: uiPrefs.gridVisible, prefMm: uiPrefs.gridMm },
        stock: stock && stockM ? { cmds: stock.cmds, m: stockM, selected: selection.stock } : null,
        colors: tokens,
      }),
    [halfX, halfY, uiPrefs.gridVisible, uiPrefs.gridMm, stock, stockM, selection.stock, tokens],
  );
  const underlay = props.canvasTool?.underlay?.(renderCtx);

  const drawBox = draw ? boxFromPoints(draw.a, draw.b) : null;
  const marqueeBox = marquee ? boxFromPoints(marquee.a, marquee.b) : null;
  const crossing = marquee ? marquee.b[0] < marquee.a[0] : false;

  // Groups among the selection get a dot-dot-dash outline around their bounds (once per selection, in one pass, so a
  // drag of thousands of selected objects does no per-move work here beyond mapping the boxes).
  const selectedGroupBoxes = useMemo(() => {
    if (!doc) return [];
    const loose = new Set(doc.objects.filter((o) => !o.group).map((o) => o.id));
    const members = new Map<string, (Box | null)[]>();
    for (const u of unitsOf(doc, selection.objects)) if (!loose.has(u)) members.set(u, []);
    if (!members.size) return [];
    for (const o of doc.objects) {
      if (!selectedSet.has(o.id)) continue;
      members.get(unitOf(doc, o.id))?.push(boundsById.get(o.id) ?? null);
    }
    return [...members.values()].map(unionBoxes).filter((b): b is Box => b !== null);
  }, [doc, selection.objects, selectedSet, boundsById]);
  const selectedGroups = useMemo(() => (liveObjects ? selectedGroupBoxes.map((b) => mapBox(b, liveObjects)) : selectedGroupBoxes), [selectedGroupBoxes, liveObjects]);

  const showHandles = tool === "select" && selBounds && selection.objects.length > 0 && !showPreview;
  // Handles follow the last pointer used on the canvas. A mouse keeps LightBurn's: 16 px scale handles plus the round
  // rotate handles outside the corners. Touch draws them about twice the size with 44 px hit areas (pen 24 px), shows
  // the scale handles or (after a tap on the selection) the rotate handles, and moves them out of a small selection's
  // way (corners only), so a finger can still grab the object itself.
  const touchHandles = handleCls !== "mouse";
  const hitPx = TOL[handleCls].handle;
  const visK = handleCls === "mouse" ? 1 : handleCls === "pen" ? 1.5 : 2;
  const selScreenMin = selBounds ? Math.min(selBounds.x1 - selBounds.x0, selBounds.y1 - selBounds.y0) * view.scale : 0;
  const smallSel = touchHandles && selScreenMin < 2 * hitPx;
  const boxPad = touchHandles ? (smallSel ? hitPx / 2 + 4 : 10) : 6;
  const showScale = !touchHandles || !rotMode;
  const showRot = !touchHandles || rotMode;
  const scaleHandles = smallSel ? HANDLES.filter((h) => h.length === 2) : HANDLES;
  /** Touch: the snap label and readout go in a bubble above the finger instead of under it (T16). */
  const fingerAt = lastCls === "touch" && pointers.current.size > 0 ? (lastMove.current?.s ?? null) : null;
  const outsideCount = outside.size;

  return (
    <div
      className={`viewport tool-${tool}${spacePan ? " space-pan" : ""}${touchHandles ? " touch-handles" : ""}`}
      ref={wrapRef}
      tabIndex={-1}
      onContextMenu={(e) => {
        e.preventDefault();
        // A finger opens the menu with the canvas's own long-press: the browser's touch long-press menu (Android) is
        // ignored, and so is a menu that arrives in the middle of a drag (a Mac Ctrl+drag).
        if (e.nativeEvent.isTrusted) {
          const pt = (e.nativeEvent as MouseEvent & { pointerType?: string }).pointerType;
          if (pt === "touch" || performance.now() - lastTouchT.current < 1000) return;
          const gk = gesture.current.kind;
          if (gk !== "none" && gk !== "idle") return;
        }
        if (!doc || tool !== "select") return;
        const s = local(e);
        const hit = hitTest(shapeIndex, toField(s[0], s[1]), TOL.mouse.click / view.scale);
        if (hit && !selectedSet.has(hit)) props.onSelection(applySelect(selection, doc, [hit], "replace"));
        props.onContextMenu(e.clientX, e.clientY);
      }}
    >
      {imageObjects.map((o) => (
        <DitheredSource key={o.id} id={o.id} g={o.geometry as ImageGeom} layer={layerById.get(o.layer)} onHref={onDithered} />
      ))}
      <CanvasLayer className="vp-back" view={view} size={size} dpr={dpr} paint={paintBackLayer} />
      {underlay != null && underlay !== false && (
        <svg className="vp-layer vp-under" width={size[0]} height={size[1]} aria-hidden="true">
          <g transform={matrix}>{underlay}</g>
        </svg>
      )}
      {paintImageLayer && <CanvasLayer className="vp-images" view={view} size={size} dpr={dpr} paint={paintImageLayer} />}
      <VectorLayer
        view={view}
        size={size}
        dpr={dpr}
        doc={doc}
        scene={scene}
        flags={flags}
        selected={selectedSet}
        outside={outside}
        live={liveObjects}
        theme={theme}
        filled={viewStyle === "filled"}
        dimmed={showPreview}
        tokens={tokens}
        viewPending={viewPending}
      />
      {toolpath && toolpath.segments > 0 && <ToolpathLayer view={view} size={size} dpr={dpr} toolpath={toolpath} style={toolpathStyle} />}
      <svg
        ref={svgRef}
        className="vp-main"
        width={size[0]}
        height={size[1]}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={(e) => endPointer(e, false)}
        onPointerCancel={(e) => endPointer(e, true)}
        onPointerLeave={() => {
          setCursor(null);
          props.onCursor?.(null);
          lastHover.current = null;
          if (gesture.current.kind === "none") {
            setSnapShown(null);
            setConstraint(null);
          }
        }}
        onAuxClick={(e) => e.preventDefault()}
      >
        <g transform={matrix}>
          {toolpath?.head && <circle className="tp-head" cx={toolpath.head[0]} cy={toolpath.head[1]} r={px(4)} strokeWidth={px(1.5)} pointerEvents="none" />}
          <HeadMarker px={px} />

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
                x={selBounds.x0 - px(boxPad)}
                y={selBounds.y0 - px(boxPad)}
                width={selBounds.x1 - selBounds.x0 + px(2 * boxPad)}
                height={selBounds.y1 - selBounds.y0 + px(2 * boxPad)}
                strokeWidth={px(1)}
                pointerEvents="none"
              />
              {!live &&
                showRot &&
                (["rot-nw", "rot-ne", "rot-se", "rot-sw"] as const).map((h) => {
                  if (touchHandles) {
                    // Touch rotate mode: the rotate handles take the corners.
                    const q = handlePoint(pad(selBounds, px(boxPad)), h.slice(4) as Handle);
                    return (
                      <g key={h} data-handle={h} className="rot-handle touch">
                        <circle cx={q[0]} cy={q[1]} r={hpx(hitPx / 2)} className="handle-hit" />
                        <circle cx={q[0]} cy={q[1]} r={hpx(4.5 * visK)} strokeWidth={px(1.5)} />
                      </g>
                    );
                  }
                  const p = handlePoint(pad(selBounds, px(6)), h.slice(4) as Handle);
                  const o = hpx(14);
                  const cx = p[0] + (h.endsWith("e") ? o : -o);
                  const cy = p[1] + (h.includes("n") ? o : -o);
                  return (
                    <g key={h} data-handle={h} className="rot-handle">
                      <circle cx={cx} cy={cy} r={hpx(9)} className="handle-hit" />
                      <circle cx={cx} cy={cy} r={hpx(4.5)} strokeWidth={px(1.2)} />
                    </g>
                  );
                })}
              {!live &&
                showScale &&
                scaleHandles.map((h) => {
                  const [x, y] = handlePoint(pad(selBounds, px(boxPad)), h);
                  const hh = hitPx / 2;
                  const vv = 4 * visK;
                  return (
                    <g key={h} data-handle={h} className={`scale-handle h-${h}`}>
                      <rect x={x - hpx(hh)} y={y - hpx(hh)} width={hpx(2 * hh)} height={hpx(2 * hh)} className="handle-hit" />
                      <rect x={x - hpx(vv)} y={y - hpx(vv)} width={hpx(2 * vv)} height={hpx(2 * vv)} strokeWidth={px(1)} className="handle" />
                    </g>
                  );
                })}
              <circle className="sel-center" cx={(selBounds.x0 + selBounds.x1) / 2} cy={(selBounds.y0 + selBounds.y1) / 2} r={px(2.5)} pointerEvents="none" />
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

          {draw && draw.tool === "circle" && (
            <g className="rubber-circle">
              <circle className="rubber" cx={draw.a[0]} cy={draw.a[1]} r={dist(draw.a, draw.b)} strokeWidth={px(1.2)} />
              <path className="rubber-radius" d={`M${draw.a[0]},${draw.a[1]}L${draw.b[0]},${draw.b[1]}`} strokeWidth={px(1)} strokeDasharray={`${px(4)} ${px(3)}`} />
            </g>
          )}
          {drawBox &&
            draw &&
            draw.tool !== "circle" &&
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
          {props.canvasTool?.render(renderCtx)}
        </g>
        {constraint && <ConstraintGuide line={constraint} at={toScreen} size={size} />}
        {snapShown && (
          <SnapOverlay hit={snapShown} label={!fingerAt} at={(p) => [(p[0] - view.cx) * view.scale + size[0] / 2, -(p[1] - view.cy) * view.scale + size[1] / 2]} />
        )}
        {uiPrefs.crosshair && cursor && (
          <path
            className="crosshair"
            d={`M0,${-(cursor[1] - view.cy) * view.scale + size[1] / 2}H${size[0]}M${(cursor[0] - view.cx) * view.scale + size[0] / 2},0V${size[1]}`}
          />
        )}
      </svg>

      <div className="vp-overlay vp-top-right">
        <button className="icon-btn" title={t("vp.fitField")} aria-label={t("vp.fitField")} onClick={fitField}>
          <Icon name="fit" />
        </button>
        <button
          className="icon-btn"
          title={t("vp.zoomToSelection")}
          aria-label={t("vp.zoomToSelection")}
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
          {outsideCount} object{outsideCount === 1 ? "" : "s"} outside the {galvo ? "galvo field" : "workspace"} (red). The laser cannot reach{" "}
          {outsideCount === 1 ? "it" : "them"}.
        </div>
      )}
      <div className="vp-overlay vp-bottom-left">
        {cursor
          ? `X ${fixedNumber(cursor[0], lu)}  Y ${fixedLength(cursor[1], lu)}`
          : galvo
            ? `Field ${formatLength(wsW, lu, 3)}`
            : `Workspace ${trimNumber(lengthToDisplay(wsW, lu), lengthDigits(lu, 3))} × ${formatLength(wsH, lu, 3)}`}
        {draw && draw.tool === "circle"
          ? `  ·  Ø ${fixedLength(2 * dist(draw.a, draw.b), lu)}`
          : drawBox && `  ·  ${fixedNumber(drawBox.x1 - drawBox.x0, lu)} × ${fixedLength(drawBox.y1 - drawBox.y0, lu)}`}
        {readout && `  ·  ${readout}`}
      </div>
      {props.canvasTool?.overlay(renderCtx)}
      {isDrawTool(tool) && (
        <ContextStripPortal>
          <div className="vp-overlay vp-top-left hint">
            {tool === "circle" ? t("rest.drawCircle") : tool === "rect" ? t("rest.drawRect") : t("rest.drawEllipse")} ·{" "}
            {t(tool === "circle" ? "rest.drawTailCircle" : "rest.drawTail", { size: formatLength(20, lu) })}
          </div>
        </ContextStripPortal>
      )}
      {fingerAt && (snapShown?.label || readout) && (
        <div className="vp-overlay snap-bubble" style={{ left: fingerAt[0], top: Math.max(4, fingerAt[1] - 60) }} aria-hidden="true">
          {snapShown?.label}
          {snapShown?.label && readout ? " · " : ""}
          {readout}
        </div>
      )}
      {lpRing && <div className="lp-ring" style={{ left: lpRing[0], top: lpRing[1] }} aria-hidden="true" />}
    </div>
  );
}

/** A grab position that is not a snap point (a selection with no snap points), as a reference point. */
function grabPoint(f: Vec2): SnapPoint {
  return { p: f, kind: "nearest", what: "", owner: "", id: "" };
}

/** Glyph path for a snap kind at screen point (x, y), Fusion-like: each kind its own shape. */
function snapGlyph(kind: SnapKind, x: number, y: number): string {
  const r = 6;
  switch (kind) {
    case "center":
      return `M${x - r},${y}a${r},${r} 0 1,0 ${2 * r},0a${r},${r} 0 1,0 ${-2 * r},0`;
    case "quadrant":
      return `M${x},${y - r - 1}L${x + r + 1},${y}L${x},${y + r + 1}L${x - r - 1},${y}Z`;
    case "endpoint":
      return `M${x - r},${y - r}h${2 * r}v${2 * r}h${-2 * r}Z`;
    case "midpoint":
      return `M${x},${y - r - 1}L${x + r + 1},${y + r - 1}L${x - r - 1},${y + r - 1}Z`;
    case "intersection":
      return `M${x - r},${y - r}L${x + r},${y + r}M${x + r},${y - r}L${x - r},${y + r}`;
    case "nearest":
      return `M${x - r},${y - r}L${x + r},${y - r}L${x - r},${y + r}L${x + r},${y + r}Z`;
    case "guide":
    case "grid":
      return `M${x - 4},${y}H${x + 4}M${x},${y - 4}V${y + 4}`;
  }
}

/** The snap glyph, its label and the dotted alignment guides, drawn in screen pixels on top of the canvas. */
function SnapOverlay({ hit, at, label = true }: { hit: SnapHit; at: (p: Vec2) => Vec2; label?: boolean }) {
  const [x, y] = at(hit.p);
  return (
    <g className={`snap-overlay snap-${hit.kind}`} pointerEvents="none">
      {hit.guides.map((g, i) => {
        const a = at(g.from);
        return (
          <g key={i}>
            <path className="snap-guide" d={`M${a[0]},${a[1]}L${x},${y}`} strokeDasharray="2 3" />
            <path className="snap-guide-source" d={`M${a[0] - 3},${a[1] - 3}h6v6h-6Z`} />
          </g>
        );
      })}
      <path className="snap-marker" d={snapGlyph(hit.kind, x, y)} />
      {hit.kind === "center" && <circle className="snap-dot" cx={x} cy={y} r={1.6} />}
      {label && hit.label && (
        <text className="snap-label" x={x + 11} y={y - 9}>
          {hit.label}
        </text>
      )}
    </g>
  );
}

/** The Shift constraint guide: the locked axis or angle through the drag's start, across the canvas, in screen px. */
function ConstraintGuide({ line, at, size }: { line: ConstraintLine; at: (p: Vec2) => Vec2; size: Vec2 }) {
  const seg = constraintSegment(line, at, size);
  if (!seg) return null;
  const [a, b] = seg;
  return <path className="constraint-guide" d={`M${a[0]},${a[1]}L${b[0]},${b[1]}`} strokeDasharray="8 4" pointerEvents="none" />;
}

type ImageGeom = Extract<GDocument["objects"][number]["geometry"], { kind: "image" }>;

/** Reports an image's dithered preview (when the image view is "dithered" or Adjust image is open), else null. */
const DitheredSource = memo(function DitheredSource(p: { id: string; g: ImageGeom; layer?: Layer; onHref: (id: string, href: string | null) => void }) {
  const { id, onHref } = p;
  const href = useDitheredHref(id, p.g, p.layer);
  useEffect(() => onHref(id, href), [id, href, onHref]);
  useEffect(() => () => onHref(id, null), [id, onHref]);
  return null;
});

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
function pad(b: Box, m: number): Box {
  return { x0: b.x0 - m, y0: b.y0 - m, x1: b.x1 + m, y1: b.y1 + m };
}
