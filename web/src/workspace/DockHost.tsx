// The hosts of the dockable windows (layout.ts): one per area — the left and right side panels, the bottom strip under
// the canvas, and the floating layer — each rendering a frame per window (or per tab group, with its tab strip) and the
// slot the window's Dock portals into. Plus the Window menu in the top bar: show or hide each window and LightBurn's
// Reset to Default Layout.

import { useEffect, useRef, useState, type PointerEvent as RPointerEvent } from "react";
import { ContextMenu } from "../components/ContextMenu.tsx";
import { Icon } from "../components/ui.tsx";
import { areaUnits, getLayout, placeOf, resetLayout, setActiveTab, setFloatRect, setVisible, updateLayout, WINDOWS, type AreaUnit, type DockArea, type FloatRect } from "./layout.ts";
import { reloadLayout, LAYOUT_KEY } from "./layout.ts";
import { useT } from "./usePrefs.ts";
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
export function FloatHost() {
  useHost();
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

/** The Window menu's entries: every tool window with a check mark (click shows or hides it), and Reset to Default
 * Layout. Shared by the Window dropdown (wide layout) and the top bar's ⋯ overflow menu. */
export function WindowMenuEntries(p: { close: () => void }) {
  const t = useT();
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

/** Another tab changed the stored layout: adopt it (mounted once, in App). */
export function useLayoutSync(): void {
  useEffect(() => {
    const onStorage = (e: StorageEvent) => e.key === LAYOUT_KEY && reloadLayout();
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
}

/** The Window menu in the top bar (wide layout). */
export function WindowMenu() {
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
          <WindowMenuEntries close={() => setOpen(false)} />
        </div>
      )}
    </div>
  );
}
