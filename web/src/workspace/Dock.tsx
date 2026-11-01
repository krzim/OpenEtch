// A tool window (Fusion's collapsible palettes, LightBurn's dock windows): a title row that collapses it (remembered in
// the preferences), optional header actions, a window menu, and the body.
//
// Where it shows is the window layout's business (layout.ts): the left or right side panel, the bottom strip under the
// canvas, floating over it, or as a tab in another window's frame; hidden windows render nothing. The Dock portals its
// frame into the slot its area's host offers (DockHost.tsx), so the components that render a Dock never change. With
// no host on the page (tests, a Dock inside a dialog) it renders in place, as it always did.

import { useEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Icon } from "../components/ui.tsx";
import { ContextMenu } from "../components/ContextMenu.tsx";
import { areaUnits, placeOf } from "./layout.ts";
import { updatePrefs } from "./prefs.ts";
import { usePrefs, useT } from "./usePrefs.ts";
import { registerWindow, unregisterWindow, useHostsActive, useLayout, useMountedWindows, useSlot } from "./windows.ts";
import { windowMenuItems } from "./windowMenu.ts";
import "./workspace.css";

export function Dock(p: { id: string; title: string; actions?: ReactNode; children: ReactNode; defaultCollapsed?: boolean }) {
  const t = useT();
  const docks = usePrefs().docks;
  const layout = useLayout();
  const mounted = useMountedWindows();
  const hostsActive = useHostsActive();
  const slot = useSlot(p.id);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  useEffect(() => {
    registerWindow(p.id, p.title);
  }, [p.id, p.title]);
  useEffect(() => () => unregisterWindow(p.id), [p.id]);

  const place = placeOf(layout, p.id);
  if (!place.visible && hostsActive) return null;
  const unit = hostsActive ? areaUnits(layout, place.area, [...mounted.keys()]).find((u) => u.members.includes(p.id)) : undefined;
  // In a tab group the host draws the tabs (and the window menu); the window shows its actions and body only.
  const tabbed = !!unit && unit.members.length > 1;
  const floating = place.area === "float";
  const collapsed = !tabbed && !floating && (docks[p.id] ?? !!p.defaultCollapsed);
  const toggle = () => updatePrefs((cur) => ({ docks: { ...cur.docks, [p.id]: !collapsed } }));
  const openMenu = (e: React.MouseEvent) => setMenu({ x: e.clientX, y: e.clientY });

  const frame = (
    <section className={`dock${collapsed ? " collapsed" : ""}${tabbed ? " tabbed" : ""}`} aria-label={p.title}>
      {tabbed ? (
        p.actions && <div className="dock-head dock-head-tabbed">{<div className="dock-actions">{p.actions}</div>}</div>
      ) : (
        <div className="dock-head" onContextMenu={hostsActive ? (e) => (e.preventDefault(), openMenu(e)) : undefined}>
          <button className="dock-toggle" aria-expanded={!collapsed} onClick={floating ? undefined : toggle} data-drag-handle={floating ? "" : undefined}>
            <span className={`chevron${collapsed ? "" : " open"}`}>
              <Icon name="chevron" size={14} />
            </span>
            <span className="dock-title">{p.title}</span>
          </button>
          {p.actions && !collapsed && <div className="dock-actions">{p.actions}</div>}
          {hostsActive && (
            <button className="icon-btn small dock-menu-btn" title={t("win.menuTitle")} aria-label={t("win.menuTitle")} aria-haspopup="menu" onClick={openMenu}>
              <Icon name="more" size={16} />
            </button>
          )}
        </div>
      )}
      {!collapsed && <div className="dock-body">{p.children}</div>}
      {menu && <ContextMenu x={menu.x} y={menu.y} items={windowMenuItems(p.id, t)} onClose={() => setMenu(null)} />}
    </section>
  );
  if (!hostsActive) return frame;
  // The host has not offered a slot yet (it renders one once this window is registered).
  if (!slot) return null;
  return createPortal(frame, slot);
}
