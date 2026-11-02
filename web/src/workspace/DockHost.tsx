// The hosts of the dockable windows (layout.ts): one per area — the left and right side panels, the bottom strip under
// the canvas, and the floating layer — each rendering a frame per window (or per tab group, with its tab strip) and the
// slot the window's Dock portals into. Plus the Window menu in the top bar: show or hide each window, the layout
// presets, Full screen, Focus canvas and LightBurn's Reset to Default Layout.
//
// The side panels (docs/ux/ui-plan-2026-11.md 1.1): `SidePanel` draws a side pinned beside the canvas with a splitter
// on its inner edge (pointer events, touch-action none, double-click resets the width, arrow keys resize), or as a rail
// of vertical window titles whose tap (or a hover with a mouse) opens the side as an overlay with a pin button.

import { useCallback, useEffect, useRef, useState, type KeyboardEvent as RKeyboardEvent, type PointerEvent as RPointerEvent, type ReactNode } from "react";
import { ContextMenu } from "../components/ContextMenu.tsx";
import { Icon } from "../components/ui.tsx";
import {
  applyPreset,
  areaUnits,
  clampSize,
  DEFAULT_SIZES,
  getLayout,
  LAYOUT_PRESETS,
  placeOf,
  resetLayout,
  setActiveTab,
  setAreaMode,
  setDockSize,
  setFloatRect,
  setVisible,
  updateLayout,
  WINDOWS,
  type AreaMode,
  type AreaUnit,
  type DockArea,
  type DockSizes,
  type FloatRect,
  type LayoutPreset,
  type SideArea,
} from "./layout.ts";
import { reloadLayout, LAYOUT_KEY } from "./layout.ts";
import { useFullscreen } from "./fullscreen.ts";
import { hintFor } from "./hotkeys.ts";
import { usePrefs, useT } from "./usePrefs.ts";
import { hostMounted, slotRef, useLayout, useMountedWindows } from "./windows.ts";
import { windowMenuItems } from "./windowMenu.ts";
import type { MsgKey } from "./i18n.ts";
import "./workspace.css";

function useHost(): void {
  useEffect(() => {
    hostMounted(1);
    return () => hostMounted(-1);
  }, []);
}

/** Whether `area` holds a shown window now (App hides an empty side panel). */
export function useAreaHasWindows(area: DockArea): boolean {
  const layout = useLayout();
  const mounted = useMountedWindows();
  return areaUnits(layout, area, [...mounted.keys()]).length > 0;
}

/** The windows docked in a side panel or the bottom strip. */
export function DockAreaHost(p: { area: Exclude<DockArea, "float"> }) {
  useHost();
  const layout = useLayout();
  const mounted = useMountedWindows();
  const units = areaUnits(layout, p.area, [...mounted.keys()]);
  return (
    <div className={`dock-area dock-area-${p.area}`} data-empty={units.length === 0 ? "" : undefined}>
      {units.map((u) => (
        <UnitFrame key={u.key} unit={u} />
      ))}
    </div>
  );
}

/** One frame: a single window's slot, or a tab group's strip and its members' slots (only the active one shown). */
function UnitFrame(p: { unit: AreaUnit }) {
  const t = useT();
  const mounted = useMountedWindows();
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const { unit } = p;
  if (unit.members.length === 1) return <div className="dock-slot" ref={slotRef(unit.members[0])} />;
  const group = placeOf(getLayout(), unit.members[0]).group ?? unit.members[0];
  return (
    <div className="dock-group">
      <div className="dock-tabs" role="tablist" data-drag-handle="" onContextMenu={(e) => (e.preventDefault(), setMenu({ x: e.clientX, y: e.clientY }))}>
        {unit.members.map((m) => (
          <button
            key={m}
            role="tab"
            aria-selected={m === unit.active}
            className={`dock-tab${m === unit.active ? " active" : ""}`}
            onClick={() => updateLayout((l) => setActiveTab(l, group, m))}
          >
            {mounted.get(m) ?? m}
          </button>
        ))}
        <span className="tool-spacer" />
        <button
          className="icon-btn small dock-menu-btn"
          title={t("win.menuTitle")}
          aria-label={t("win.menuTitle")}
          aria-haspopup="menu"
          onClick={(e) => setMenu({ x: e.clientX, y: e.clientY })}
        >
          <Icon name="more" size={16} />
        </button>
      </div>
      {unit.members.map((m) => (
        <div key={m} className="dock-slot" ref={slotRef(m)} hidden={m !== unit.active} />
      ))}
      {menu && <ContextMenu x={menu.x} y={menu.y} items={windowMenuItems(unit.active, t)} onClose={() => setMenu(null)} />}
    </div>
  );
}

/** The floating windows: frames over the canvas, moved by their title (or tab strip) and resized from the corner. */
/** The window's size, re-read on resize and orientation changes (floating frames are re-clamped to it, L15). */
export function useWindowSize(): { w: number; h: number } {
  const read = () => ({ w: typeof window === "undefined" ? 1200 : window.innerWidth, h: typeof window === "undefined" ? 800 : window.innerHeight });
  const [size, setSize] = useState(read);
  useEffect(() => {
    let frame = 0;
    const on = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setSize((cur) => {
        const next = read();
        return next.w === cur.w && next.h === cur.h ? cur : next;
      }));
    };
    window.addEventListener("resize", on);
    window.addEventListener("orientationchange", on);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", on);
      window.removeEventListener("orientationchange", on);
    };
  }, []);
  return size;
}

export function FloatHost() {
  useHost();
  // Re-render (and so re-clamp every frame) when the window is resized or rotated.
  useWindowSize();
  const layout = useLayout();
  const mounted = useMountedWindows();
  const units = areaUnits(layout, "float", [...mounted.keys()]);
  return (
    <div className="dock-float-layer">
      {units.map((u, i) => (
        <FloatFrame key={u.key} unit={u} rect={placeOf(layout, u.members[0]).float ?? { x: 120 + 28 * i, y: 120 + 28 * i, w: 320, h: 360 }} />
      ))}
    </div>
  );
}

/** Keep a floating frame reachable: its title row stays inside the window. */
export function clampFloat(r: FloatRect, view: { w: number; h: number }): FloatRect {
  const w = Math.min(Math.max(160, r.w), Math.max(160, view.w));
  const h = Math.min(Math.max(100, r.h), Math.max(100, view.h));
  return { x: Math.min(Math.max(r.x, 40 - w), view.w - 40), y: Math.min(Math.max(r.y, 0), view.h - 32), w, h };
}

function FloatFrame(p: { unit: AreaUnit; rect: FloatRect }) {
  const [live, setLive] = useState<FloatRect | null>(null);
  const drag = useRef<{ kind: "move" | "size"; sx: number; sy: number; r: FloatRect; id: number } | null>(null);
  // Set once a press became a drag: the click that ends it must not also act on the title row or a tab.
  const dragged = useRef(false);
  const view = { w: typeof window === "undefined" ? 1200 : window.innerWidth, h: typeof window === "undefined" ? 800 : window.innerHeight };
  const r = clampFloat(live ?? p.rect, view);
  const save = (rect: FloatRect) => {
    for (const m of p.unit.members) updateLayout((l) => setFloatRect(l, m, rect));
  };
  const start = (kind: "move" | "size") => (e: RPointerEvent<HTMLElement>) => {
    if (e.button !== 0) return;
    if (kind === "move") {
      const target = e.target as HTMLElement;
      // Only the title row or the tab strip moves the frame; its buttons and fields keep their clicks.
      if (!target.closest("[data-drag-handle]") || target.closest(".dock-actions, .dock-menu-btn, input, select, textarea")) return;
    }
    e.stopPropagation();
    drag.current = { kind, sx: e.clientX, sy: e.clientY, r, id: e.pointerId };
    dragged.current = false;
  };
  const moveTo = (e: RPointerEvent<HTMLElement>) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.sx;
    const dy = e.clientY - d.sy;
    if (!dragged.current) {
      if (Math.abs(dx) + Math.abs(dy) < 4) return;
      // Captured only once it is a drag, so a plain click still reaches the tab or button under the pointer.
      dragged.current = true;
      try {
        (e.currentTarget as HTMLElement).setPointerCapture(d.id);
      } catch {
        // The pointer is gone already.
      }
    }
    setLive(d.kind === "move" ? { ...d.r, x: d.r.x + dx, y: d.r.y + dy } : { ...d.r, w: d.r.w + dx, h: d.r.h + dy });
  };
  const end = () => {
    const was = live;
    drag.current = null;
    if (was) {
      save(clampFloat(was, view));
      setLive(null);
    }
    window.setTimeout(() => (dragged.current = false), 0);
  };
  const { unit } = p;
  return (
    <div
      className="dock-float"
      style={{ left: r.x, top: r.y, width: r.w, height: r.h }}
      onPointerDown={start("move")}
      onPointerMove={moveTo}
      onPointerUp={end}
      onPointerCancel={end}
      // A click that ended a drag must not also toggle the title row.
      onClickCapture={(e) => {
        if (dragged.current) e.stopPropagation();
      }}
    >
      <div className="dock-float-body">
        <UnitFrame unit={unit} />
      </div>
      <div className="dock-float-grip" onPointerDown={start("size")} onPointerMove={moveTo} onPointerUp={end} onPointerCancel={end} aria-hidden="true" />
    </div>
  );
}

/** The Window menu's entries: every tool window with a check mark (click shows or hides it), the layout presets, Full
 * screen, Focus canvas and Reset to Default Layout. Shared by the Window dropdown (wide layout) and the top bar's ⋯
 * overflow menu. */
export function WindowMenuEntries(p: { close: () => void; focusCanvas?: boolean; onFocusCanvas?: () => void }) {
  const t = useT();
  const prefs = usePrefs();
  const fs = useFullscreen();
  const layout = useLayout();
  const mounted = useMountedWindows();
  const known = WINDOWS.map((w) => w.id);
  const extra = [...mounted.keys()].filter((id) => !known.includes(id));
  const entries = [
    ...WINDOWS.map((w) => ({ id: w.id, title: mounted.get(w.id) ?? t(w.titleKey as MsgKey) })),
    ...extra.map((id) => ({ id, title: mounted.get(id) ?? id })),
  ];
  return (
    <>
      {entries.map((e) => {
        const shown = placeOf(layout, e.id).visible;
        const here = mounted.has(e.id);
        return (
          <button
            key={e.id}
            role="menuitemcheckbox"
            aria-checked={shown}
            className="menu-item"
            title={here ? undefined : t("win.notHereTitle")}
            onClick={() => updateLayout((l) => setVisible(l, e.id, !shown))}
          >
            <span>
              <span className="menu-check" aria-hidden="true">
                {shown ? "✓" : ""}
              </span>
              {e.title}
            </span>
            {!here && <span className="menu-hint">{t("win.notHere")}</span>}
          </button>
        );
      })}
      <div className="menu-sep" role="separator" />
      {LAYOUT_PRESETS.map((preset) => (
        <button
          key={preset}
          role="menuitem"
          className="menu-item"
          onClick={() => {
            p.close();
            updateLayout((l) => applyPreset(l, preset));
          }}
        >
          <span>{t("shell.layoutPreset", { name: presetName(preset, t) })}</span>
        </button>
      ))}
      <div className="menu-sep" role="separator" />
      {fs.supported && (
        <button
          role="menuitemcheckbox"
          aria-checked={fs.active}
          className="menu-item"
          onClick={() => {
            p.close();
            fs.toggle();
          }}
        >
          <span>
            <span className="menu-check" aria-hidden="true">
              {fs.active ? "✓" : ""}
            </span>
            {t("shell.fullscreen")}
          </span>
          <span className="menu-hint">{hintFor("view.fullscreen", prefs.hotkeys)}</span>
        </button>
      )}
      {p.onFocusCanvas && (
        <button
          role="menuitemcheckbox"
          aria-checked={!!p.focusCanvas}
          className="menu-item"
          onClick={() => {
            p.close();
            p.onFocusCanvas?.();
          }}
        >
          <span>
            <span className="menu-check" aria-hidden="true">
              {p.focusCanvas ? "✓" : ""}
            </span>
            {t("shell.focusCanvas")}
          </span>
          <span className="menu-hint">{hintFor("view.focusCanvas", prefs.hotkeys)}</span>
        </button>
      )}
      <button
        role="menuitem"
        className="menu-item"
        onClick={() => {
          p.close();
          resetLayout();
        }}
      >
        <span>{t("win.reset")}</span>
      </button>
    </>
  );
}

function presetName(preset: LayoutPreset, t: ReturnType<typeof useT>): string {
  return preset === "wide" ? t("shell.presetWide") : preset === "compact" ? t("shell.presetCompact") : t("shell.presetTouch");
}

/** Another tab changed the stored layout: adopt it (mounted once, in App). */
export function useLayoutSync(): void {
  useEffect(() => {
    const onStorage = (e: StorageEvent) => e.key === LAYOUT_KEY && reloadLayout();
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
}

/** The Window menu in the top bar (wide layout). */
export function WindowMenu(p: { focusCanvas?: boolean; onFocusCanvas?: () => void }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => root.current && !root.current.contains(e.target as Node) && setOpen(false);
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", down);
    window.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", down);
      window.removeEventListener("keydown", key);
    };
  }, [open]);
  return (
    <div className="file-menu window-menu" ref={root}>
      <button className={`file-btn${open ? " open" : ""}`} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        {t("win.menu")}
        <Icon name="chevronDown" size={16} />
      </button>
      {open && (
        <div className="menu menu-right" role="menu" aria-label={t("win.menu")}>
          <WindowMenuEntries close={() => setOpen(false)} focusCanvas={p.focusCanvas} onFocusCanvas={p.onFocusCanvas} />
        </div>
      )}
    </div>
  );
}

/**
 * Esc belongs to something else first: a dialog, an open menu, or a field being edited (its Esc reverts the edit).
 * Overlays and sheets close on Esc only when none of these is there.
 */
export function escapeTakenElsewhere(e: KeyboardEvent): boolean {
  const el = e.target as HTMLElement | null;
  const typing = !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
  return typing || !!document.querySelector(".modal-backdrop, .menu");
}

// ---- side panels: splitter, rail, overlay -------------------------------------------------------------------------------

/** Pixels per arrow-key press on a splitter. */
const SPLITTER_STEP = 16;

/**
 * The splitter on a pinned side panel's inner edge (or above the bottom strip): drag to resize (pointer events, so it
 * works with a finger and a pen; the width is set live on `.app` and saved on release), double-click to reset, arrow
 * keys (Home / End) with keyboard focus.
 */
export function Splitter(p: { area: keyof DockSizes; label: string }) {
  const t = useT();
  const size = useLayout().sizes[p.area];
  const drag = useRef<{ start: number; size: number; live: number; app: HTMLElement | null } | null>(null);
  const vertical = p.area !== "bottom";
  // Dragging towards the canvas grows the panel: right for the left side, left for the right side, up for the bottom.
  const sign = p.area === "left" ? 1 : -1;
  const cssVar = `--dock-${p.area}-size`;
  const commit = (px: number) => updateLayout((l) => setDockSize(l, p.area, px));
  const onDown = (e: RPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // The pointer is already gone.
    }
    const cur = getLayout().sizes[p.area];
    drag.current = { start: vertical ? e.clientX : e.clientY, size: cur, live: cur, app: e.currentTarget.closest<HTMLElement>(".app") };
  };
  const onMove = (e: RPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    d.live = clampSize(p.area, d.size + sign * ((vertical ? e.clientX : e.clientY) - d.start));
    d.app?.style.setProperty(cssVar, `${d.live}px`);
  };
  const onUp = () => {
    const d = drag.current;
    if (!d) return;
    drag.current = null;
    if (d.live !== d.size) commit(d.live);
  };
  const onKey = (e: RKeyboardEvent<HTMLDivElement>) => {
    const grow = vertical ? (p.area === "left" ? "ArrowRight" : "ArrowLeft") : "ArrowUp";
    const shrink = vertical ? (p.area === "left" ? "ArrowLeft" : "ArrowRight") : "ArrowDown";
    let next: number | null = null;
    if (e.key === grow) next = size + SPLITTER_STEP;
    else if (e.key === shrink) next = size - SPLITTER_STEP;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = 100000;
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    commit(next);
  };
  return (
    <div
      role="separator"
      tabIndex={0}
      aria-orientation={vertical ? "vertical" : "horizontal"}
      aria-label={p.label}
      aria-valuenow={size}
      title={t("shell.splitterTitle")}
      className={`dock-splitter dock-splitter-${p.area}`}
      onPointerDown={onDown}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onPointerCancel={onUp}
      onDoubleClick={() => commit(DEFAULT_SIZES[p.area])}
      onKeyDown={onKey}
    />
  );
}

/** A rail of a side's window titles, written vertically: a tap (or resting the mouse on it) opens the side. */
function DockRail(p: { side: SideArea; open: boolean; label: string; onOpen: (id: string, by: "click" | "hover") => void }) {
  const layout = useLayout();
  const mounted = useMountedWindows();
  const timer = useRef(0);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const ids = areaUnits(layout, p.side, [...mounted.keys()]).flatMap((u) => u.members);
  return (
    <nav className={`dock-rail dock-rail-${p.side}`} aria-label={p.label}>
      {ids.map((id) => (
        <button
          key={id}
          type="button"
          className="dock-rail-btn"
          aria-expanded={p.open}
          onClick={() => {
            window.clearTimeout(timer.current);
            p.onOpen(id, "click");
          }}
          onPointerEnter={(e) => {
            if (e.pointerType !== "mouse" || p.open) return;
            window.clearTimeout(timer.current);
            timer.current = window.setTimeout(() => p.onOpen(id, "hover"), 400);
          }}
          onPointerLeave={() => window.clearTimeout(timer.current)}
        >
          <span className="dock-rail-label">{mounted.get(id) ?? id}</span>
        </button>
      ))}
    </nav>
  );
}

export interface SidePanelProps {
  side: SideArea;
  /** What the side is now (layout.ts `dockPolicy`, or hidden). */
  mode: AreaMode;
  /** A rail's overlay is open. */
  overlay: boolean;
  onOverlay: (open: boolean) => void;
  /** The panel's name (accessible name, the bar's caption). */
  label: string;
  /** Draw the rail strip (false: the side opens only from its toolbar toggle; the tablet's left side, whose Browser
   * button sits in the tool rail, so the canvas keeps its width). */
  railVisible?: boolean;
  children: ReactNode;
}

/**
 * One side of the canvas: pinned (the panel, a caption bar with "collapse to a rail", and the splitter), a rail that
 * opens the panel as an overlay (with "pin" and "close"; Esc or a tap outside closes it), or hidden. The panel stays
 * mounted in every mode, so the windows docked in it keep their state; when it is not shown it is `inert`.
 */
export function SidePanel(p: SidePanelProps) {
  const t = useT();
  const aside = useRef<HTMLElement>(null);
  const byHover = useRef(false);
  const leave = useRef(0);
  const { onOverlay } = p;
  const rail = p.mode === "rail";
  const railVisible = rail && p.railVisible !== false;
  const overlayOpen = rail && p.overlay;
  const shown = p.mode === "pinned" || overlayOpen;
  useEffect(() => {
    if (aside.current) aside.current.inert = !shown;
  }, [shown]);
  useEffect(() => () => window.clearTimeout(leave.current), []);
  // Esc closes the overlay and nothing else (a dialog over it keeps its own Esc).
  useEffect(() => {
    if (!overlayOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || escapeTakenElsewhere(e)) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      onOverlay(false);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [overlayOpen, onOverlay]);
  const open = useCallback(
    (id: string, by: "click" | "hover") => {
      const group = placeOf(getLayout(), id).group;
      if (group) updateLayout((l) => setActiveTab(l, group, id));
      byHover.current = by === "hover";
      onOverlay(true);
    },
    [onOverlay],
  );
  const onLeave = (e: RPointerEvent<HTMLElement>) => {
    if (!byHover.current || e.pointerType !== "mouse") return;
    window.clearTimeout(leave.current);
    leave.current = window.setTimeout(() => {
      if (!aside.current?.contains(document.activeElement)) onOverlay(false);
    }, 350);
  };
  const panel = (
    <aside
      ref={aside}
      className={`panel panel-${p.side}`}
      data-mode={p.mode}
      data-overlay={overlayOpen ? "" : undefined}
      data-norail={rail && !railVisible ? "" : undefined}
      aria-label={p.label}
      onPointerEnter={() => window.clearTimeout(leave.current)}
      onPointerLeave={onLeave}
      onPointerDown={() => (byHover.current = false)}
    >
      {p.mode !== "hidden" && (
        <div className="panel-bar">
          <span className="panel-bar-title">{p.label}</span>
          {rail ? (
            <>
              <button
                className="icon-btn small"
                title={t("shell.pinPanel")}
                aria-label={t("shell.pinPanel")}
                onClick={() => {
                  updateLayout((l) => setAreaMode(l, p.side, "pinned"));
                  onOverlay(false);
                }}
              >
                <Icon name="pin" size={16} />
              </button>
              <button className="icon-btn small" title={t("common.close")} aria-label={t("common.close")} onClick={() => onOverlay(false)}>
                <Icon name="close" size={16} />
              </button>
            </>
          ) : (
            <button
              className="icon-btn small"
              title={t("shell.railPanel")}
              aria-label={t("shell.railPanel")}
              onClick={() => updateLayout((l) => setAreaMode(l, p.side, "rail"))}
            >
              <Icon name={p.side === "left" ? "panelLeft" : "panelRight"} size={16} />
            </button>
          )}
        </div>
      )}
      <div className="panel-body">{p.children}</div>
    </aside>
  );
  const edge = railVisible ? (
    <DockRail side={p.side} open={overlayOpen} label={p.label} onOpen={open} />
  ) : p.mode === "pinned" ? (
    <Splitter area={p.side} label={t("shell.resize", { name: p.label })} />
  ) : null;
  const scrim = overlayOpen ? <div className="drawer-scrim" aria-hidden="true" onPointerDown={() => onOverlay(false)} /> : null;
  // The rail sits on the window's edge, the splitter on the canvas side.
  return p.side === "left" ? (
    <>
      {railVisible && edge}
      {panel}
      {!rail && edge}
      {scrim}
    </>
  ) : (
    <>
      {scrim}
      {!rail && edge}
      {panel}
      {railVisible && edge}
    </>
  );
}
