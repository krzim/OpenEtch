// LightBurn's Camera Control for a galvo (unverified on hardware): a frame from the engine's UVC camera (builds with
// the `camera` feature), from this browser's camera or from an image file; Calibrate Camera Lens (checkerboard views,
// detected by the engine, then a two-term radial fit); Align Camera (marks added to the design and burned, then their
// image positions clicked, at least four); and the camera image drawn under the design on the canvas with an opacity
// slider (GET /api/camera/overlay.png, the frame resampled into the field square). Shown only when the engine has the
// camera routes. Calibrations are per device and only from the UI.

import { useEffect, useRef, useState } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument, MachineStatus } from "../api/types.ts";
import { CheckField, NumberField } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { libsApi } from "./api.ts";
import { cameraOverlay } from "./store.ts";
import { FilePick, errText } from "./ui.tsx";
import "./libs.css";

type Notify = (msg: string, kind?: "info" | "error") => void;
type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

interface CameraInfo {
  capture: boolean;
  frame: { width: number; height: number; unix_s: number } | null;
  lens: { k1: number; k2: number; rms_px: number } | null;
  alignment: { rms_mm: number; points: number } | null;
}

function parseCamera(raw: unknown): CameraInfo {
  const o = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const f = o.frame as { width?: unknown; height?: unknown; unix_s?: unknown } | null;
  const c = (o.calibration && typeof o.calibration === "object" ? o.calibration : {}) as Record<string, unknown>;
  const lens = c.lens as { k1?: unknown; k2?: unknown; rms_px?: unknown } | null | undefined;
  const al = c.alignment as { rms_mm?: unknown; points?: unknown } | null | undefined;
  const n = (v: unknown) => (typeof v === "number" ? v : 0);
  return {
    capture: o.capture_support === true,
    frame: f && typeof f.width === "number" ? { width: n(f.width), height: n(f.height), unix_s: n(f.unix_s) } : null,
    lens: lens ? { k1: n(lens.k1), k2: n(lens.k2), rms_px: n(lens.rms_px) } : null,
    alignment: al ? { rms_mm: n(al.rms_mm), points: n(al.points) } : null,
  };
}

/** Whether the engine has the camera routes (GET /api/camera answers). Cached per page load. */
let support: Promise<boolean> | null = null;
export function cameraSupported(): Promise<boolean> {
  support ??= libsApi
    .camera()
    .then(() => true)
    .catch(() => false);
  return support;
}

export function useCameraSupported(): boolean {
  const [ok, setOk] = useState(false);
  useEffect(() => {
    let live = true;
    void cameraSupported().then((v) => live && setOk(v));
    return () => {
      live = false;
    };
  }, []);
  return ok;
}

type Tab = "capture" | "lens" | "align" | "overlay";

export function CameraWindow(p: { doc: GDocument | null; machine: MachineStatus | null; activeLayer: string | null; run: Run; notify: Notify; report: (e: unknown) => void; onClose: () => void }) {
  const t = useT();
  const [info, setInfo] = useState<CameraInfo | null>(null);
  const [frameUrl, setFrameUrl] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("capture");
  const [busy, setBusy] = useState(false);
  const [index, setIndex] = useState(0);
  const [board, setBoard] = useState<[number, number]>([9, 6]);
  const [views, setViews] = useState<{ cols: number; rows: number; corners: [number, number][] }[]>([]);
  const [corners, setCorners] = useState<[number, number][] | null>(null);
  const [points, setPoints] = useState<{ img: [number, number]; field: [number, number] }[]>([]);
  const ov = cameraOverlay.use();
  const field = typeof p.doc?.field_mm === "number" && p.doc.field_mm > 0 ? p.doc.field_mm : typeof p.machine?.field_mm === "number" ? p.machine.field_mm : 110;
  const markAt = Number((field / 3).toFixed(1));

  const refresh = async () => {
    const i = parseCamera(await libsApi.camera());
    setInfo(i);
    if (i.frame) {
      const { blob } = await libsApi.cameraFramePng();
      setFrameUrl((old) => {
        if (old) URL.revokeObjectURL(old);
        return URL.createObjectURL(blob);
      });
    }
  };
  useEffect(() => {
    refresh().catch((e) => p.report(new Error(errText(e))));
    return () => setFrameUrl((old) => (old && URL.revokeObjectURL(old), null));
  }, []);

  const act = async (fn: () => Promise<unknown>, ok?: string) => {
    setBusy(true);
    try {
      const r = await fn();
      if (ok) p.notify(ok);
      return r;
    } catch (e) {
      p.report(new Error(errText(e)));
      return null;
    } finally {
      setBusy(false);
    }
  };
  const afterFrame = async () => {
    setCorners(null);
    await refresh();
  };

  const updateOverlay = async (shown = true) => {
    const ppm = Math.max(1, Math.min(8, Math.floor(4096 / field)));
    const { blob } = await libsApi.cameraOverlayPng(ppm);
    const url = URL.createObjectURL(blob);
    const old = cameraOverlay.get().url;
    if (old) URL.revokeObjectURL(old);
    cameraOverlay.set({ ...cameraOverlay.get(), url, fieldMm: field, shown });
  };

  const tabs: { v: Tab; label: string }[] = [
    { v: "capture", label: t("cam.tabCapture") },
    { v: "lens", label: t("cam.tabLens") },
    { v: "align", label: t("cam.tabAlign") },
    { v: "overlay", label: t("cam.tabOverlay") },
  ];

  return (
    <WsModal
      title={t("cam.title")}
      wide
      onClose={p.onClose}
      footer={
        <button className="btn btn-primary" onClick={p.onClose}>
          {t("common.close")}
        </button>
      }
    >
      <div className="segmented dev-tabs" role="tablist">
        {tabs.map((x) => (
          <button key={x.v} role="tab" aria-selected={tab === x.v} className={tab === x.v ? "active" : ""} onClick={() => setTab(x.v)}>
            {x.label}
          </button>
        ))}
      </div>
      <dl className="kv lx-cam-status">
        <dt>{t("cam.frame")}</dt>
        <dd>{info?.frame ? `${info.frame.width} × ${info.frame.height} px` : t("cam.noFrame")}</dd>
        <dt>{t("cam.lens")}</dt>
        <dd>{info?.lens ? t("cam.lensDone", { rms: info.lens.rms_px.toFixed(2) }) : t("cam.notCalibrated")}</dd>
        <dt>{t("cam.alignment")}</dt>
        <dd>{info?.alignment ? t("cam.alignDone", { rms: info.alignment.rms_mm.toFixed(3), n: info.alignment.points }) : t("cam.notAligned")}</dd>
      </dl>

      {tab === "capture" && (
        <div className="lx-cam-src">
          {info?.capture ? (
            <div className="lx-row">
              <NumberField label={t("cam.index")} value={index} min={0} max={9} digits={0} onCommit={(v) => v !== null && setIndex(Math.round(v))} />
              <button className="btn btn-primary" disabled={busy} onClick={() => void act(async () => (await libsApi.cameraCapture(index), afterFrame()), t("cam.captured"))}>
                {t("cam.capture")}
              </button>
            </div>
          ) : (
            <p className="hint">{t("cam.noUvc")}</p>
          )}
          <BrowserCamera disabled={busy} onFrame={(b) => void act(async () => (await libsApi.cameraFrame(b), afterFrame()), t("cam.captured"))} />
          <FilePick label={t("cam.upload")} accept="image/png,image/jpeg" disabled={busy} onFile={(f) => void act(async () => (await libsApi.cameraFrame(f), afterFrame()), t("cam.captured"))} />
        </div>
      )}

      {tab === "lens" && (
        <div>
          <p className="hint">{t("cam.lensIntro")}</p>
          <div className="lx-row">
            <NumberField label={t("cam.cols")} value={board[0]} min={3} max={100} digits={0} onCommit={(v) => v !== null && setBoard([Math.round(v), board[1]])} />
            <NumberField label={t("cam.rows")} value={board[1]} min={3} max={100} digits={0} onCommit={(v) => v !== null && setBoard([board[0], Math.round(v)])} />
          </div>
          <div className="lx-row">
            <button
              className="btn"
              disabled={busy || !info?.frame}
              onClick={() =>
                void act(async () => {
                  const r = (await libsApi.cameraDetect(board[0], board[1])) as { found?: boolean; corners?: [number, number][] | null };
                  setCorners(r?.found && r.corners ? r.corners : null);
                  if (!r?.found) p.notify(t("cam.notFound"), "error");
                })
              }
            >
              {t("cam.detect")}
            </button>
            <button className="btn" disabled={!corners} onClick={() => corners && (setViews((v) => [...v, { cols: board[0], rows: board[1], corners }]), setCorners(null))}>
              {t("cam.addView", { n: views.length + 1 })}
            </button>
            <button
              className="btn btn-primary"
              disabled={busy || views.length < 3 || !info?.frame}
              onClick={() => info?.frame && void act(async () => (await libsApi.cameraLens(info.frame!.width, info.frame!.height, views), setViews([]), refresh()), t("cam.lensSaved"))}
            >
              {t("cam.calibrate")}
            </button>
          </div>
          <p className="hint">{t("cam.views", { n: views.length })}</p>
          <FrameView url={frameUrl} frame={info?.frame ?? null} marks={corners ?? []} />
        </div>
      )}

      {tab === "align" && (
        <div>
          <p className="hint">{t("cam.alignIntro", { at: markAt })}</p>
          <div className="lx-row">
            <button
              className="btn"
              disabled={busy || !p.activeLayer}
              onClick={() => {
                const s = 3;
                const marks = [
                  [-markAt, markAt],
                  [markAt, markAt],
                  [markAt, -markAt],
                  [-markAt, -markAt],
                ];
                const invs: Invocation[] = marks.map(([x, y], i) => ({
                  id: "shape.path",
                  params: { svg_path_data: `M${x - s},${y}H${x + s}M${x},${y - s}V${y + s}`, layer: p.activeLayer, name: `Camera mark ${i + 1}` },
                }));
                void act(() => p.run(invs), t("cam.marksAdded"));
              }}
            >
              {t("cam.addMarks")}
            </button>
            <button className="btn" disabled={points.length === 0} onClick={() => setPoints([])}>
              {t("cam.clearPoints")}
            </button>
            <button
              className="btn btn-primary"
              disabled={busy || points.length < 4}
              onClick={() =>
                void act(
                  async () => (
                    await libsApi.cameraAlign(
                      points.map((x) => x.img),
                      points.map((x) => x.field),
                    ),
                    refresh()
                  ),
                  t("cam.aligned"),
                )
              }
            >
              {t("cam.align")}
            </button>
          </div>
          <FrameView
            url={frameUrl}
            frame={info?.frame ?? null}
            marks={points.map((x) => x.img)}
            onPick={(img) => {
              const defaults: [number, number][] = [
                [-markAt, markAt],
                [markAt, markAt],
                [markAt, -markAt],
                [-markAt, -markAt],
              ];
              setPoints((ps) => [...ps, { img, field: defaults[ps.length] ?? [0, 0] }]);
            }}
          />
          {points.length > 0 && (
            <table className="lx-table">
              <thead>
                <tr>
                  <th>#</th>
                  <th>{t("cam.imagePx")}</th>
                  <th>{t("cam.fieldX")}</th>
                  <th>{t("cam.fieldY")}</th>
                </tr>
              </thead>
              <tbody>
                {points.map((pt, i) => (
                  <tr key={i}>
                    <td>{i + 1}</td>
                    <td>{`${pt.img[0].toFixed(1)}, ${pt.img[1].toFixed(1)}`}</td>
                    {[0, 1].map((k) => (
                      <td key={k}>
                        <input
                          type="number"
                          step="0.1"
                          className="lx-num"
                          value={pt.field[k]}
                          onChange={(e) => {
                            const v = Number(e.target.value);
                            setPoints((ps) => ps.map((x, j) => (j === i ? { ...x, field: (k === 0 ? [v, x.field[1]] : [x.field[0], v]) as [number, number] } : x)));
                          }}
                        />
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <button className="btn small btn-warn" disabled={busy || (!info?.lens && !info?.alignment)} onClick={() => void act(async () => (await libsApi.cameraForget(), refresh()), t("cam.forgotten"))}>
            {t("cam.forget")}
          </button>
        </div>
      )}

      {tab === "overlay" && (
        <div>
          <p className="hint">{t("cam.overlayIntro")}</p>
          <CheckField
            label={t("cam.showOverlay")}
            checked={ov.shown}
            onChange={(v) => {
              if (v && !ov.url) void act(() => updateOverlay(true));
              else cameraOverlay.set({ ...ov, shown: v });
            }}
          />
          <label className="field">
            <span className="field-label">{t("cam.opacity")}</span>
            <span className="field-input">
              <input type="range" min={0.1} max={1} step={0.05} value={ov.opacity} onChange={(e) => cameraOverlay.set({ ...cameraOverlay.get(), opacity: Number(e.target.value) })} />
            </span>
          </label>
          <button className="btn" disabled={busy || !info?.alignment || !info?.frame} onClick={() => void act(() => updateOverlay(true), t("cam.overlayUpdated"))}>
            {t("cam.updateOverlay")}
          </button>
          {!info?.alignment && <p className="run-hint">{t("cam.alignFirst")}</p>}
        </div>
      )}
    </WsModal>
  );
}

/** The frame with picked or detected points; a click reports the image pixel. */
function FrameView(p: { url: string | null; frame: { width: number; height: number } | null; marks: [number, number][]; onPick?: (px: [number, number]) => void }) {
  if (!p.url || !p.frame) return null;
  const { width: w, height: h } = p.frame;
  return (
    <div className={`lx-frame${p.onPick ? " pick" : ""}`}>
      <svg
        viewBox={`0 0 ${w} ${h}`}
        onClick={(e) => {
          if (!p.onPick) return;
          const r = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
          p.onPick([((e.clientX - r.left) / r.width) * w, ((e.clientY - r.top) / r.height) * h]);
        }}
      >
        <image href={p.url} x={0} y={0} width={w} height={h} />
        {p.marks.map(([x, y], i) => (
          <g key={i} className="lx-frame-mark">
            <circle cx={x} cy={y} r={Math.max(3, w / 200)} />
            {p.onPick && (
              <text x={x + w / 120} y={y - w / 120} fontSize={Math.max(10, w / 50)}>
                {i + 1}
              </text>
            )}
          </g>
        ))}
      </svg>
    </div>
  );
}

/** A camera of this browser (getUserMedia): live view and Snapshot, sent to the engine as a PNG. */
function BrowserCamera(p: { disabled: boolean; onFrame: (png: Blob) => void }) {
  const t = useT();
  const video = useRef<HTMLVideoElement>(null);
  const [stream, setStream] = useState<MediaStream | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => () => stream?.getTracks().forEach((tr) => tr.stop()), [stream]);
  const start = async () => {
    setError(null);
    try {
      const s = await navigator.mediaDevices.getUserMedia({ video: { width: { ideal: 1920 }, height: { ideal: 1080 } } });
      setStream(s);
      if (video.current) video.current.srcObject = s;
    } catch (e) {
      setError(errText(e));
    }
  };
  const snap = () => {
    const v = video.current;
    if (!v || !v.videoWidth) return;
    const c = document.createElement("canvas");
    c.width = v.videoWidth;
    c.height = v.videoHeight;
    c.getContext("2d")?.drawImage(v, 0, 0);
    c.toBlob((b) => b && p.onFrame(b), "image/png");
  };
  const can = typeof navigator !== "undefined" && !!navigator.mediaDevices?.getUserMedia;
  return (
    <div className="lx-browser-cam">
      {!stream ? (
        <button className="btn" disabled={!can || p.disabled} onClick={() => void start()} title={can ? undefined : t("cam.noBrowserCam")}>
          {t("cam.browserCam")}
        </button>
      ) : (
        <div className="lx-row">
          <button className="btn btn-primary" disabled={p.disabled} onClick={snap}>
            {t("cam.snapshot")}
          </button>
          <button className="btn" onClick={() => setStream(null)}>
            {t("cam.stopCam")}
          </button>
        </div>
      )}
      <video ref={video} autoPlay playsInline muted className={stream ? "lx-video" : "lx-video hidden"} />
      {error && <p className="form-error">{error}</p>}
    </div>
  );
}
