// The 3D relief preview (comparison: xTool's 3D preview, Rayforge's 3D simulation): a WebGL height view of what the
// job removes, for depth-map and 3D Sliced image layers and Z-stepped passes, with a scrubber over the job's time,
// to check a coin's relief before a long burn. Opened from the Laser preview's bar (the mountain button) and the
// "3D relief preview" command (hotkeys.ts laser.relief3d). Display only: it reads the preview the Laser workspace
// already has and never talks to the engine or the laser.
//
// Mouse: drag turns, right-drag or Shift+drag pans, the wheel zooms. Touch: one finger turns, two fingers pan and
// pinch. Keys (on the view): arrows turn, + / - zoom, 0 resets.

import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { clockText, segmentTimes, segmentsDoneAt, timeOfSegments } from "../api/job.ts";
import { packSegments, segmentCount } from "../api/packedPreview.ts";
import type { GDocument, Preview } from "../api/types.ts";
import { Icon } from "../components/ui.tsx";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { useT } from "../workspace/usePrefs.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { ReliefGL, fitCamera, type Camera } from "./reliefGL.ts";
import { autoExaggeration, depthText, fieldNormals, includedPasses, reliefArrays, reliefPasses, type PassKind, type ReliefField } from "./reliefModel.ts";
import type { FromRelief, ToRelief } from "./reliefProtocol.ts";
import "./relief.css";

// ---- open / close from anywhere (the preview bar, command search) ----------------------------------------------

let isOpen = false;
const subs = new Set<() => void>();
const emit = () => {
  for (const f of [...subs]) f();
};

export function openRelief(): void {
  isOpen = true;
  emit();
}

export function closeRelief(): void {
  isOpen = false;
  emit();
}

export function useReliefOpen(): boolean {
  return useSyncExternalStore(
    (f) => {
      subs.add(f);
      return () => void subs.delete(f);
    },
    () => isOpen,
    () => false,
  );
}

const EXAGGERATIONS = [1, 2, 5, 10, 20, 50, 100, 200, 500];

/** The preview bar's button. */
export function ReliefButton() {
  const t = useT();
  return (
    <button className="icon-btn small" title={t("rel.openTitle")} aria-label={t("rel.open")} onClick={openRelief}>
      <Icon name="mountain" size={16} />
    </button>
  );
}

/** Mounted at the app's root with the other dialogs: shows the dialog while it is open and the Laser preview is on. */
export function ReliefHost(p: { preview: Preview | null; doc: GDocument | null; active: boolean }) {
  const open = useReliefOpen();
  // Leaving the Laser preview closes it (it shows that preview).
  useEffect(() => {
    if (open && !p.active) closeRelief();
  }, [open, p.active]);
  if (!open || !p.active || !p.preview) return null;
  return <ReliefDialog preview={p.preview} doc={p.doc} onClose={closeRelief} />;
}

function kindText(t: ReturnType<typeof useT>, k: PassKind): string {
  switch (k) {
    case "slice3d":
      return t("rel.kindSlice");
    case "grayscale":
      return t("rel.kindGray");
    case "zstep":
      return t("rel.kindZ");
    default:
      return t("rel.kindOther");
  }
}

function cssVar(name: string, fallback: string): string {
  try {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || fallback;
  } catch {
    return fallback;
  }
}

export function ReliefDialog(p: { preview: Preview; doc: GDocument | null; onClose: () => void }) {
  const t = useT();
  const phone = useLayoutMode().layout === "phone";
  const { preview } = p;
  const [depthUm, setDepthUm] = useState(10);
  const [reliefOnly, setReliefOnly] = useState(true);
  const [exag, setExag] = useState<number | null>(null);
  const [ramp, setRamp] = useState(true);
  const [upTo, setUpTo] = useState<number | null>(null);
  const [clock, setClock] = useState<number | null>(null);
  const [playing, setPlaying] = useState(false);
  const [field, setField] = useState<ReliefField | null>(null);
  const [fullDepth, setFullDepth] = useState(0);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const gl = useRef<ReliefGL | null>(null);
  const cam = useRef<Camera | null>(null);
  const worker = useRef<Worker | null>(null);
  const reqId = useRef(0);
  const fullId = useRef(-1);
  const frame = useRef(0);

  const times = useMemo(() => segmentTimes(preview), [preview]);
  const total = segmentCount(preview);
  const duration = times.length ? times[times.length - 1] : 0;
  const passes = useMemo(() => reliefPasses(preview.passes ?? [], p.doc?.layers ?? [], depthUm / 1000), [preview, p.doc, depthUm]);
  const { include, fallback } = useMemo(() => includedPasses(passes, reliefOnly), [passes, reliefOnly]);
  const shownLayers = useMemo(() => {
    const out: { id: string; name: string; color: string; kind: PassKind; passes: number }[] = [];
    passes.forEach((ps, i) => {
      if (!include[i]) return;
      const l = p.doc?.layers.find((x) => x.id === ps.layerId);
      const have = out.find((o) => o.id === ps.layerId);
      if (have) have.passes += 1;
      else out.push({ id: ps.layerId, name: l?.name ?? ps.layerId, color: l?.color ?? "#888", kind: ps.kind, passes: 1 });
    });
    return out;
  }, [passes, include, p.doc]);
  const allZ = passes.length > 0 && passes.every((ps, i) => !include[i] || ps.kind === "zstep");

  const redraw = () => {
    cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => {
      if (gl.current && cam.current) gl.current.draw(cam.current, fullDepth > 0 ? fullDepth : (field?.maxDepth ?? 0), cssVar("--canvas", "#202226"));
    });
  };

  // The renderer, once the canvas is there.
  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    try {
      gl.current = new ReliefGL(c);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return;
    }
    const lost = (ev: Event) => {
      ev.preventDefault();
      setError(t("rel.contextLost"));
    };
    c.addEventListener("webglcontextlost", lost);
    return () => {
      c.removeEventListener("webglcontextlost", lost);
      gl.current?.dispose();
      gl.current = null;
    };
  }, []);

  // The worker: the job's arrays whenever the preview or the passes drawn change, then the whole job's field.
  useEffect(() => {
    let w: Worker;
    try {
      w = new Worker(new URL("./reliefWorker.ts", import.meta.url), { type: "module", name: "openetch-relief" });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return;
    }
    worker.current = w;
    w.onmessage = (e: MessageEvent<FromRelief>) => {
      const m = e.data;
      if (m.id !== reqId.current && m.id !== fullId.current) return;
      if (m.type === "error") {
        setError(m.error);
        setBusy(false);
        return;
      }
      if (m.id === fullId.current) setFullDepth(m.field?.maxDepth ?? 0);
      if (m.id === reqId.current) {
        setField(m.field);
        setBusy(false);
      }
    };
    return () => {
      w.terminate();
      worker.current = null;
    };
  }, []);

  useEffect(() => {
    const w = worker.current;
    if (!w || total === 0) {
      setBusy(false);
      return;
    }
    const packed = preview.packed ?? packSegments(preview.segments);
    const msg: ToRelief = { type: "load", input: { ...reliefArrays(packed, preview.rasters), passes }, include, maxSide: phone ? 224 : 360 };
    w.postMessage(msg);
    setBusy(true);
    // The whole job first (it fixes the colour ramp and the exaggeration), then the scrubber's position.
    fullId.current = ++reqId.current;
    w.postMessage({ type: "build", id: fullId.current, upTo: null } satisfies ToRelief);
    if (upTo !== null) {
      reqId.current += 1;
      w.postMessage({ type: "build", id: reqId.current, upTo } satisfies ToRelief);
    }
  }, [preview, passes, include, phone]);

  // Scrubbing builds the field up to the new position (the worker skips stale requests).
  const scrubTo = (n: number | null) => {
    setUpTo(n);
    const w = worker.current;
    if (!w) return;
    reqId.current += 1;
    w.postMessage({ type: "build", id: reqId.current, upTo: n } satisfies ToRelief);
  };

  // A new field: the camera fits it the first time, the exaggeration starts where the relief shows.
  useEffect(() => {
    if (!field || !gl.current) {
      redraw();
      return;
    }
    let k = exag;
    if (k === null) {
      k = autoExaggeration({ ...field, maxDepth: Math.max(field.maxDepth, fullDepth) });
      setExag(k);
    }
    if (!cam.current) cam.current = fitCamera(field);
    gl.current.ramp = ramp;
    gl.current.setField(field, fieldNormals(field, k), k);
    redraw();
  }, [field, exag, ramp, fullDepth]);

  // Play: the scrubber runs through the whole job in about 20 s.
  useEffect(() => {
    if (!playing || !(duration > 0)) return;
    let last = performance.now();
    let pos = clock ?? (upTo === null ? 0 : timeOfSegments(times, upTo));
    if (pos >= duration) pos = 0;
    const speed = duration / 20;
    const id = window.setInterval(() => {
      const now = performance.now();
      pos += ((now - last) / 1000) * speed;
      last = now;
      if (pos >= duration) {
        window.clearInterval(id);
        setClock(null);
        scrubTo(null);
        setPlaying(false);
        return;
      }
      setClock(pos);
      scrubTo(segmentsDoneAt(times, pos));
    }, 120);
    return () => window.clearInterval(id);
  }, [playing]);

  useEffect(() => {
    const onResize = () => redraw();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  });

  // ---- camera gestures ----
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const gesture = useRef<{ dist: number; mid: { x: number; y: number } } | null>(null);
  const orbit = (dx: number, dy: number) => {
    const c = cam.current;
    if (!c) return;
    c.yaw -= dx * 0.008;
    c.pitch = Math.min(1.45, Math.max(0, c.pitch - dy * 0.008));
    redraw();
  };
  const pan = (dx: number, dy: number) => {
    const c = cam.current;
    const el = canvas.current;
    if (!c || !el) return;
    const k = (c.dist * 0.63) / Math.max(1, el.clientHeight);
    const cy = Math.cos(c.yaw);
    const sy = Math.sin(c.yaw);
    // Screen right is (cos yaw, sin yaw); screen up on the ground is (-sin yaw, cos yaw).
    c.tx -= (dx * cy - dy * -sy) * k;
    c.ty -= (dx * sy - dy * cy) * k;
    redraw();
  };
  const zoom = (factor: number) => {
    const c = cam.current;
    if (!c) return;
    c.dist = Math.min(1e5, Math.max(0.5, c.dist * factor));
    redraw();
  };
  const reset = (pitchDeg?: number) => {
    if (!field) return;
    cam.current = fitCamera(field, pitchDeg);
    redraw();
  };

  const onPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId);
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    gesture.current = null;
  };
  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const prev = pointers.current.get(e.pointerId);
    if (!prev) return;
    const now = { x: e.clientX, y: e.clientY };
    pointers.current.set(e.pointerId, now);
    const all = [...pointers.current.values()];
    if (all.length >= 2) {
      const [a, b] = all;
      const dist = Math.hypot(a.x - b.x, a.y - b.y);
      const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      const g = gesture.current;
      if (g) {
        if (dist > 0 && g.dist > 0) zoom(g.dist / dist);
        pan(mid.x - g.mid.x, mid.y - g.mid.y);
      }
      gesture.current = { dist, mid };
      return;
    }
    const dx = now.x - prev.x;
    const dy = now.y - prev.y;
    if (e.shiftKey || (e.buttons & 2) !== 0 || (e.buttons & 4) !== 0) pan(dx, dy);
    else orbit(dx, dy);
  };
  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    pointers.current.delete(e.pointerId);
    gesture.current = null;
  };
  const onKey = (e: React.KeyboardEvent<HTMLCanvasElement>) => {
    const k = e.key;
    if (k === "ArrowLeft") orbit(-20, 0);
    else if (k === "ArrowRight") orbit(20, 0);
    else if (k === "ArrowUp") orbit(0, -20);
    else if (k === "ArrowDown") orbit(0, 20);
    else if (k === "+" || k === "=") zoom(0.85);
    else if (k === "-") zoom(1 / 0.85);
    else if (k === "0") reset();
    else return;
    e.preventDefault();
  };
  // Wheel zoom without scrolling the dialog (a passive React listener cannot prevent it).
  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      zoom(Math.exp(e.deltaY * (e.deltaMode === 1 ? 0.05 : 0.0015)));
    };
    c.addEventListener("wheel", onWheel, { passive: false });
    return () => c.removeEventListener("wheel", onWheel);
  }, []);

  const done = upTo === null ? total : Math.min(total, upTo);
  const at = clock ?? timeOfSegments(times, done);
  const step = duration > 0 ? Math.max(0.001, duration / 1000) : 1;
  const scrub = (v: number) => {
    setPlaying(false);
    if (v + step > duration) {
      setClock(null);
      scrubTo(null);
      return;
    }
    setClock(v);
    scrubTo(segmentsDoneAt(times, v));
  };
  const deepest = field?.maxDepth ?? 0;
  const empty = !busy && !error && (total === 0 || (!field && fullDepth === 0));
  const label = t("rel.viewLabel", { depth: depthText(deepest), layers: shownLayers.map((l) => l.name).join(", ") || "—" });

  return (
    <WsModal help="relief-preview#relief-window" title={t("rel.title")} size={phone ? "full" : "xl"} onClose={p.onClose}>
      <div className="rel-body">
        <div className="rel-stage">
          <canvas
            ref={canvas}
            className="rel-canvas"
            tabIndex={0}
            role="img"
            aria-label={label}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onPointerCancel={onPointerUp}
            onContextMenu={(e) => e.preventDefault()}
            onKeyDown={onKey}
          />
          {(busy || error || empty) && (
            <div className={`rel-overlay${error ? " rel-error" : ""}`} role={error ? "alert" : "status"}>
              {error ?? (empty ? t("rel.empty") : t("rel.building"))}
            </div>
          )}
          <div className="rel-views" role="group" aria-label={t("rel.views")}>
            <button className="btn small" onClick={() => reset(0)} title={t("rel.topTitle")}>
              {t("rel.top")}
            </button>
            <button className="btn small" onClick={() => reset(55)} title={t("rel.tiltTitle")}>
              {t("rel.tilt")}
            </button>
            <button className="btn small" onClick={() => reset(78)} title={t("rel.sideTitle")}>
              {t("rel.side")}
            </button>
          </div>
          {deepest > 0 && (
            <div className="rel-legend" aria-hidden="true">
              <span>0</span>
              <span className={`rel-ramp${ramp ? "" : " flat"}`} />
              <span>{depthText(fullDepth || deepest)}</span>
            </div>
          )}
        </div>

        <div className="rel-player" role="group" aria-label={t("rel.time")}>
          <button
            className="icon-btn small"
            title={playing ? t("pvt.pause") : t("pvt.play")}
            aria-label={playing ? t("pvt.pause") : t("pvt.play")}
            disabled={total === 0}
            onClick={() => {
              if (!playing && upTo === null) {
                setClock(0);
                scrubTo(0);
              }
              setPlaying((v) => !v);
            }}
          >
            <Icon name={playing ? "pause" : "play"} size={16} />
          </button>
          <input
            type="range"
            min={0}
            max={duration > 0 ? duration : 1}
            step={step}
            value={duration > 0 ? Math.min(at, duration) : 1}
            aria-label={t("rel.time")}
            aria-valuetext={`${clockText(at)} / ${clockText(duration)}`}
            disabled={total === 0}
            onChange={(e) => scrub(Number(e.target.value))}
          />
          <span className="rel-clock">
            {clockText(Math.min(at, duration))} / {clockText(duration)}
          </span>
        </div>

        <div className="rel-controls">
          <label className="lx-inline">
            {t("rel.exaggeration")}
            <select value={exag ?? 1} onChange={(e) => setExag(Number(e.target.value))}>
              {EXAGGERATIONS.map((k) => (
                <option key={k} value={k}>
                  {k}×
                </option>
              ))}
            </select>
          </label>
          <label className="lx-inline" title={allZ ? t("rel.depthZTitle") : t("rel.depthTitle")}>
            {t("rel.depthPerPass")}
            <select value={depthUm} disabled={allZ} onChange={(e) => setDepthUm(Number(e.target.value))}>
              {[1, 2, 5, 10, 20, 50, 100].map((k) => (
                <option key={k} value={k}>
                  {k} µm
                </option>
              ))}
            </select>
          </label>
          <label className="laser-check">
            <input type="checkbox" checked={reliefOnly} onChange={(e) => setReliefOnly(e.target.checked)} />
            {t("rel.reliefOnly")}
          </label>
          <label className="laser-check">
            <input type="checkbox" checked={ramp} onChange={(e) => setRamp(e.target.checked)} />
            {t("rel.colourByDepth")}
          </label>
        </div>

        <p className="rel-stats" role="status">
          {t("rel.deepest", { depth: depthText(deepest), total: depthText(fullDepth) })}
          {shownLayers.map((l) => (
            <span key={l.id} className="rel-layer">
              <span className="rel-swatch" style={{ background: l.color }} />
              {t("rel.layerLine", { name: l.name, kind: kindText(t, l.kind), passes: l.passes })}
            </span>
          ))}
        </p>
        {fallback && <p className="hint">{t("rel.fallback")}</p>}
        <p className="hint">{t("rel.note")}</p>
      </div>
    </WsModal>
  );
}
