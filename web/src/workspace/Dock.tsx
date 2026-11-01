// A docked tool window in a side panel (Fusion's collapsible palettes, LightBurn's dock windows): a title row that
// collapses it (remembered in the preferences), optional header actions, and the body.

import type { ReactNode } from "react";
import { Icon } from "../components/ui.tsx";
import { updatePrefs } from "./prefs.ts";
import { usePrefs } from "./usePrefs.ts";
import "./workspace.css";

export function Dock(p: { id: string; title: string; actions?: ReactNode; children: ReactNode; defaultCollapsed?: boolean }) {
  const docks = usePrefs().docks;
  const collapsed = docks[p.id] ?? !!p.defaultCollapsed;
  const toggle = () => updatePrefs((cur) => ({ docks: { ...cur.docks, [p.id]: !collapsed } }));
  return (
    <section className={`dock${collapsed ? " collapsed" : ""}`} aria-label={p.title}>
      <div className="dock-head">
        <button className="dock-toggle" aria-expanded={!collapsed} onClick={toggle}>
          <span className={`chevron${collapsed ? "" : " open"}`}>
            <Icon name="chevron" size={14} />
          </span>
          <span className="dock-title">{p.title}</span>
        </button>
        {p.actions && !collapsed && <div className="dock-actions">{p.actions}</div>}
      </div>
      {!collapsed && <div className="dock-body">{p.children}</div>}
    </section>
  );
}
