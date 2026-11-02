// LightBurn's Tools menu extras for App.tsx in one hook (src/tools/): entries for the Design Tools dropdown (Add Tabs,
// remove / clear tabs, Dogbone, Resize Slots in Selection, Multi-File Trace), the Web Tools dropdown with the User
// Tools (LightBurn 2.2's Manage User Tools), the canvas layer (hand-placed tabs as red dots; the Add Tabs tool: click
// an outline to add, drag a dot to move it, Shift+click or double-click it to remove it) and the windows.
//
// Nothing here acts on the machine: tabs, dogbones, slots and traces are document commands, and a user tool either
// opens a web page in a new tab or runs document commands through the batch endpoint, which refuses machine actions.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Invocation } from "../api/client.ts";
import { onEngineEvent } from "../api/events.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import type { CanvasTool } from "../canvasTool.ts";
import type { MenuItem } from "../components/Menu.tsx";
import { Dropdown } from "../components/EditMenus.tsx";
import { RibbonSlot, usePublish } from "../components/Ribbon.tsx";
import { libsApi } from "../libs/api.ts";
import { ConfirmDialog, errText } from "../libs/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { forLayout } from "../workspace/desktopOnly.ts";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import {
  addTab,
  clearTabs,
  groupUserTools,
  moveTab,
  nearestTab,
  parseUserTools,
  removeTab,
  removeTabs,
  tabHitPx,
  tabMarks,
  userToolInvocations,
  userToolUrl,
  userToolsBody,
  type TabMark,
  type UserTool,
} from "./toolsApi.ts";
import { DogboneDialog, MultiTraceDialog, ResizeSlotsDialog, UserToolsDialog } from "./ToolsDialogs.tsx";
import "./tools.css";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
type Notify = (msg: string, kind?: "info" | "error") => void;

export interface ToolsUiProps {
  doc: GDocument | null;
  selection: readonly string[];
  activeLayer: string | null;
  /** The Design workspace is shown. */
  design: boolean;
  run: Run;
  notify: Notify;
  report: (e: unknown) => void;
  selectIds: (ids: string[]) => void;
  /** Close other canvas tools before Add Tabs takes the canvas. */
  onTabTool?: () => void;
}

type Win = "dogbone" | "slots" | "trace" | "userTools" | null;

/** The browser remembers that the third-party notice of web tools was accepted (LightBurn asks once). */
const NOTICE_KEY = "openetch.webToolsNotice";

function noticeAccepted(): boolean {
  try {
    return localStorage.getItem(NOTICE_KEY) === "1";
  } catch {
    return false;
  }
}

function acceptNotice() {
  try {
    localStorage.setItem(NOTICE_KEY, "1");
  } catch {
    // Private windows: ask again next time.
  }
}

export function useToolsUi(p: ToolsUiProps) {
  const t = useT();
  const layout = useLayoutMode().layout;
  const P = useRef(p);
  P.current = p;
  const [win, setWin] = useState<Win>(null);
  const [tabTool, setTabTool] = useState(false);
  const [userTools, setUserTools] = useState<UserTool[]>([]);
  const [supported, setSupported] = useState(true);
  // The web tool waiting for the one-time third-party notice (a Dialog, not window.confirm).
  const [notice, setNotice] = useState<UserTool | null>(null);

  // The User Tools from the engine's preferences, again whenever they change (another browser saved them).
  useEffect(() => {
    const load = () =>
      void libsApi
        .prefs()
        .then((r) => {
          setUserTools(parseUserTools(r));
          setSupported(true);
        })
        .catch(() => setSupported(false));
    load();
    return onEngineEvent((ev) => {
      if (ev.type === "machine" && (ev as { event?: unknown }).event === "prefs") load();
    });
  }, []);

  // Esc leaves the Add Tabs tool; leaving the Design workspace does too.
  useEffect(() => {
    if (!tabTool) return;
    const key = (e: KeyboardEvent) => e.key === "Escape" && setTabTool(false);
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [tabTool]);
  useEffect(() => {
    if (!p.design) setTabTool(false);
  }, [p.design]);

  const marks = useMemo(() => tabMarks(p.doc), [p.doc]);
  const objIds = useMemo(() => new Set(p.selection), [p.selection]);
  const exec = (inv: Invocation | Invocation[], done: string) =>
    void P.current
      .run(inv)
      .then(() => P.current.notify(done))
      .catch((e) => P.current.notify(errText(e), "error"));

  const toggleTabTool = () => {
    const on = !tabTool;
    if (on) P.current.onTabTool?.();
    setTabTool(on);
    if (on) P.current.notify(t("tabs.started"));
  };

  // ---- the canvas: tab marks always, the Add Tabs tool while on ----
  const drag = useRef<{ mark: TabMark; from: Vec2; moved: boolean } | null>(null);
  const lastClick = useRef<{ key: string; time: number } | null>(null);
  const hitPx = useRef(tabHitPx("mouse"));
  const [hover, setHover] = useState<string | null>(null);
  const wrap = (inner: CanvasTool): CanvasTool => {
    if (!tabTool && marks.length === 0) return inner;
    // A finger or a pen gets a larger reach than the mouse's 8 px (pointer.ts TOL), set by the press that began.
    const tolMm = (scale: number) => hitPx.current / Math.max(scale, 1e-6);
    return {
      ...inner,
      down: (f, m, info, ctx) => {
        if (!tabTool) return inner.down(f, m, info, ctx);
        hitPx.current = tabHitPx(info.pointerType);
        const mark = nearestTab(marks, f, tolMm(ctx.scale));
        if (mark) {
          const key = `${mark.id}:${mark.index}`;
          const double = lastClick.current?.key === key && info.time - lastClick.current.time < 400;
          lastClick.current = { key, time: info.time };
          if (m.shift || double) {
            lastClick.current = null;
            exec(removeTab(mark), t("tabs.removed"));
            return true;
          }
          drag.current = { mark, from: f, moved: false };
          return true;
        }
        const id = ctx.hitObject(f);
        if (!id) {
          P.current.notify(t("tabs.clickShape"), "error");
          return true;
        }
        const layer = P.current.doc?.objects.find((o) => o.id === id)?.layer ?? "";
        const before = P.current.doc?.layers.find((l) => l.id === layer)?.settings as { options?: { tabs?: { enabled?: boolean } | null } } | undefined;
        const wasOn = before?.options?.tabs?.enabled !== false && !!before?.options?.tabs;
        const layerName = P.current.doc?.layers.find((l) => l.id === layer)?.name ?? layer;
        exec(addTab(id, f, tolMm(ctx.scale) * 2), wasOn ? t("tabs.added") : t("tabs.addedOn", { layer: layerName }));
        return true;
      },
      drag: (f, m, ctx) => {
        if (!tabTool) return inner.drag(f, m, ctx);
        const d = drag.current;
        if (d && Math.hypot(f[0] - d.from[0], f[1] - d.from[1]) > tolMm(ctx.scale) / 2) d.moved = true;
        if (d) ctx.readout(t("tabs.readoutTab"));
      },
      up: (f, m, ctx) => {
        if (!tabTool) return inner.up(f, m, ctx);
        const d = drag.current;
        drag.current = null;
        if (d?.moved) exec(moveTab(d.mark, f, ctx.hitObject(f)), t("tabs.moved"));
      },
      cancel: () => {
        if (!tabTool) return inner.cancel();
        drag.current = null;
      },
      hover: (f, m, ctx) => {
        if (!tabTool) return inner.hover(f, m, ctx);
        hitPx.current = tabHitPx("mouse");
        const mark = nearestTab(marks, f, tolMm(ctx.scale));
        const id = mark ? null : ctx.hitObject(f);
        const key = mark ? `${mark.id}:${mark.index}` : null;
        if (key !== hover) setHover(key);
        ctx.readout(mark ? t("tabs.readoutTab") : id ? t("tabs.readoutShape") : t("tabs.readout"));
        return true;
      },
      render: (r) => (
        <>
          {inner.render(r)}
          <g className="tools-tab-marks" pointerEvents="none">
            {marks.map((mk) => (
              <circle
                key={`${mk.id}:${mk.index}`}
                className={`${objIds.has(mk.id) ? "sel" : ""}${hover === `${mk.id}:${mk.index}` ? " hot" : ""}`}
                cx={mk.at[0]}
                cy={mk.at[1]}
                r={r.px(tabTool ? 5 : 4)}
                strokeWidth={r.px(1)}
              />
            ))}
          </g>
        </>
      ),
      overlay: (r) => (
        <>
          {inner.overlay(r)}
          {tabTool && (
            <div className="vp-overlay lx-sp-bar" role="status">
              <span>{t("tabs.bar")}</span>
              <button className="btn small" onClick={() => setTabTool(false)}>
                {t("tabs.done")}
              </button>
            </div>
          )}
        </>
      ),
    };
  };

  // ---- menus ----
  const ids = p.selection;
  const hasTabs = marks.some((m) => objIds.has(m.id));
  const layerHasTabs = !!p.activeLayer && marks.some((m) => p.doc?.objects.find((o) => o.id === m.id)?.layer === p.activeLayer);
  const imageIds = (p.doc?.objects ?? []).filter((o) => objIds.has(o.id) && (o.geometry as { type?: string }).type === "image").map((o) => o.id);
  const toolsItems: MenuItem[] = [
    // A desktop feature on the phone (placing tabs on an outline wants a pointer), offered there from More while on.
    { label: tabTool ? t("tools.stopTabs") : t("tools.addTabs"), desktopOnly: !tabTool, action: toggleTabTool },
    { label: t("tools.removeTabs"), disabled: !hasTabs, action: () => exec(removeTabs(ids), t("tabs.cleared", { n: marks.filter((m) => objIds.has(m.id)).length })) },
    {
      label: t("tools.clearTabs", { layer: p.doc?.layers.find((l) => l.id === p.activeLayer)?.name ?? p.activeLayer ?? "" }),
      disabled: !layerHasTabs,
      action: () => p.activeLayer && exec(clearTabs(p.activeLayer), t("tabs.cleared", { n: marks.filter((m) => p.doc?.objects.find((o) => o.id === m.id)?.layer === p.activeLayer).length })),
    },
    { sep: true },
    { label: t("tools.dogbone"), disabled: ids.length === 0, action: () => setWin("dogbone") },
    { label: t("tools.resizeSlots"), disabled: ids.length === 0, action: () => setWin("slots") },
    { label: t("tools.multiTrace"), action: () => setWin("trace") },
  ];

  const runUserTool = (tool: UserTool) => {
    if (tool.kind === "url") {
      const url = userToolUrl(tool);
      if (!url) {
        p.notify(t("ut.badUrl"), "error");
        return;
      }
      if (!noticeAccepted()) {
        setNotice(tool);
        return;
      }
      window.open(url, "_blank", "noopener,noreferrer");
      return;
    }
    exec(userToolInvocations(tool), t("ut.ran", { name: tool.name }));
  };
  const webItems: MenuItem[] = [];
  // A category is a submenu (the Menu primitive's), tools without one are plain rows.
  for (const g of groupUserTools(userTools)) {
    const rows: MenuItem[] = g.tools.map((tool) => ({ label: tool.name, action: () => runUserTool(tool) }));
    if (g.category) webItems.push({ label: g.category, submenu: rows });
    else webItems.push(...rows);
  }
  if (webItems.length === 0) webItems.push({ label: t("ut.none"), disabled: true, action: () => undefined });
  webItems.push({ sep: true }, { label: t("ut.manage"), desktopOnly: true, disabled: !supported, action: () => setWin("userTools") });

  // In the ribbon's MODIFY group beside Edit and Tools (and so in its overflow, the tablet rail and the phone strip);
  // command search lists the user tools and Manage User Tools as "Web Tools › ...".
  const webShown = forLayout(webItems, layout);
  usePublish("menu.webTools", { label: t("ut.menu"), items: p.design ? webShown : [] });
  const toolbar: ReactNode = p.design ? (
    <RibbonSlot group="modify" order={35}>
      <Dropdown icon="web" label={t("ut.menu")} title={t("ut.menuTitle")} items={webShown} />
    </RibbonSlot>
  ) : null;

  // ---- windows ----
  const close = () => setWin(null);
  let dialogs: ReactNode = null;
  if (win === "dogbone") dialogs = <DogboneDialog ids={[...ids]} run={p.run} notify={p.notify} onClose={close} />;
  else if (win === "slots") dialogs = <ResizeSlotsDialog ids={[...ids]} run={p.run} notify={p.notify} onClose={close} />;
  else if (win === "trace")
    dialogs = <MultiTraceDialog imageIds={imageIds} activeLayer={p.activeLayer} run={p.run} notify={p.notify} onClose={close} onCreated={(c) => c.length && p.selectIds(c)} />;
  else if (win === "userTools")
    dialogs = (
      <UserToolsDialog
        tools={userTools}
        notify={p.notify}
        onClose={close}
        save={async (tools) => {
          const r = await libsApi.prefsEdit({ user_tools: userToolsBody(tools) });
          setUserTools(parseUserTools(r));
        }}
      />
    );

  if (notice)
    dialogs = (
      <>
        {dialogs}
        <ConfirmDialog
          title={t("ut.menu")}
          ok={t("files.ok")}
          onClose={() => setNotice(null)}
          onOk={() => {
            const tool = notice;
            setNotice(null);
            acceptNotice();
            // Still inside the click on OK, so the browser lets the new tab open.
            const url = userToolUrl(tool);
            if (url) window.open(url, "_blank", "noopener,noreferrer");
          }}
        >
          <p>{t("ut.firstUse")}</p>
        </ConfirmDialog>
      </>
    );

  return {
    toolsItems,
    toolbar,
    wrap,
    dialogs,
    tabToolActive: tabTool,
    stopTabTool: () => setTabTool(false),
    modalOpen: win !== null || notice !== null,
    // For the phone's More > Desktop features (App.tsx).
    toggleTabTool,
    openUserTools: supported ? () => setWin("userTools") : null,
  };
}
