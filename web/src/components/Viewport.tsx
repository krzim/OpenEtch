// The centre canvas: an SVG in field millimetres with +Y up. One group carries the view transform
// (pan/zoom and the y-flip); everything inside it is drawn in field mm. Stroke widths are divided by the zoom so they
// stay a constant number of screen pixels on every browser.
//
// Interaction uses Pointer Events only (mouse, pen and touch alike):
// - drag on empty space pans, two pointers pan and pinch-zoom, the wheel zooms (ctrl+wheel is a trackpad pinch);
// - tap an object to select it (shift adds), drag a selected object to move it. The move is drawn as a transient
//   offset and sent as a single `object.transform` translate on pointerup; the document itself is never touched;
// - with the Rectangle or Ellipse tool, drag to draw (a tap places a 20 mm shape).

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import type { GDocument, Preview, Vec2 } from "../api/types.ts";
import { boxCenter, cmdsBounds, geometryCmds, objectBounds, stockCmds, toPathData, unionBoxes, type Box } from "../geometry.ts";
import type { Selection } from "../selection.ts";
import { Icon } from "./ui.tsx";

export type Tool = "select" | "rect" | "ellipse";

/** View transform: field point (cx, cy) at the canvas centre, `scale` screen px per mm. */
interface View {
  cx: number;
  cy: number;
  scale: number;
}

type Gesture =
  | { kind: "none" }
  | { kind: "pan"; start: Vec2; view: View; moved: boolean; additive: boolean }
  | { kind: "move"; startField: Vec2; startScreen: Vec2; moved: boolean; hitId: string; additive: boolean }
  | { kind: "draw"; start: Vec2 }
  | { kind: "pinch"; dist: number; mid: Vec2; view: View };

const MIN_SCALE = 0.2;
const MAX_SCALE = 400;
/** Screen px a pointer may wander before a tap becomes a drag. */
const TAP_SLOP = 5;

export interface ViewportProps {
  doc: GDocument | null;
  fieldMm: number;
  selection: Selection;
  tool: Tool;
  /** Layer that new shapes go on. */
  activeLayer: string | null;
  preview: Preview | null;
  showPreview: boolean;
  onSelectObject: (id: string, additive: boolean) => void;
  onClearSelection: () => void;
  onToolDone: () => void;
  run: (inv: Invocation) => Promise<unknown>;
  notify: (msg: string, kind?: "info" | "error") => void;
}

export function Viewport(props: ViewportProps) {
  const { doc, fieldMm, selection, tool, preview, showPreview } = props;
  const wrapRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const [size, setSize] = useState<Vec2>([1, 1]);
  const [view, setView] = useState<View>({ cx: 0, cy: 0, scale: 3 });
  const [cursor, setCursor] = useState<Vec2 | null>(null);
  const [dragDelta, setDragDelta] = useState<Vec2 | null>(null);
  const [draw, setDraw] = useState<{ a: Vec2; b: Vec2 } | null>(null);
  /** Offset kept on screen after pointerup until the document revision changes, so the object does not snap back. */
  const [settling, setSettling] = useState<{ delta: Vec2; rev: number } | null>(null);
  const pointers = useRef(new Map<number, Vec2>());
  const gesture = useRef<Gesture>({ kind: "none" });
  /** Field size the view was last fitted to; null before the first fit. */
  const fitted = useRef<number | null>(null);

  // Track canvas size.
  useLayoutEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      setSize([Math.max(1, r.width), Math.max(1, r.height)]);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const fit = useCallback(
    (box?: Box | null) => {
      const b = box ?? { x0: -fieldMm / 2, y0: -fieldMm / 2, x1: fieldMm / 2, y1: fieldMm / 2 };
      const w = Math.max(b.x1 - b.x0, 1);
      const h = Math.max(b.y1 - b.y0, 1);
      const scale = clamp(Math.min(size[0] / w, size[1] / h) * 0.88, MIN_SCALE, MAX_SCALE);
      const [cx, cy] = boxCenter(b);
      setView({ cx, cy, scale });
    },
    [fieldMm, size],
  );

  // Fit the field once the canvas has a real size, and again whenever the field size changes.
  useEffect(() => {
    if (size[0] > 20 && size[1] > 20 && fitted.current !== fieldMm) {
      fitted.current = fieldMm;
      fit();
    }
  }, [size, fit, fieldMm]);

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

  // Wheel needs a non-passive listener to prevent page scroll/zoom.
  useEffect(() => {
    const el = svgRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const s = local(e);
      const pixelish = e.deltaMode === 0;
      const trackpadPan = !e.ctrlKey && pixelish && (e.deltaX !== 0 || Math.abs(e.deltaY) < 40);
      if (trackpadPan) {
        setView((v) => ({ ...v, cx: v.cx + e.deltaX / v.scale, cy: v.cy - e.deltaY / v.scale }));
      } else {
        const dy = pixelish ? e.deltaY : e.deltaY * 40;
        const factor = Math.exp(-dy * (e.ctrlKey ? 0.01 : 0.0015));
        setView((v) => zoomAtPure(v, size, s, factor));
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [size]);

  const selectedSet = useMemo(() => new Set(selection.objects), [selection.objects]);
  const layerById = useMemo(() => new Map((doc?.layers ?? []).map((l) => [l.id, l])), [doc]);

  const offset = dragDelta ?? settling?.delta ?? null;

  const selBounds = useMemo(() => {
    if (!doc || selection.objects.length === 0) return null;
    const b = unionBoxes(doc.objects.filter((o) => selectedSet.has(o.id)).map(objectBounds));
    if (!b || !offset) return b;
    return { x0: b.x0 + offset[0], y0: b.y0 + offset[1], x1: b.x1 + offset[0], y1: b.y1 + offset[1] };
  }, [doc, selection.objects, selectedSet, offset]);

  const finishDraw = (a: Vec2, b: Vec2) => {
    const layer = props.activeLayer ?? doc?.layers[0]?.id;
    if (!layer) {
      props.notify("No layer to draw on.", "error");
      return;
    }
    let w = Math.abs(b[0] - a[0]);
    let h = Math.abs(b[1] - a[1]);
    let center: Vec2 = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    if (w < 0.5 && h < 0.5) {
      // A tap: drop a default-size shape centred on it.
      w = h = 20;
      center = a;
    }
    center = [round2(center[0]), round2(center[1])];
    w = Math.max(round2(w), 0.1);
    h = Math.max(round2(h), 0.1);
    const inv = tool === "ellipse" ? C.shapeEllipse(layer, center, w / 2, h / 2) : C.shapeRect(layer, center, w, h);
    props.run(inv).catch((e) => props.notify(String(e instanceof Error ? e.message : e), "error"));
    props.onToolDone();
  };

  const onPointerDown = (e: React.PointerEvent<SVGSVGElement>) => {
    if (e.button !== 0 && e.pointerType === "mouse") return;
    const s = local(e);
    pointers.current.set(e.pointerId, s);
    try {
      svgRef.current?.setPointerCapture(e.pointerId);
    } catch {
      // Capture can fail for synthetic events; harmless.
    }
    if (pointers.current.size === 2) {
      // Second finger: abandon the single-pointer gesture and pinch instead.
      const [p, q] = [...pointers.current.values()] as [Vec2, Vec2];
      gesture.current = { kind: "pinch", dist: dist(p, q), mid: mid(p, q), view };
      setDragDelta(null);
      setDraw(null);
      return;
    }
    if (pointers.current.size > 2) return;
    const f = toField(s[0], s[1]);
    if (tool !== "select") {
      gesture.current = { kind: "draw", start: f };
      setDraw({ a: f, b: f });
      return;
    }
    const hit = (e.target as Element).closest?.("[data-oid]")?.getAttribute("data-oid");
    const additive = e.shiftKey || e.metaKey || e.ctrlKey;
    if (hit) {
      if (!selectedSet.has(hit) || additive) props.onSelectObject(hit, additive);
      gesture.current = { kind: "move", startField: f, startScreen: s, moved: false, hitId: hit, additive };
      return;
    }
    gesture.current = { kind: "pan", start: s, view, moved: false, additive };
  };

  const onPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const s = local(e);
    setCursor(toField(s[0], s[1]));
    if (!pointers.current.has(e.pointerId)) return;
    pointers.current.set(e.pointerId, s);
    const g = gesture.current;
    switch (g.kind) {
      case "pinch": {
        const pts = [...pointers.current.values()];
        if (pts.length < 2) return;
        const [p, q] = pts as [Vec2, Vec2];
        const m = mid(p, q);
        const factor = dist(p, q) / Math.max(g.dist, 1);
        // Zoom about the starting midpoint, then pan by the midpoint's travel.
        const z = zoomAtPure(g.view, size, g.mid, factor);
        setView({ ...z, cx: z.cx - (m[0] - g.mid[0]) / z.scale, cy: z.cy + (m[1] - g.mid[1]) / z.scale });
        break;
      }
      case "pan": {
        const dx = s[0] - g.start[0];
        const dy = s[1] - g.start[1];
        if (!g.moved && Math.hypot(dx, dy) < TAP_SLOP) return;
        g.moved = true;
        setView({ ...g.view, cx: g.view.cx - dx / g.view.scale, cy: g.view.cy + dy / g.view.scale });
        break;
      }
      case "move": {
        if (!g.moved && Math.hypot(s[0] - g.startScreen[0], s[1] - g.startScreen[1]) < TAP_SLOP) return;
        g.moved = true;
        const f = toField(s[0], s[1]);
        setDragDelta([f[0] - g.startField[0], f[1] - g.startField[1]]);
        break;
      }
      case "draw": {
        setDraw({ a: g.start, b: toField(s[0], s[1]) });
        break;
      }
    }
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
    gesture.current = { kind: "none" };
    if (cancelled) {
      setDragDelta(null);
      setDraw(null);
      return;
    }
    const s = local(e);
    switch (g.kind) {
      case "pan":
        if (!g.moved && !g.additive) props.onClearSelection();
        break;
      case "move": {
        const delta = dragDelta;
        setDragDelta(null);
        if (!g.moved || !delta || (Math.abs(delta[0]) < 1e-6 && Math.abs(delta[1]) < 1e-6)) {
          // A tap on an already-selected object narrows the selection to it.
          if (!g.moved && !g.additive && selection.objects.length > 1) props.onSelectObject(g.hitId, false);
          break;
        }
        const d: Vec2 = [round3(delta[0]), round3(delta[1])];
        const ids = selection.objects.includes(g.hitId) ? selection.objects : [g.hitId];
        setSettling({ delta: d, rev: doc?.revision ?? -1 });
        props
          .run(C.transform(ids, { translate: d }))
          .catch((err) => {
            setSettling(null);
            props.notify(String(err instanceof Error ? err.message : err), "error");
          });
        break;
      }
      case "draw": {
        setDraw(null);
        finishDraw(g.start, toField(s[0], s[1]));
        break;
      }
    }
  };

  // --- rendering -------------------------------------------------------------------------------------------------
  const px = (n: number) => n / view.scale;
  const half = fieldMm / 2;
  const matrix = `matrix(${view.scale},0,0,${-view.scale},${size[0] / 2 - view.scale * view.cx},${size[1] / 2 + view.scale * view.cy})`;

  const grid = useMemo(() => {
    const minor = [1, 2, 5, 10, 20, 50].find((st) => st * view.scale >= 10) ?? 50;
    const major = minor * 5;
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

  const stock = doc?.stock ? stockCmds(doc.stock) : null;

  const previewPaths = useMemo(() => {
    if (!preview || !showPreview) return null;
    const marks = new Map<string, string>();
    let jumps = "";
    for (const s of preview.segments ?? []) {
      const seg = `M${s.from[0]},${s.from[1]}L${s.to[0]},${s.to[1]}`;
      if (s.kind === "mark") {
        const k = s.layer ?? "";
        marks.set(k, (marks.get(k) ?? "") + seg);
      } else jumps += seg;
    }
    return { marks: [...marks.entries()], jumps };
  }, [preview, showPreview]);

  const drawBox = draw
    ? { x0: Math.min(draw.a[0], draw.b[0]), y0: Math.min(draw.a[1], draw.b[1]), x1: Math.max(draw.a[0], draw.b[0]), y1: Math.max(draw.a[1], draw.b[1]) }
    : null;

  return (
    <div className={`viewport tool-${tool}`} ref={wrapRef}>
      <svg
        ref={svgRef}
        width={size[0]}
        height={size[1]}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={(e) => endPointer(e, false)}
        onPointerCancel={(e) => endPointer(e, true)}
        onPointerLeave={() => setCursor(null)}
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
              d={toPathData(stock.cmds, stock.transform)}
              strokeWidth={px(1.5)}
              strokeDasharray={`${px(6)} ${px(3)}`}
            />
          )}

          <g className={showPreview ? "objects dimmed" : "objects"}>
            {doc?.objects.map((o) => {
              const layer = layerById.get(o.layer);
              if (layer && !layer.visible) return null;
              const d = toPathData(geometryCmds(o.geometry), o.transform);
              const sel = selectedSet.has(o.id);
              const t = sel && offset ? `translate(${offset[0]},${offset[1]})` : undefined;
              return (
                <g key={o.id} data-oid={o.id} transform={t}>
                  <path className="obj-hit" d={d} strokeWidth={px(14)} />
                  <path
                    className={sel ? "obj selected" : "obj"}
                    d={d}
                    stroke={sel ? undefined : (layer?.color ?? "#333")}
                    strokeWidth={px(sel ? 2 : 1.3)}
                  />
                </g>
              );
            })}
          </g>

          {previewPaths && (
            <g className="toolpath" pointerEvents="none">
              <path className="tp-jump" d={previewPaths.jumps} strokeWidth={px(0.8)} strokeDasharray={`${px(4)} ${px(3)}`} />
              {previewPaths.marks.map(([layerId, d]) => (
                <path key={layerId} className="tp-mark" d={d} stroke={layerById.get(layerId)?.color ?? "#d22"} strokeWidth={px(1.4)} />
              ))}
            </g>
          )}

          {selBounds && (
            <g className="sel-bounds" pointerEvents="none">
              <rect
                x={selBounds.x0 - px(4)}
                y={selBounds.y0 - px(4)}
                width={selBounds.x1 - selBounds.x0 + px(8)}
                height={selBounds.y1 - selBounds.y0 + px(8)}
                strokeWidth={px(1)}
                strokeDasharray={`${px(4)} ${px(3)}`}
              />
              {corners(selBounds, px(4)).map(([x, y], i) => (
                <rect key={i} className="handle" x={x - px(3.5)} y={y - px(3.5)} width={px(7)} height={px(7)} strokeWidth={px(1)} />
              ))}
            </g>
          )}

          {drawBox &&
            (tool === "ellipse" ? (
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
        <button className="icon-btn" title="Fit field" aria-label="Fit field" onClick={() => fit()}>
          <Icon name="fit" />
        </button>
        <button
          className="icon-btn"
          title="Zoom to selection"
          aria-label="Zoom to selection"
          disabled={!selBounds && !doc?.stock}
          onClick={() => {
            const b = selBounds ?? (stock ? cmdsBounds(stock.cmds, stock.transform) : null);
            if (b) fit(pad(b, 3));
          }}
        >
          <Icon name="place" />
        </button>
      </div>
      <div className="vp-overlay vp-bottom-left">
        {cursor ? `X ${cursor[0].toFixed(2)}  Y ${cursor[1].toFixed(2)} mm` : `Field ${fieldMm} mm`}
        {drawBox && `  ·  ${(drawBox.x1 - drawBox.x0).toFixed(2)} × ${(drawBox.y1 - drawBox.y0).toFixed(2)} mm`}
        {dragDelta && `  ·  Δ ${dragDelta[0].toFixed(2)}, ${dragDelta[1].toFixed(2)} mm`}
      </div>
      {tool !== "select" && <div className="vp-overlay vp-top-left hint">Drag to draw a {tool === "rect" ? "rectangle" : "ellipse"} · tap for 20 mm · Esc to cancel</div>}
    </div>
  );
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
function round2(v: number): number {
  return Math.round(v * 100) / 100;
}
function round3(v: number): number {
  return Math.round(v * 1000) / 1000;
}
function pad(b: Box, m: number): Box {
  return { x0: b.x0 - m, y0: b.y0 - m, x1: b.x1 + m, y1: b.y1 + m };
}
function corners(b: Box, m: number): Vec2[] {
  return [
    [b.x0 - m, b.y0 - m],
    [b.x1 + m, b.y0 - m],
    [b.x1 + m, b.y1 + m],
    [b.x0 - m, b.y1 + m],
  ];
}
