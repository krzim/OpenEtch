// LightBurn's image tools: the Trace Image dialog (Alt+T) with its live preview (`image.trace_preview`, read-only)
// drawn over the image, Fade Image, Show Points, a boundary dragged in the preview (Clear Boundary), Cutoff /
// Threshold, Ignore less than, Smoothness, Optimize, Trace Transparency, Center Line (2.2), Sketch and Delete image
// after trace; OK runs `image.trace`. And Convert to Bitmap (`art.convert_to_bitmap`): DPI, render (cut settings,
// fill all, outlines), grey level, line width and keep original.

import { useEffect, useMemo, useRef, useState } from "react";
import { execute, type Invocation } from "../api/client.ts";
import type { DocObject } from "../api/types.ts";
import { originalHref } from "../components/SceneCanvas.tsx";
import { CheckField, Modal } from "../components/ui.tsx";
import { objectBounds, unionBoxes } from "../geometry.ts";
import { useT } from "../workspace/usePrefs.ts";
import {
  DEFAULT_BITMAP,
  DEFAULT_TRACE,
  bitmapParams,
  bitmapPixels,
  pathPoints,
  traceParams,
  traceProblem,
  type BitmapOptions,
  type TraceOptions,
} from "./fileFormats.ts";
import { MmBox, NumBox, Row, parseBox, useBox } from "./FilesUiParts.tsx";
import { errorText } from "../api/errorText.ts";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export const IMAGE_CMD = {
  trace: "image.trace",
  tracePreview: "image.trace_preview",
  toBitmap: "art.convert_to_bitmap",
  applyMask: "image.apply_mask",
  removeMask: "image.remove_mask",
  flattenMask: "image.flatten_mask",
  replace: "image.replace",
  exportImage: "image.export",
  processed: "image.processed",
} as const;

type ImageGeom = { width_mm: number; height_mm: number; mime: string; data_base64: string };

function Slider(p: { label: string; value: number; min: number; max: number; step: number; onChange: (v: number) => void; digits?: number }) {
  return (
    <label className="field trace-slider">
      <span className="field-label">{p.label}</span>
      <input type="range" min={p.min} max={p.max} step={p.step} value={p.value} onChange={(e) => p.onChange(Number(e.target.value))} />
      <input
        className="trace-num"
        type="number"
        min={p.min}
        max={p.max}
        step={p.step}
        value={p.value}
        onChange={(e) => {
          const v = Number(e.target.value);
          if (Number.isFinite(v)) p.onChange(Math.min(p.max, Math.max(p.min, v)));
        }}
      />
    </label>
  );
}

export function TraceDialog(p: { image: DocObject; run: Run; report: (e: unknown) => void; onDone: (created: string[]) => void; onClose: () => void }) {
  const t = useT();
  const [o, setO] = useState<TraceOptions>(DEFAULT_TRACE);
  const [fade, setFade] = useState(false);
  const [points, setPoints] = useState(false);
  const [preview, setPreview] = useState<{ d: string; contours: number; nodes: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragFrom, setDragFrom] = useState<[number, number] | null>(null);
  const [dragTo, setDragTo] = useState<[number, number] | null>(null);
  const svg = useRef<SVGSVGElement>(null);
  const set = <K extends keyof TraceOptions>(k: K, v: TraceOptions[K]) => setO((x) => ({ ...x, [k]: v }));
  const problem = traceProblem(o);
  const g = p.image.geometry as unknown as ImageGeom;
  const box = objectBounds(p.image) ?? { x0: -10, y0: -10, x1: 10, y1: 10 };
  const pad = Math.max(box.x1 - box.x0, box.y1 - box.y0) * 0.03;
  const vb = { x: box.x0 - pad, y: -(box.y1 + pad), w: box.x1 - box.x0 + 2 * pad, h: box.y1 - box.y0 + 2 * pad };
  const m = p.image.transform;

  // Live preview, debounced; the newest request wins.
  const seq = useRef(0);
  const key = JSON.stringify(o);
  useEffect(() => {
    if (problem) return;
    const n = ++seq.current;
    const h = window.setTimeout(() => {
      execute({ id: IMAGE_CMD.tracePreview, params: traceParams(p.image.id, o, true) })
        .then((r) => {
          if (n !== seq.current) return;
          const res = r.result as { d?: unknown; contours?: unknown; nodes?: unknown };
          setPreview({ d: typeof res.d === "string" ? res.d : "", contours: Number(res.contours) || 0, nodes: Number(res.nodes) || 0 });
          setError(null);
        })
        .catch((e: unknown) => n === seq.current && setError(errorText(e)));
    }, 250);
    return () => window.clearTimeout(h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, p.image.id, problem]);

  const pts = useMemo(() => (points && preview ? pathPoints(preview.d, 4000) : []), [points, preview]);
  // A field point from a pointer event (the preview's viewBox is field mm with y flipped).
  const fieldAt = (e: React.PointerEvent): [number, number] | null => {
    const el = svg.current;
    const ctm = el?.getScreenCTM();
    if (!el || !ctm) return null;
    const pt = el.createSVGPoint();
    pt.x = e.clientX;
    pt.y = e.clientY;
    const q = pt.matrixTransform(ctm.inverse());
    return [q.x, -q.y];
  };
  const endDrag = () => {
    if (dragFrom && dragTo) {
      const b: [number, number, number, number] = [Math.min(dragFrom[0], dragTo[0]), Math.min(dragFrom[1], dragTo[1]), Math.max(dragFrom[0], dragTo[0]), Math.max(dragFrom[1], dragTo[1])];
      if (b[2] - b[0] > 0.05 && b[3] - b[1] > 0.05) set("boundary", b);
    }
    setDragFrom(null);
    setDragTo(null);
  };
  const ok = () => {
    if (problem || busy) return;
    setBusy(true);
    p.run({ id: IMAGE_CMD.trace, params: traceParams(p.image.id, o, false) })
      .then((r) => {
        const c = r && typeof r === "object" ? (r as { created?: unknown }).created : null;
        p.onDone(Array.isArray(c) ? c.filter((x): x is string => typeof x === "string") : []);
      })
      .catch(p.report)
      .finally(() => setBusy(false));
  };
  const strokeW = vb.w / 220;
  const bnd = o.boundary;
  const live = dragFrom && dragTo ? [Math.min(dragFrom[0], dragTo[0]), Math.min(dragFrom[1], dragTo[1]), Math.max(dragFrom[0], dragTo[0]), Math.max(dragFrom[1], dragTo[1])] : null;
  return (
    <Modal
      title={t("files.traceTitle")}
      onClose={p.onClose}
      wide
      footer={
        <>
          {(problem || error) && <span className="form-error">{problem ?? error}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!!problem || busy} onClick={ok}>
            {t("files.ok")}
          </button>
        </>
      }
    >
      <div className="trace-layout">
        <div className="trace-preview">
          <svg
            ref={svg}
            viewBox={`${vb.x} ${vb.y} ${vb.w} ${vb.h}`}
            preserveAspectRatio="xMidYMid meet"
            onPointerDown={(e) => {
              const f = fieldAt(e);
              if (!f) return;
              (e.target as Element).setPointerCapture?.(e.pointerId);
              setDragFrom(f);
              setDragTo(f);
            }}
            onPointerMove={(e) => dragFrom && setDragTo(fieldAt(e))}
            onPointerUp={endDrag}
            role="img"
            aria-label={t("files.tracePreview")}
          >
            <g transform="scale(1,-1)">
              <g transform={`matrix(${m.join(" ")})`} opacity={fade ? 0.25 : 1}>
                <image href={originalHref(g)} x={-g.width_mm / 2} y={-g.height_mm / 2} width={g.width_mm} height={g.height_mm} transform="scale(1,-1)" preserveAspectRatio="none" />
              </g>
              {preview && <path className="trace-path" d={preview.d} strokeWidth={strokeW} fill="none" fillRule="evenodd" />}
              {pts.map(([x, y], i) => (
                <rect key={i} className="trace-point" x={x - strokeW * 1.5} y={y - strokeW * 1.5} width={strokeW * 3} height={strokeW * 3} />
              ))}
              {bnd && <rect className="trace-boundary" x={bnd[0]} y={bnd[1]} width={bnd[2] - bnd[0]} height={bnd[3] - bnd[1]} strokeWidth={strokeW} />}
              {live && <rect className="trace-boundary" x={live[0]} y={live[1]} width={live[2] - live[0]} height={live[3] - live[1]} strokeWidth={strokeW} />}
            </g>
          </svg>
          <div className="btn-row compact">
            <button className={`btn${fade ? " active" : ""}`} aria-pressed={fade} onClick={() => setFade((v) => !v)}>
              {t("files.fadeImage")}
            </button>
            <button className={`btn${points ? " active" : ""}`} aria-pressed={points} onClick={() => setPoints((v) => !v)}>
              {t("files.showPoints")}
            </button>
            <button className="btn" disabled={!bnd} onClick={() => set("boundary", null)}>
              {t("files.clearBoundary")}
            </button>
          </div>
          <p className="hint">{preview ? t("files.traceStats", { contours: preview.contours, nodes: preview.nodes }) : t("files.tracing")}</p>
        </div>
        <div className="trace-controls">
          <Slider label={t("files.cutoff")} value={o.cutoff} min={0} max={255} step={1} onChange={(v) => set("cutoff", Math.round(v))} />
          <Slider label={t("files.threshold")} value={o.threshold} min={0} max={255} step={1} onChange={(v) => set("threshold", Math.round(v))} />
          <Slider label={t("files.ignoreLess")} value={o.ignoreLessThan} min={0} max={1000} step={1} onChange={(v) => set("ignoreLessThan", Math.round(v))} />
          <Slider label={t("files.smoothness")} value={o.smoothness} min={0} max={1.33} step={0.01} onChange={(v) => set("smoothness", v)} />
          <Slider label={t("files.optimize")} value={o.optimize} min={0} max={5} step={0.05} onChange={(v) => set("optimize", v)} />
          <CheckField label={t("files.traceTransparency")} checked={o.transparency} onChange={(v) => set("transparency", v)} />
          <CheckField label={t("files.centerLine")} checked={o.centerline} onChange={(v) => set("centerline", v)} />
          <CheckField label={t("files.sketchTrace")} checked={o.sketch} onChange={(v) => set("sketch", v)} />
          <CheckField label={t("files.deleteAfter")} checked={o.deleteImage} onChange={(v) => set("deleteImage", v)} />
          <p className="hint">{t("files.traceHint")}</p>
        </div>
      </div>
    </Modal>
  );
}

export function ConvertBitmapDialog(p: { objects: DocObject[]; run: Run; report: (e: unknown) => void; onDone: (created: string[]) => void; onClose: () => void }) {
  const t = useT();
  const [dpi, setDpi] = useBox(DEFAULT_BITMAP.dpi);
  const [render, setRender] = useState<BitmapOptions["render"]>("cut_settings");
  const [gray, setGray] = useState(DEFAULT_BITMAP.gray);
  const [lw, setLw] = useBox(null);
  const [keep, setKeep] = useState(false);
  const [busy, setBusy] = useState(false);
  const d = parseBox(dpi);
  const w = parseBox(lw);
  const box = unionBoxes(p.objects.map(objectBounds));
  const px = box && d && d > 0 ? bitmapPixels(box.x1 - box.x0, box.y1 - box.y0, d) : null;
  const problem = !(d !== null && d >= 10 && d <= 2540) ? t("files.badDpi") : w !== null && !(w > 0 && w <= 50) ? t("files.badLineWidth") : px && px[0] * px[1] > 100e6 ? t("files.bitmapTooBig") : null;
  const ok = () => {
    if (problem || busy) return;
    setBusy(true);
    p.run({ id: IMAGE_CMD.toBitmap, params: bitmapParams(p.objects.map((o) => o.id), { dpi: d ?? 254, render, gray, lineWidthMm: w, keepOriginal: keep }) })
      .then((r) => {
        const c = r && typeof r === "object" ? (r as { created?: unknown }).created : null;
        p.onDone(Array.isArray(c) ? c.filter((x): x is string => typeof x === "string") : []);
      })
      .catch(p.report)
      .finally(() => setBusy(false));
  };
  return (
    <Modal
      title={t("files.toBitmapTitle")}
      onClose={p.onClose}
      onSubmit={ok}
      footer={
        <>
          {problem && <span className="form-error">{problem}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!!problem || busy} onClick={ok} autoFocus>
            {t("files.ok")}
          </button>
        </>
      }
    >
      <p className="hint">{t("files.toBitmapCount", { n: p.objects.length })}</p>
      <NumBox label={t("files.dpi")} value={dpi} onChange={setDpi} />
      {px && <p className="hint">{t("files.bitmapSize", { w: px[0], h: px[1] })}</p>}
      <Row label={t("files.render")}>
        <select value={render} onChange={(e) => setRender(e.target.value as BitmapOptions["render"])}>
          <option value="cut_settings">{t("files.renderCut")}</option>
          <option value="fill_all">{t("files.renderFill")}</option>
          <option value="outlines">{t("files.renderOutlines")}</option>
        </select>
      </Row>
      <Slider label={t("files.grey")} value={gray} min={0} max={255} step={1} onChange={(v) => setGray(Math.round(v))} />
      <MmBox label={t("files.lineWidth")} value={lw} onChange={setLw} title={t("files.lineWidthHint")} />
      <CheckField label={t("files.keepOriginal")} checked={keep} onChange={setKeep} />
      <p className="hint">{t("files.toBitmapHint")}</p>
    </Modal>
  );
}
