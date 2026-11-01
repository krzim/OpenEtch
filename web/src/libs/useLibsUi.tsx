// The libs UI for App.tsx in one hook: the Library dock, LightBurn's Laser Tools menu (Material / Interval / Focus
// Test, Set Start Point, Print and Cut, Camera), the File and Help menu entries (Material Library, Find My Laser,
// Batch, Job log; Report a Bug, Check for Updates), the hotkey handlers, the canvas layer (start point tool, start
// point marks, the camera image), and the job-finished chime. App renders `dock`, `dialogs`, `toolbar` and wraps its
// canvas tool with `wrap`.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { execute, type Invocation } from "../api/client.ts";
import { onEngineEvent } from "../api/events.ts";
import type { GDocument, MachineStatus, Vec2 } from "../api/types.ts";
import type { CanvasTool } from "../canvasTool.ts";
import type { MenuItem } from "../components/ContextMenu.tsx";
import { boxCenter, objectBounds, unionBoxes } from "../geometry.ts";
import { parseNodeList, type NodeList } from "../nodes.ts";
import { updatePrefs } from "../workspace/prefs.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { hintFor } from "../workspace/hotkeys.ts";
import { CameraWindow, useCameraSupported } from "./Camera.tsx";
import { FindLaserWizard } from "./FindLaser.tsx";
import { BugReportDialog, UpdatesDialog } from "./HelpExtras.tsx";
import { onJobFinishedSound } from "./JobSafety.tsx";
import { BatchDialog, JobLogDialog, PrintCutDialog, RepeatTableDialog } from "./LaserExtras.tsx";
import { LibraryDock } from "./MaterialLibrary.tsx";
import { clearStartPoint, nearestStartNode, setStartPoint, startMarks } from "./model.ts";
import { setLibsToast, cameraOverlay, closeLibsWindow, openLibsWindow, startPointTool, useDevicePrefs, useLibsWindow, editPrefs, type LibsWindow } from "./store.ts";
import { FocusTestDialog, IntervalTestDialog, MaterialTestDialog } from "./TestGenerators.tsx";
import { DropMenu, errText, type DropItem } from "./ui.tsx";
import "./libs.css";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
type Notify = (msg: string, kind?: "info" | "error") => void;

export interface LibsUiProps {
  doc: GDocument | null;
  machine: MachineStatus | null;
  selection: readonly string[];
  activeLayer: string | null;
  design: boolean;
  designOnly: string | null;
  run: Run;
  notify: Notify;
  report: (e: unknown) => void;
  selectIds: (ids: string[]) => void;
  refreshMachine: () => void;
  /** Close other tools before the start point tool takes the canvas. */
  onStartPointTool?: () => void;
}

function selectionCentre(doc: GDocument | null, ids: readonly string[]): Vec2 | null {
  if (!doc || ids.length === 0) return null;
  const set = new Set(ids);
  const b = unionBoxes(doc.objects.filter((o) => set.has(o.id)).map(objectBounds));
  return b ? boxCenter(b) : null;
}

export function useLibsUi(p: LibsUiProps) {
  const t = useT();
  const win = useLibsWindow();
  const cam = useCameraSupported();
  const sp = startPointTool.use();
  const ov = cameraOverlay.use();
  const { prefs } = useDevicePrefs();
  const hotkeys = usePrefs().hotkeys;
  const P = useRef(p);
  P.current = p;
  setLibsToast(p.notify);

  // The chime on `job_finished_sound` (the engine sends it only with Play Sound on).
  useEffect(() => onEngineEvent((ev) => void onJobFinishedSound(ev)), []);

  // Opening the Library window expands its dock and scrolls it into view.
  useEffect(() => {
    if (win?.kind !== "library") return;
    updatePrefs((cur) => ({ docks: { ...cur.docks, library: false } }));
    closeLibsWindow();
    window.setTimeout(() => document.querySelector('[data-dock="library"]')?.scrollIntoView({ block: "nearest", behavior: "smooth" }), 50);
  }, [win]);

  // Esc leaves the start point tool.
  useEffect(() => {
    if (!sp) return;
    const key = (e: KeyboardEvent) => e.key === "Escape" && startPointTool.set(false);
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [sp]);

  const toggleStartPoint = () => {
    const on = !startPointTool.get();
    if (on) P.current.onStartPointTool?.();
    startPointTool.set(on);
    if (on) P.current.notify(t("sp.started"));
  };

  // ---- the start point tool's node lists (node.list is a query: nothing is recorded) ----
  const [hover, setHover] = useState<{ id: string; nl: NodeList | null; node: number | null } | null>(null);
  const cache = useRef(new Map<string, NodeList | null>());
  const rev = p.doc?.revision ?? 0;
  useEffect(() => cache.current.clear(), [rev]);
  const nodesOf = (id: string): NodeList | null | undefined => {
    const c = cache.current;
    if (c.has(id)) return c.get(id);
    c.set(id, null);
    execute({ id: "node.list", params: { id } })
      .then((r) => {
        c.set(id, parseNodeList(r.result));
        setHover((h) => (h && h.id === id ? { ...h, nl: c.get(id) ?? null } : h));
      })
      .catch(() => c.set(id, null));
    return undefined;
  };
  useEffect(() => {
    if (!sp) setHover(null);
  }, [sp]);

  const marks = useMemo(() => startMarks(p.doc), [p.doc]);

  const wrap = (inner: CanvasTool): CanvasTool => {
    if (!sp && !(ov.shown && ov.url) && marks.length === 0) return inner;
    const tolMm = (scale: number) => 10 / Math.max(scale, 1e-6);
    return {
      down: (f, m, info, ctx) => {
        if (!sp) return inner.down(f, m, info, ctx);
        const id = ctx.hitObject(f) ?? hover?.id ?? null;
        if (!id) {
          P.current.notify(t("sp.clickShape"), "error");
          return true;
        }
        if (m.ctrl) {
          void P.current.run(clearStartPoint([id])).then(() => P.current.notify(t("sp.cleared"))).catch((e) => P.current.report(new Error(errText(e))));
          return true;
        }
        const nl = cache.current.get(id) ?? null;
        const node = nl ? nearestStartNode(nl, f, tolMm(ctx.scale)) : null;
        if (node === null) {
          if (!nl) nodesOf(id);
          P.current.notify(t("sp.clickNode"), "error");
          return true;
        }
        void P.current
          .run(setStartPoint(id, node, m.shift))
          .then(() => P.current.notify(m.shift ? t("sp.setReversed", { node }) : t("sp.set", { node })))
          .catch((e) => P.current.report(new Error(errText(e))));
        return true;
      },
      drag: (f, m, ctx) => {
        if (!sp) inner.drag(f, m, ctx);
      },
      up: (f, m, ctx) => {
        if (!sp) inner.up(f, m, ctx);
      },
      cancel: () => {
        if (!sp) inner.cancel();
      },
      hover: (f, m, ctx) => {
        if (!sp) return inner.hover(f, m, ctx);
        const id = ctx.hitObject(f);
        if (!id) {
          if (hover) setHover((h) => (h ? { ...h, node: null } : h));
          ctx.readout(t("sp.readout"));
          return true;
        }
        const nl = nodesOf(id) ?? null;
        const node = nl ? nearestStartNode(nl, f, tolMm(ctx.scale)) : null;
        if (!hover || hover.id !== id || hover.node !== node || hover.nl !== nl) setHover({ id, nl, node });
        ctx.readout(node === null ? t("sp.readoutShape") : t("sp.readoutNode", { node }));
        return true;
      },
      // The camera overlay goes under the design (LightBurn shows the camera image behind the shapes).
      underlay: (r) => (
        <>
          {inner.underlay?.(r)}
          {ov.shown && ov.url && ov.fieldMm > 0 && (
            <g transform="scale(1,-1)" pointerEvents="none" className="lx-cam-overlay">
              <image href={ov.url} x={-ov.fieldMm / 2} y={-ov.fieldMm / 2} width={ov.fieldMm} height={ov.fieldMm} opacity={ov.opacity} preserveAspectRatio="none" />
            </g>
          )}
        </>
      ),
      render: (r) => (
        <>
          {inner.render(r)}
          {marks.map((mk) => (
            <g key={mk.id} className="lx-start-mark" pointerEvents="none">
              <circle cx={mk.at[0]} cy={mk.at[1]} r={r.px(5)} strokeWidth={r.px(1.5)} />
              <circle cx={mk.at[0]} cy={mk.at[1]} r={r.px(1.5)} className="lx-start-dot" />
            </g>
          ))}
          {sp && hover?.nl && (
            <g className="lx-sp-nodes" pointerEvents="none">
              {hover.nl.nodes.map((n) => (
                <rect
                  key={n.index}
                  className={n.index === hover.node ? "hot" : undefined}
                  x={n.point[0] - r.px(n.index === hover.node ? 5 : 3)}
                  y={n.point[1] - r.px(n.index === hover.node ? 5 : 3)}
                  width={r.px(n.index === hover.node ? 10 : 6)}
                  height={r.px(n.index === hover.node ? 10 : 6)}
                  strokeWidth={r.px(1)}
                />
              ))}
            </g>
          )}
        </>
      ),
      overlay: (r) => (
        <>
          {inner.overlay(r)}
          {sp && (
            <div className="vp-overlay lx-sp-bar" role="status">
              <span>{t("sp.bar")}</span>
              <button className="btn small" onClick={() => startPointTool.set(false)}>
                {t("sp.done")}
              </button>
            </div>
          )}
        </>
      ),
      snapParts: inner.snapParts,
    };
  };

  // ---- menus ----
  const open = (k: LibsWindow) => () => openLibsWindow(k);
  const hk = (id: string) => hintFor(id, hotkeys) || undefined;
  const laserTools: DropItem[] = [
    { label: t("ltest.materialTitle"), hint: hk("laser.materialTest"), action: open("materialTest") },
    { label: t("ltest.intervalTitle"), hint: hk("laser.intervalTest"), action: open("intervalTest") },
    { label: t("ltest.focusTitle"), hint: hk("laser.focusTest"), action: open("focusTest") },
    { sep: true },
    { label: sp ? t("sp.stop") : t("sp.tool"), hint: hk("tool.startPoint"), action: toggleStartPoint },
    { label: t("pc.title"), hint: hk("laser.printCut"), action: open("printCut") },
    ...(cam ? [{ label: t("cam.title"), hint: hk("window.camera"), action: open("camera") } as DropItem] : []),
    { sep: true },
    { label: t("lib.title"), hint: hk("window.library"), action: open("library") },
  ];
  const toolbar: ReactNode = (
    <div className="tool-group">
      <DropMenu className="lx-tool-menu" label={<LaserToolsLabel label={t("lx.laserTools")} active={sp} />} title={t("lx.laserToolsTitle")} items={laserTools} />
    </div>
  );

  const menuItems: MenuItem[] = [
    { label: sp ? t("sp.stop") : t("sp.tool"), action: toggleStartPoint },
    {
      label: t("sp.clear"),
      disabled: !p.selection.some((id) => marks.some((m) => m.id === id)),
      action: () => void p.run(clearStartPoint(p.selection)).catch(p.report),
    },
  ];

  const fileItems = (close: () => void) => {
    const item = (label: string, k: LibsWindow, hint?: string) => (
      <button
        role="menuitem"
        className="menu-item"
        onClick={() => {
          close();
          openLibsWindow(k);
        }}
      >
        <span>{label}</span>
        {hint && <span className="menu-hint">{hint}</span>}
      </button>
    );
    return (
      <>
        {item(t("lib.title"), "library", hk("window.library"))}
        {item(t("fml.title"), "findLaser")}
        {item(`${t("batch.title")}…`, "batch")}
        {item(`${t("jlog.title")}…`, "jobLog", hk("laser.jobLog"))}
      </>
    );
  };

  const helpItems = (close: () => void) => (
    <>
      <button
        role="menuitem"
        className="menu-item"
        onClick={() => {
          close();
          void editPrefs({ debug_log: !prefs.debug_log })
            .then((x) => p.notify(x.debug_log ? t("dbg.on") : t("dbg.off")))
            .catch((e) => p.report(new Error(errText(e))));
        }}
      >
        <span>{`${prefs.debug_log ? "✓ " : ""}${t("dbg.debugLog")}`}</span>
      </button>
      <button
        role="menuitem"
        className="menu-item"
        onClick={() => {
          close();
          openLibsWindow("logs");
        }}
      >
        <span>{t("dbg.title")}</span>
      </button>
      <button
        role="menuitem"
        className="menu-item"
        onClick={() => {
          close();
          openLibsWindow("updates");
        }}
      >
        <span>{t("upd.title")}</span>
      </button>
    </>
  );

  const handlers: Record<string, () => boolean | void> = {
    "window.library": () => openLibsWindow("library"),
    "laser.materialTest": () => openLibsWindow("materialTest"),
    "laser.intervalTest": () => openLibsWindow("intervalTest"),
    "laser.focusTest": () => openLibsWindow("focusTest"),
    "laser.printCut": () => openLibsWindow("printCut"),
    "laser.jobLog": () => openLibsWindow("jobLog"),
    "window.camera": () => (cam ? openLibsWindow("camera") : false),
    "tool.startPoint": () => (p.design ? toggleStartPoint() : false),
  };

  // ---- dialogs ----
  const close = closeLibsWindow;
  const gen = {
    machine: p.machine,
    selectionCentre: selectionCentre(p.doc, p.selection),
    run: p.run,
    notify: p.notify,
    report: p.report,
    onCreated: p.selectIds,
    designOnly: p.designOnly,
    onClose: close,
  };
  let dialogs: ReactNode = null;
  switch (win?.kind) {
    case "materialTest":
      dialogs = <MaterialTestDialog {...gen} />;
      break;
    case "intervalTest":
      dialogs = <IntervalTestDialog {...gen} />;
      break;
    case "focusTest":
      dialogs = <FocusTestDialog {...gen} />;
      break;
    case "printCut":
      dialogs = <PrintCutDialog doc={p.doc} selection={p.selection} run={p.run} notify={p.notify} report={p.report} onClose={close} />;
      break;
    case "camera":
      dialogs = <CameraWindow doc={p.doc} machine={p.machine} activeLayer={p.activeLayer} run={p.run} notify={p.notify} report={p.report} onClose={close} />;
      break;
    case "findLaser":
      dialogs = <FindLaserWizard notify={p.notify} report={p.report} onClose={close} onAdded={p.refreshMachine} />;
      break;
    case "jobLog":
      dialogs = <JobLogDialog report={p.report} onClose={close} />;
      break;
    case "batch":
      dialogs = <BatchDialog notify={p.notify} report={p.report} onClose={close} onRequested={p.refreshMachine} />;
      break;
    case "repeatTable":
      dialogs = <RepeatTableDialog selection={p.selection} notify={p.notify} report={p.report} onClose={close} onRequested={p.refreshMachine} />;
      break;
    case "logs":
      dialogs = <BugReportDialog report={p.report} notify={p.notify} onClose={close} />;
      break;
    case "updates":
      dialogs = <UpdatesDialog report={p.report} onClose={close} />;
      break;
  }

  const dock = (
    <div data-dock="library">
      <LibraryDock doc={p.doc} activeLayer={p.activeLayer} notify={p.notify} report={p.report} />
    </div>
  );
  return { dock, dialogs, toolbar, laserTools, wrap, menuItems, fileItems, helpItems, handlers, startPointActive: sp, modalOpen: !!win && win.kind !== "library" };
}

function LaserToolsLabel({ label, active }: { label: string; active: boolean }) {
  return (
    <>
      <svg className="icon" width={18} height={18} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M4 4h6v6H4z M14 4h6v6h-6z M4 14h6v6H4z M17 14v6 M14 17h6" />
      </svg>
      <span>{label}</span>
      {active && <span className="lx-dot" aria-hidden="true" />}
    </>
  );
}
