// Jigs and placement for App.tsx in one hook (src/jigs/, short list #2, #8, #9, #15):
// - Fit to stock: Arrange menu, context menu, command search, the phone's selection sheet;
// - the Jigs dropdown (ribbon ARRANGE group, so also its overflow, the tablet rail and the phone strip): new, import
//   (DXF / SVG), from the selection, place the selection in every pocket, switch pockets, the Jigs window;
// - the canvas: every jig's pockets (numbered; off pockets dashed), plate, datum and keep-out zones (hatched, also the
//   device's), the pocket mode (tap a pocket to switch it on or off, or to frame it) and the Re-mark region tool
//   (drag a rectangle or draw a lasso; Start then runs only the planned toolpath inside it, through the one Start
//   flow and the engine's approval).
//
// Nothing here fires the laser: commands edit the document, framing is light only, and Start is laser/startFlow.ts.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { execute, type Invocation } from "../api/client.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import { jobApi } from "../api/jobApi.ts";
import type { CanvasTool, RenderCtx } from "../canvasTool.ts";
import { Dropdown } from "../components/EditMenus.tsx";
import type { MenuItem } from "../components/Menu.tsx";
import { RibbonSlot, usePublish } from "../components/Ribbon.tsx";
import { geometryCmds, toPathData } from "../geometry.ts";
import { HudItem } from "../hud/CanvasHud.tsx";
import { useLaserLive } from "../laser/useLaserLive.ts";
import { errText } from "../libs/ui.tsx";
import { useT, useTD } from "../workspace/usePrefs.ts";
import { forLayout } from "../workspace/desktopOnly.ts";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { PHONE_CANVAS_EVENT } from "../workspace/PhoneShell.tsx";
import { FitDialog, JigsDialog, NewJigDialog, fitToStock } from "./JigDialogs.tsx";
import {
  J,
  jigTransform,
  jigsOf,
  objectsInPocket,
  pocketAt,
  pocketBox,
  pocketCenter,
  pocketCmds,
  pocketOn,
  rectRegion,
  regionUsable,
  simplifyLasso,
  type Jig,
} from "./jigModel.ts";
import { useDeviceZones } from "./Preflight.tsx";
import { setRemarkRegion, useRemarkRegion } from "./regionStore.ts";
import "./jigs.css";
import { HelpLink } from "../guide/HelpLink.tsx";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
type Notify = (msg: string, kind?: "info" | "error") => void;
type KeyHandler = (chord: string) => unknown;

export interface JigsUiProps {
  doc: GDocument | null;
  selection: readonly string[];
  /** The Design workspace is shown. */
  design: boolean;
  run: Run;
  notify: Notify;
  report: (e: unknown) => void;
  selectIds: (ids: string[]) => void;
  /** Another canvas tool is about to take the canvas (the region tool and the pocket mode close then). */
  onCanvasTool?: () => void;
}

type Win = { kind: "fit" } | { kind: "new" } | { kind: "jigs" } | null;
type RegionMode = "rect" | "lasso";

interface RemarkInfo {
  objects: string[];
  warnings: { text: string; params?: { layer?: string; preset?: string } }[];
}

export function useJigsUi(p: JigsUiProps) {
  const t = useT();
  const td = useTD();
  const layout = useLayoutMode().layout;
  const { facts } = useLaserLive();
  const P = useRef(p);
  P.current = p;
  const [win, setWin] = useState<Win>(null);
  const [jigSel, setJigSel] = useState<string | null>(null);
  const [pocketMode, setPocketMode] = useState<{ jig: string; frame: boolean } | null>(null);
  const [regionMode, setRegionMode] = useState<RegionMode | null>(null);
  const region = useRemarkRegion();
  const [draft, setDraft] = useState<Vec2[] | null>(null);
  const [info, setInfo] = useState<RemarkInfo | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const zones = useDeviceZones();
  const jigs = useMemo(() => jigsOf(p.doc), [p.doc]);
  const ids = p.selection;
  const hasStock = !!p.doc?.stock;
  const designOnly = !!facts.designOnly;

  const exec = (inv: Invocation | Invocation[], done?: string) =>
    P.current
      .run(inv)
      .then((r) => {
        if (done) P.current.notify(done);
        return r;
      })
      .catch((e) => {
        P.current.notify(errText(e), "error");
        return null;
      });

  // The pocket mode ends when its jig goes; Esc ends it and the region tool.
  useEffect(() => {
    if (pocketMode && !jigs.some((j) => j.id === pocketMode.jig)) setPocketMode(null);
  }, [jigs, pocketMode]);
  useEffect(() => {
    if (!pocketMode && !regionMode && !region) return;
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      setPocketMode(null);
      setRegionMode(null);
      setDraft(null);
      setRemarkRegion(null);
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [pocketMode, regionMode, region]);

  // What the region reaches (job.remark_check), again when the document or the region changes.
  const rev = p.doc?.revision;
  useEffect(() => {
    if (!regionUsable(region)) {
      setInfo(null);
      return;
    }
    let live = true;
    void execute(J.remarkCheck(region, null))
      .then((r) => live && setInfo(r.result as RemarkInfo))
      .catch(() => live && setInfo(null));
    return () => {
      live = false;
    };
  }, [region, rev]);

  // On the phone the canvas tools want the whole canvas: the open sheet closes (workspace/PhoneShell.tsx).
  const clearPhoneSheet = () => {
    if (layout === "phone") window.dispatchEvent(new CustomEvent(PHONE_CANVAS_EVENT));
  };
  const startRegion = (mode: RegionMode) => {
    clearPhoneSheet();
    P.current.onCanvasTool?.();
    setPocketMode(null);
    setRegionMode(mode);
    setDraft(null);
  };
  const stopRegion = () => {
    setRegionMode(null);
    setDraft(null);
    setRemarkRegion(null);
  };
  const startPocketMode = (jig: string) => {
    clearPhoneSheet();
    P.current.onCanvasTool?.();
    stopRegion();
    setPocketMode({ jig, frame: false });
  };

  const fit = () => {
    if (!hasStock) return P.current.notify(t("fit.noStock"), "error");
    void fitToStock(P.current.run, P.current.notify, t, ids).catch((e) => P.current.notify(errText(e), "error"));
  };
  const placeIn = (jig: Jig) => {
    if (!ids.length) return P.current.notify(t("jig.selectFirst"), "error");
    void exec(J.place(jig.id, ids), t("jig.placed", { n: jig.pockets.filter(pocketOn).length }));
  };
  const framePocket = (jig: Jig, number: number, pocketId: string) => {
    const inside = objectsInPocket(P.current.doc, jig.id, pocketId);
    const body = inside.length ? { ids: inside, mode: "outline" } : { jig_pockets: { jig: jig.id, pockets: [number] }, mode: "outline" };
    void jobApi
      .frame({ continuous: true, ...body })
      .then(() => P.current.notify(t("jig.framePocket", { n: number })))
      .catch((e) => P.current.notify(errText(e), "error"));
  };
  const togglePocket = (jig: Jig, number: number, on: boolean) => void exec(J.setPockets(jig.id, [number], !on));

  // ---- import a jig drawing ----
  const onFile = (e: React.ChangeEvent<HTMLInputElement>) => {
    const f = e.target.files?.[0];
    e.target.value = "";
    if (!f) return;
    const format = /\.dxf$/i.test(f.name) ? "dxf" : "svg";
    void f
      .text()
      .then((content) => P.current.run(J.importFile(content, format, f.name.replace(/\.[^.]+$/, ""), P.current.doc?.stock?.center ?? null)))
      .then((r) => {
        const j = (r as { jig?: { id?: string; name?: string; pockets?: unknown[] } }).jig;
        if (j?.id) setJigSel(j.id);
        P.current.notify(t("jig.imported", { name: j?.name ?? "", n: j?.pockets?.length ?? 0 }));
      })
      .catch((err) => P.current.notify(t("jig.importFailed", { error: errText(err) }), "error"));
  };
  const openImport = () => fileRef.current?.click();

  // ---- menus ----
  const fitItems: MenuItem[] = [
    { label: t("fit.menu"), icon: "fitStock", actionId: "arrange.fitStock", disabled: !ids.length || !hasStock, title: hasStock ? undefined : t("fit.noStock"), action: fit },
    { label: t("fit.menuOptions"), disabled: !ids.length || !hasStock, action: () => setWin({ kind: "fit" }) },
  ];
  const placeItems: MenuItem[] =
    jigs.length <= 1
      ? [{ label: t("jig.place"), disabled: !ids.length || !jigs.length, action: () => jigs[0] && placeIn(jigs[0]) }]
      : [{ label: t("jig.place"), disabled: !ids.length, submenu: jigs.map((j) => ({ label: t("jig.placeIn", { name: j.name }), action: () => placeIn(j) })) }];
  const jigItems: MenuItem[] = [
    { label: t("jig.new"), action: () => setWin({ kind: "new" }) },
    { label: t("jig.import"), action: openImport },
    { label: t("jig.fromSelection"), disabled: !ids.length, action: () => void exec(J.fromObjects(ids)).then((r) => r && setJigSel((r as { jig?: { id?: string } }).jig?.id ?? null)) },
    { sep: true },
    ...placeItems,
    {
      label: pocketMode ? t("jig.done") : t("jig.pockets"),
      disabled: !jigs.length,
      action: () => (pocketMode ? setPocketMode(null) : jigs[0] && startPocketMode(jigSel && jigs.some((j) => j.id === jigSel) ? jigSel : jigs[0].id)),
    },
    { sep: true },
    { label: regionMode || region ? t("rm.stop") : t("rm.menu"), icon: "lasso", actionId: "laser.remarkRegion", action: () => (regionMode || region ? stopRegion() : startRegion("rect")) },
    { sep: true },
    { label: t("jig.window"), icon: "jig", actionId: "window.jigs", action: () => setWin({ kind: "jigs" }) },
  ];
  const arrangeItems: MenuItem[] = [{ sep: true }, ...fitItems];
  // The canvas context menu: Fit to stock and Place in every pocket under Arrange ▸, Re-mark region under Laser ▸.
  const menuItems: MenuItem[] = [
    { sep: true },
    { ...fitItems[0], section: "arrange" } as MenuItem,
    ...(jigs.length ? placeItems.map((it) => ({ ...it, section: "arrange" }) as MenuItem) : []),
    { label: regionMode || region ? t("rm.stop") : t("rm.menu"), section: "laser", action: () => (regionMode || region ? stopRegion() : startRegion("rect")) },
  ];
  const shown = forLayout(jigItems, layout);
  usePublish("menu.jigs", { label: t("jig.menu"), items: p.design ? shown : [] });
  const toolbar: ReactNode = p.design ? (
    <RibbonSlot group="arrange" order={60}>
      <Dropdown icon="jig" label={t("jig.menu")} title={t("jig.menuTitle")} items={shown} />
    </RibbonSlot>
  ) : null;

  const handlers: Record<string, KeyHandler> = {
    "arrange.fitStock": () => (p.design && ids.length && hasStock ? fit() : false),
    "window.jigs": () => setWin({ kind: "jigs" }),
    "laser.remarkRegion": () => (regionMode || region ? stopRegion() : startRegion("rect")),
  };

  // ---- the canvas ----
  const drag = useRef<{ from: Vec2; pts: Vec2[] } | null>(null);
  const live = useRef<Vec2[] | null>(null);
  const wrap = (inner: CanvasTool): CanvasTool => {
    const showing = jigs.length > 0 || zones.length > 0 || !!regionMode || !!region || !!draft;
    if (!showing) return inner;
    return {
      ...inner,
      down: (f, m, info, ctx) => {
        if (regionMode) {
          drag.current = { from: f, pts: [f] };
          live.current = null;
          return true;
        }
        if (pocketMode) {
          const hit = pocketAt(jigs.filter((j) => j.id === pocketMode.jig), f);
          if (hit) {
            if (pocketMode.frame) framePocket(hit.jig, hit.pocket.number, hit.pocket.id);
            else togglePocket(hit.jig, hit.pocket.number, pocketOn(hit.pocket));
            return true;
          }
        }
        return inner.down(f, m, info, ctx);
      },
      drag: (f, m, ctx) => {
        const d = drag.current;
        if (!regionMode || !d) return inner.drag(f, m, ctx);
        if (regionMode === "rect") live.current = rectRegion(d.from, f);
        else {
          d.pts.push(f);
          live.current = d.pts;
        }
        setDraft(live.current ? [...live.current] : null);
        ctx.readout(regionMode === "rect" ? t("rm.barDraw") : t("rm.barLasso"));
      },
      up: (f, m, ctx) => {
        const d = drag.current;
        if (!regionMode || !d) return inner.up(f, m, ctx);
        drag.current = null;
        const r = regionMode === "rect" ? rectRegion(d.from, f) : simplifyLasso([...d.pts, f], 2 / Math.max(ctx.scale, 1e-6));
        setDraft(null);
        if (regionUsable(r)) {
          setRemarkRegion(r);
          setRegionMode(null);
        }
      },
      cancel: () => {
        if (!regionMode) return inner.cancel();
        drag.current = null;
        setDraft(null);
      },
      hover: (f, m, ctx) => {
        if (regionMode) {
          ctx.readout(regionMode === "rect" ? t("rm.barDraw") : t("rm.barLasso"));
          return true;
        }
        if (pocketMode && pocketAt(jigs.filter((j) => j.id === pocketMode.jig), f)) {
          ctx.readout(pocketMode.frame ? t("jig.barFrame") : t("jig.bar"));
          return true;
        }
        return inner.hover(f, m, ctx);
      },
      render: (r) => (
        <>
          {inner.render(r)}
          <JigLayer jigs={jigs} zones={zones} r={r} active={pocketMode?.jig ?? null} />
          {(draft ?? region) && <polygon className={`rm-region${draft ? " draft" : ""}`} points={(draft ?? region ?? []).map((q) => `${q[0]},${q[1]}`).join(" ")} strokeWidth={r.px(1.5)} strokeDasharray={`${r.px(6)} ${r.px(4)}`} />}
        </>
      ),
      overlay: (r) => (
        <>
          {inner.overlay(r)}
          <PocketLabels jigs={jigs} r={r} mode={pocketMode} onPocket={(j, n, id, on) => (pocketMode?.frame ? framePocket(j, n, id) : togglePocket(j, n, on))} />
          {pocketMode && (
            <HudItem id="jig-pockets-bar" slot="tc" rank={24} minW={240}>
              <div className="vp-overlay lx-sp-bar jig-bar" role="status">
                <span>{pocketMode.frame ? t("jig.barFrame") : t("jig.bar")}</span> <HelpLink to="jigs#switch-pockets-on-or-off" topic={t("jig.pockets")} />
                <span className="segmented jig-seg-small">
                  <button type="button" className={pocketMode.frame ? "" : "active"} aria-pressed={!pocketMode.frame} onClick={() => setPocketMode({ ...pocketMode, frame: false })}>
                    {t("jig.modeToggle")}
                  </button>
                  <button type="button" disabled={designOnly} className={pocketMode.frame ? "active" : ""} aria-pressed={pocketMode.frame} onClick={() => setPocketMode({ ...pocketMode, frame: true })}>
                    {t("jig.modeFrame")}
                  </button>
                </span>
                <button className="btn small" onClick={() => void exec(J.setPockets(pocketMode.jig, "all", true))}>
                  {t("jig.allOn")}
                </button>
                <button className="btn small btn-primary" onClick={() => setPocketMode(null)}>
                  {t("jig.done")}
                </button>
              </div>
            </HudItem>
          )}
          {(regionMode || region) && (
            <HudItem id="remark-bar" slot="tc" rank={23} minW={260}>
              <div className="vp-overlay lx-sp-bar jig-bar rm-bar" role="status">
                <HelpLink to="remark-region#check-what-the-region-touches" topic={t("rm.menu")} />
                {regionMode ? (
                  <>
                    <span>{regionMode === "rect" ? t("rm.barDraw") : t("rm.barLasso")}</span>
                    <span className="segmented jig-seg-small">
                      <button type="button" className={regionMode === "rect" ? "active" : ""} aria-pressed={regionMode === "rect"} onClick={() => setRegionMode("rect")}>
                        {t("rm.rect")}
                      </button>
                      <button type="button" className={regionMode === "lasso" ? "active" : ""} aria-pressed={regionMode === "lasso"} onClick={() => setRegionMode("lasso")}>
                        {t("rm.lasso")}
                      </button>
                    </span>
                    <button className="btn small" onClick={stopRegion}>
                      {t("rm.cancel")}
                    </button>
                  </>
                ) : (
                  <>
                    <span>{info && info.objects.length === 0 ? t("rm.nothing") : t("rm.summary", { n: info?.objects.length ?? 0 })}</span>
                    {info?.warnings.map((w, i) => (
                      <span key={i} className="rm-warn" role="alert">
                        {w.params?.layer ? td("rm.colour", w.text, { layer: w.params.layer, preset: w.params.preset ?? "" }) : w.text}
                      </span>
                    ))}
                    <button
                      className="btn small"
                      disabled={designOnly}
                      onClick={() =>
                        region &&
                        void jobApi
                          .frame({ region, continuous: true })
                          .then(() => P.current.notify(t("rm.frame")))
                          .catch((e) => P.current.notify(errText(e), "error"))
                      }
                    >
                      {t("rm.frame")}
                    </button>
                    <button className="btn small" onClick={() => startRegion("rect")}>
                      {t("rm.rect")}
                    </button>
                    <button className="btn small" onClick={() => startRegion("lasso")}>
                      {t("rm.lasso")}
                    </button>
                    <button className="btn small" onClick={stopRegion}>
                      {t("rm.clear")}
                    </button>
                  </>
                )}
              </div>
            </HudItem>
          )}
        </>
      ),
    };
  };

  // ---- windows ----
  const close = () => setWin(null);
  let dialogs: ReactNode = null;
  if (win?.kind === "fit") dialogs = <FitDialog ids={[...ids]} run={p.run} notify={p.notify} onClose={close} />;
  else if (win?.kind === "new") dialogs = <NewJigDialog doc={p.doc} run={p.run} notify={p.notify} onClose={close} onCreated={(id) => setJigSel(id)} />;
  else if (win?.kind === "jigs" && p.doc)
    dialogs = (
      <JigsDialog
        doc={p.doc}
        selection={ids}
        jig={jigSel}
        onJig={setJigSel}
        designOnly={designOnly}
        run={p.run}
        notify={p.notify}
        onClose={close}
        onNew={() => setWin({ kind: "new" })}
        onImport={openImport}
        onPocketMode={startPocketMode}
        selectIds={p.selectIds}
      />
    );

  // The jig drawing picker stays mounted with the windows (the toolbar is not shown in every workspace).
  dialogs = (
    <>
      <input ref={fileRef} type="file" accept=".dxf,.svg,image/svg+xml" hidden onChange={onFile} />
      {dialogs}
    </>
  );

  // The phone's selection sheet: Fit to stock beside Centre on stock.
  const phoneButtons: ReactNode = (
    <button type="button" className="btn" disabled={!hasStock || !ids.length} onClick={fit}>
      {t("fit.menu")}
    </button>
  );

  return {
    toolbar,
    arrangeItems,
    menuItems,
    handlers,
    wrap,
    dialogs,
    phoneButtons,
    modalOpen: win !== null,
    /** Another canvas tool took the canvas. */
    stopTools: () => {
      setPocketMode(null);
      setRegionMode(null);
      setDraft(null);
    },
  };
}

// ---- drawing --------------------------------------------------------------------------------------------------------

/** Jigs (plate, pockets, datum, keep-out walls) and the device's keep-out zones, in the field frame. */
function JigLayer(p: { jigs: readonly Jig[]; zones: readonly { name: string; points: Vec2[]; enabled?: boolean }[]; r: RenderCtx; active: string | null }) {
  const { r } = p;
  const w = r.px(1);
  return (
    <g className="jig-layer" pointerEvents="none">
      <defs>
        <pattern id="jig-hatch" patternUnits="userSpaceOnUse" width={r.px(8)} height={r.px(8)} patternTransform="rotate(45)">
          <line x1="0" y1="0" x2="0" y2={r.px(8)} className="jig-hatch-line" strokeWidth={r.px(2)} />
        </pattern>
      </defs>
      {p.jigs.map((j) => {
        const t = jigTransform(j);
        return (
          <g key={j.id} className={`jig${p.active === j.id ? " active" : ""}`}>
            {j.outline && <path className="jig-plate" d={toPathData(geometryCmds({ kind: "path", path: j.outline }), t)} strokeWidth={w} />}
            {(j.keep_out ?? []).map((k) => (
              <path key={k.id} className={`jig-keepout${k.enabled === false ? " off" : ""}`} d={toPathData(geometryCmds({ kind: "path", path: k.path }), t)} strokeWidth={w} />
            ))}
            {j.pockets.map((pk) => {
              const c = pocketCmds(j, pk);
              const on = pocketOn(pk);
              const sw = p.active === j.id ? r.px(2) : w;
              return <path key={pk.id} className={`jig-pocket-outline${on ? "" : " off"}`} d={toPathData(c.cmds, c.transform)} strokeWidth={sw} strokeDasharray={on ? undefined : `${r.px(4)} ${r.px(3)}`} />;
            })}
            <g className="jig-datum">
              <line x1={j.datum[0] - r.px(6)} y1={j.datum[1]} x2={j.datum[0] + r.px(6)} y2={j.datum[1]} strokeWidth={w} />
              <line x1={j.datum[0]} y1={j.datum[1] - r.px(6)} x2={j.datum[0]} y2={j.datum[1] + r.px(6)} strokeWidth={w} />
            </g>
          </g>
        );
      })}
      {p.zones.map((z, i) => (
        <polygon key={i} className={`jig-keepout device${z.enabled === false ? " off" : ""}`} points={z.points.map((q) => `${q[0]},${q[1]}`).join(" ")} strokeWidth={w} />
      ))}
    </g>
  );
}

/** The screen point 12 px under the middle of a pocket's top edge (the label's centre; it is 20 px tall). */
function topInside(j: Jig, pk: Jig["pockets"][number], r: RenderCtx): [number, number] {
  const b = pocketBox(j, pk);
  const cx = (b.x0 + b.x1) / 2;
  const a = r.toScreen([cx, b.y0]);
  const c = r.toScreen([cx, b.y1]);
  const top = a[1] < c[1] ? a : c;
  const bottom = a[1] < c[1] ? c : a;
  // A pocket smaller than the label on screen: its middle.
  return bottom[1] - top[1] < 32 ? [(top[0] + bottom[0]) / 2, (top[1] + bottom[1]) / 2] : [top[0], top[1] + 12];
}

/** Pocket numbers in screen space; in the pocket mode they are buttons (44 px on touch). */
function PocketLabels(p: {
  jigs: readonly Jig[];
  r: RenderCtx;
  mode: { jig: string; frame: boolean } | null;
  onPocket: (j: Jig, number: number, id: string, on: boolean) => void;
}) {
  const t = useT();
  if (!p.jigs.length) return null;
  const [w, h] = p.r.size;
  return (
    <div className="jig-labels" style={{ width: w, height: h }}>
      {p.jigs.flatMap((j) =>
        j.pockets.map((pk) => {
          const active = p.mode?.jig === j.id;
          // Choosing pockets: a button in the middle. Otherwise the number sits just inside the pocket's top edge, so
          // it does not cover the design in the middle of the pocket.
          const [x, y] = active ? p.r.toScreen(pocketCenter(j, pk)) : topInside(j, pk, p.r);
          if (x < -20 || y < -20 || x > w + 20 || y > h + 20) return null;
          const on = pocketOn(pk);
          const label = on ? t("jig.pocket", { n: pk.number }) : t("jig.pocketOff", { n: pk.number });
          return active ? (
            <button
              key={`${j.id}/${pk.id}`}
              type="button"
              className={`jig-label button${on ? "" : " off"}`}
              style={{ left: x, top: y }}
              aria-pressed={on}
              aria-label={label}
              title={label}
              onClick={() => p.onPocket(j, pk.number, pk.id, on)}
            >
              {pk.number}
            </button>
          ) : (
            <span key={`${j.id}/${pk.id}`} className={`jig-label${on ? "" : " off"}`} style={{ left: x, top: y }} aria-hidden="true">
              {pk.number}
            </span>
          );
        }),
      )}
    </div>
  );
}
