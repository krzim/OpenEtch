// The Fill Region tool on the canvas (a paint bucket; the state machine and engine calls are in fillRegion.ts): the
// ribbon button (MODIFY, so also the tablet rail and the phone strip), the options bar in the context strip (target
// layer, gap tolerance, islands as holes, status and errors), the highlight of the area under the pointer, the
// collected areas and the touch "armed" area, and the keys (Enter fills the collection, Esc clears it or leaves).
//
// Hover asks `path.fill_region_preview` (read-only) after a short pause; faces it returns are cached for the
// document's revision, so moving inside an area already found asks nothing. Filling is one `path.fill_region` command
// (one undo step) for every collected area.

import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { execute, type Invocation } from "../api/client.ts";
import { createdIds } from "../api/drop.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import type { CanvasTool } from "../canvasTool.ts";
import { RibbonSlot } from "../components/Ribbon.tsx";
import type { Tool } from "../components/Viewport.tsx";
import { Icon } from "../components/ui.tsx";
import { errText } from "../libs/ui.tsx";
import { TOL } from "../pointer.ts";
import { ContextStripPortal } from "../workspace/contextStrip.tsx";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { usePrefs, useT, useUnitConversion } from "../workspace/usePrefs.ts";
import { trimNumber } from "../workspace/units.ts";
import {
  DEFAULT_FILL_OPTS,
  areaText,
  FILL_CMD,
  FaceCache,
  IDLE,
  MAX_GAP_MM,
  facePathData,
  fillErrorText,
  fillPreview,
  fillRegion,
  onAdd,
  onClick,
  onCommitted,
  onHover,
  onKey,
  onPreview,
  parsePreview,
  readyCount,
  type Face,
  type FillEffect,
  type FillOpts,
  type FillState,
  type PointAnswer,
} from "./fillRegion.ts";
import "./fill.css";

export interface FillToolProps {
  doc: GDocument | null;
  tool: Tool;
  setTool: (t: Tool) => void;
  activeLayer: string | null;
  design: boolean;
  has: (id: string) => boolean;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  notify: (msg: string, kind?: "info" | "error") => void;
  selectIds: (ids: string[]) => void;
  /** The tool turned on: other canvas modes (Add Tabs) step aside. */
  onStart?: () => void;
}

/** Pause before a hover asks the engine, ms. */
const HOVER_MS = 120;

function typingIn(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

export function useFillRegionTool(p: FillToolProps) {
  const t = useT();
  const P = useRef(p);
  P.current = p;
  const on = p.tool === "fill";
  const supported = p.has(FILL_CMD.fill) && p.has(FILL_CMD.preview);
  const [st, setSt] = useState<FillState>(IDLE);
  const S = useRef(st);
  S.current = st;
  const [opts, setOpts] = useState<FillOpts>(DEFAULT_FILL_OPTS);
  const O = useRef(opts);
  O.current = opts;
  // The layer picked in the bar; null follows the current layer.
  const [layerPick, setLayerPick] = useState<string | null>(null);
  const cache = useRef(new FaceCache());
  const hoverTimer = useRef(0);
  const seq = useRef(0);
  const touchUi = useRef(false);
  const press = useRef<{ at: Vec2; screen: Vec2; moved: boolean } | null>(null);

  const layers = p.doc?.layers ?? [];
  const target = (layerPick && layers.some((l) => l.id === layerPick) ? layerPick : null) ?? (p.activeLayer && layers.some((l) => l.id === p.activeLayer) ? p.activeLayer : null) ?? layers[0]?.id ?? null;
  const targetLayer = layers.find((l) => l.id === target) ?? null;
  // Read at fill time: the key handler keeps the closure of the render that installed it.
  const TARGET = useRef({ id: target, name: targetLayer?.name ?? target ?? "" });
  TARGET.current = { id: target, name: targetLayer?.name ?? target ?? "" };

  const set = (s: FillState) => {
    S.current = s;
    setSt(s);
  };

  // The cache belongs to one revision and one set of options; a change asks again for what is shown.
  const revision = p.doc?.revision ?? -1;
  useEffect(() => {
    if (!cache.current.sync(revision, opts)) return;
    const s = S.current;
    const stale = (k: { at: Vec2; face: Face | null } | null) => (k ? { at: k.at, face: null } : null);
    const next: FillState = { ...s, hover: stale(s.hover), armed: stale(s.armed), pending: s.pending.map((k) => ({ at: k.at, face: null })) };
    set(next);
    for (const k of [next.hover, next.armed, ...next.pending]) if (k) ask(k.at);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revision, opts]);

  // Leaving the tool (or the Design workspace) forgets everything it showed.
  useEffect(() => {
    if (on) {
      P.current.onStart?.();
      return;
    }
    window.clearTimeout(hoverTimer.current);
    seq.current++;
    press.current = null;
    if (S.current !== IDLE) set(IDLE);
  }, [on]);
  useEffect(() => {
    if (!p.design && on) p.setTool("select");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.design]);

  const answerText = (a: PointAnswer): string | null => {
    if ("face" in a) return null;
    return a.code === "not_enclosed" ? t("fill.notEnclosed") : a.code === "on_line" ? t("fill.onLine") : a.error;
  };

  /** Ask the engine for the face at `at` and fold the answer in. */
  const ask = (at: Vec2) => {
    // Answers for points no longer shown are ignored by onPreview; answers that arrive after the tool was left (the
    // epoch moved on) are dropped here.
    const epoch = seq.current;
    void execute(fillPreview([at], O.current))
      .then((r) => {
        if (epoch !== seq.current) return;
        const [a] = parsePreview(r.result, 1);
        if ("face" in a) cache.current.add(a.face);
        const shown: PointAnswer = "face" in a ? a : { error: answerText(a) ?? a.error, code: a.code };
        set(onPreview(S.current, at, shown));
      })
      .catch((e: unknown) => epoch === seq.current && set(onPreview(S.current, at, { error: errText(e), code: "error" })));
  };

  const commit = (points: Vec2[]) => {
    const { id: layer, name } = TARGET.current;
    void P.current
      .run(fillRegion(points, layer, O.current))
      .then((r) => {
        set(onCommitted(S.current, null));
        const ids = createdIds(r);
        if (ids.length) P.current.selectIds(ids);
        P.current.notify(t("fill.filled", { n: ids.length, layer: name }));
      })
      .catch((e: unknown) => set(onCommitted(S.current, fillErrorText(errText(e)))));
  };

  const apply = ([s, fx]: [FillState, FillEffect]) => {
    set(s);
    switch (fx.kind) {
      case "preview":
        window.clearTimeout(hoverTimer.current);
        if (fx.why === "hover") {
          const at = fx.at;
          hoverTimer.current = window.setTimeout(() => S.current.hover?.at === at && ask(at), HOVER_MS);
        } else ask(fx.at);
        return;
      case "commit":
        window.clearTimeout(hoverTimer.current);
        commit(fx.points);
        return;
      case "exit":
        P.current.setTool("select");
        return;
    }
  };

  // Enter and Esc while the tool is on (before App's own Esc, which would leave the tool at once).
  useEffect(() => {
    if (!on) return;
    const key = (e: KeyboardEvent) => {
      if ((e.key !== "Enter" && e.key !== "Escape") || typingIn(e.target) || e.repeat) return;
      const r = onKey(S.current, e.key);
      if (!r) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      apply(r);
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [on]);

  const wrap = (inner: CanvasTool): CanvasTool => {
    if (!on) return inner;
    return {
      ...inner,
      down: (f, m, info) => {
        const touch = info.pointerType === "touch";
        touchUi.current = touch;
        if (touch) {
          // A finger commits on lift: a drag or a second finger (pinch) fills nothing.
          press.current = { at: f, screen: info.screen, moved: false };
          return true;
        }
        apply(onClick(S.current, { at: f, shift: m.shift, touch: false, known: cache.current.find(f) }));
        return true;
      },
      drag: (f, _m, ctx) => {
        const pr = press.current;
        if (!pr) return;
        const s = ctx.toScreen(f);
        const slop = (ctx.tol ?? TOL.touch).slop;
        if (Math.hypot(s[0] - pr.screen[0], s[1] - pr.screen[1]) > slop) pr.moved = true;
      },
      up: (_f, m, ctx) => {
        const pr = press.current;
        press.current = null;
        if (!pr || pr.moved) return;
        const tolMm = (ctx.tol ?? TOL.touch).click / Math.max(ctx.scale, 1e-6);
        apply(onClick(S.current, { at: pr.at, shift: m.shift, touch: true, known: cache.current.find(pr.at), tolMm }));
      },
      cancel: () => {
        press.current = null;
      },
      hover: (f, _m, ctx) => {
        touchUi.current = false;
        ctx.showSnap(null);
        apply(onHover(S.current, f, cache.current.find(f)));
        return true;
      },
      render: (r) => (
        <>
          {inner.render(r)}
          <FillHighlight st={st} color={targetLayer?.color ?? null} strokePx={r.px(1.5)} />
        </>
      ),
      overlay: (r) => (
        <>
          {inner.overlay(r)}
          <FillBar
            st={st}
            layers={layers}
            target={target}
            opts={opts}
            touch={touchUi.current}
            onLayer={setLayerPick}
            onOpts={setOpts}
            onAdd={() => apply(onAdd(S.current))}
            onClear={() => apply(onKey(S.current, "Escape") ?? [S.current, { kind: "none" }])}
            onDone={() => P.current.setTool("select")}
          />
        </>
      ),
    };
  };

  const toolbar: ReactNode = p.design ? (
    <RibbonSlot group="modify" order={11}>
      <button
        type="button"
        className={`tool-btn${on ? " active" : ""}`}
        onClick={() => p.setTool(on ? "select" : "fill")}
        disabled={!supported}
        aria-pressed={on}
        aria-label={t("fill.tool")}
        title={supported ? t("fill.toolTitle") : t("fill.unsupported")}
      >
        <Icon name="fillRegion" size={20} />
        <span className="tool-caption">{t("fill.tool")}</span>
      </button>
    </RibbonSlot>
  ) : null;

  const handlers: Record<string, () => boolean | void> = {
    "tool.fillRegion": () => (p.design && supported ? p.setTool("fill") : false),
  };

  return { wrap, toolbar, handlers, supported };
}

/** The area under the pointer, the armed one and the collected ones, in the field frame. */
function FillHighlight(p: { st: FillState; color: string | null; strokePx: number }) {
  const { st } = p;
  const faces: { key: string; face: Face; cls: string }[] = [];
  st.pending.forEach((k, i) => k.face && faces.push({ key: `p${i}`, face: k.face, cls: "fr-pending" }));
  if (st.armed?.face) faces.push({ key: "armed", face: st.armed.face, cls: "fr-armed" });
  const shown = faces.map((f) => f.face);
  if (st.hover?.face && !shown.includes(st.hover.face)) faces.push({ key: "hover", face: st.hover.face, cls: "fr-hover" });
  if (faces.length === 0) return null;
  const style = p.color ? ({ ["--fr-color" as string]: p.color } as CSSProperties) : undefined;
  return (
    <g className="fr-layer" pointerEvents="none" style={style}>
      {faces.map((f) => (
        <path key={f.key} className={`fr-face ${f.cls}`} d={facePathData(f.face)} fillRule="evenodd" strokeWidth={p.strokePx} />
      ))}
    </g>
  );
}

function FillBar(p: {
  st: FillState;
  layers: GDocument["layers"];
  target: string | null;
  opts: FillOpts;
  touch: boolean;
  onLayer: (id: string) => void;
  onOpts: (o: FillOpts) => void;
  onAdd: () => void;
  onClear: () => void;
  onDone: () => void;
}) {
  const t = useT();
  const { units } = usePrefs();
  const coarse = useLayoutMode().pointer === "coarse";
  const { st } = p;
  const ready = readyCount(st);
  const swatch = p.layers.find((l) => l.id === p.target)?.color;
  const shownFace = st.armed?.face ?? st.hover?.face ?? null;
  const looking = (!!st.armed && !st.armed.face) || st.pending.some((k) => !k.face);
  let status: ReactNode;
  if (st.error) status = <span className="fr-error" role="alert">{st.error}</span>;
  else if (looking) status = <span className="et-hint">{t("fill.looking")}</span>;
  else if (shownFace) status = <span className="et-hint">{t("fill.area", { area: areaText(shownFace.areaMm2, units) })}</span>;
  else status = <span className="et-hint">{p.touch || coarse ? t("fill.hintTouch") : t("fill.hintMouse")}</span>;
  return (
    <ContextStripPortal>
      <div className="vp-overlay et-bar fr-bar" role="toolbar" aria-label={t("fill.tool")}>
        <b>{t("fill.tool")}</b>
        <label className="et-opt" title={t("fill.layerTitle")}>
          <span className="fr-swatch" style={swatch ? { background: swatch } : undefined} aria-hidden="true" />
          {t("fill.layer")}
          <select value={p.target ?? ""} onChange={(e) => p.onLayer(e.target.value)}>
            {p.layers.map((l) => (
              <option key={l.id} value={l.id}>
                {l.name}
              </option>
            ))}
          </select>
        </label>
        <GapField value={p.opts.gapMm} onCommit={(gapMm) => p.onOpts({ ...p.opts, gapMm })} />
        <label className="et-opt" title={t("fill.holesTitle")}>
          <input type="checkbox" checked={p.opts.holes} onChange={(e) => p.onOpts({ ...p.opts, holes: e.target.checked })} />
          {t("fill.holes")}
        </label>
        {st.pending.length > 0 && <span className="et-hint">{t("fill.pending", { n: st.pending.length })}</span>}
        {status}
        {(ready > 0 || st.armed) && (
          <button type="button" className="btn small btn-primary" disabled={ready === 0 || st.busy} onClick={p.onAdd}>
            {ready > 1 ? t("fill.addN", { n: ready }) : t("fill.add")}
          </button>
        )}
        {(st.pending.length > 0 || st.armed || st.error) && (
          <button type="button" className="btn small" onClick={p.onClear}>
            {t("fill.clear")}
          </button>
        )}
        <button type="button" className="btn small" onClick={p.onDone}>
          {t("fill.done")}
        </button>
      </div>
    </ContextStripPortal>
  );
}

/** The gap tolerance in the display unit (mm or inches). */
function GapField(p: { value: number; onCommit: (mm: number) => void }) {
  const t = useT();
  const conv = useUnitConversion("mm");
  const shown = trimNumber(conv.to(p.value), 3 + conv.extraDigits);
  const [text, setText] = useState<string | null>(null);
  const commit = () => {
    if (text === null) return;
    const n = Number(text.replace(",", "."));
    setText(null);
    if (!Number.isFinite(n)) return;
    const mm = Math.min(MAX_GAP_MM, Math.max(0, conv.from(n)));
    if (Math.abs(mm - p.value) > 1e-9) p.onCommit(mm);
  };
  return (
    <label className="et-opt" title={t("fill.gapTitle")}>
      {t("fill.gap")}
      <input
        type="text"
        inputMode="decimal"
        className="fr-gap"
        value={text ?? shown}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") commit();
          if (e.key === "Escape") setText(null);
        }}
      />
      <span className="unit">{conv.unit}</span>
    </label>
  );
}
