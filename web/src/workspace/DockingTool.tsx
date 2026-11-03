// LightBurn's Docking toolbar as a tool window (Window > Docking): four direction buttons that slide the selection until
// it touches another shape or the workspace edge, with Move as group, Lock inner objects and Padding (docking.ts has
// the rules). The same moves are in the canvas right-click menu (Dock left/right/up/down). One `object.transform` per
// moved unit, sent as one batch: one undo step. The options are kept in this browser.

import { useCallback, useSyncExternalStore } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import type { MenuItem } from "../components/ContextMenu.tsx";
import { NumberField } from "../components/ui.tsx";
import { roundMm } from "../editing.ts";
import { unitMembers, unitsOf } from "../selection.ts";
import { Dock } from "./Dock.tsx";
import { DEFAULT_DOCK_OPTIONS, dockMoves, objectOutline, polysBox, type DockDir, type DockOptions, type DockShape } from "./docking.ts";
import { useT } from "./usePrefs.ts";

const KEY = "openetch.docking";

function load(): DockOptions {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<DockOptions> | null;
    if (!raw || typeof raw !== "object") return DEFAULT_DOCK_OPTIONS;
    const pad = typeof raw.padding === "number" && Number.isFinite(raw.padding) && raw.padding >= 0 && raw.padding <= 1000 ? raw.padding : 0;
    return { moveAsGroup: raw.moveAsGroup === true, lockInner: raw.lockInner !== false, padding: pad };
  } catch {
    return DEFAULT_DOCK_OPTIONS;
  }
}

let opts = load();
const listeners = new Set<() => void>();
function setOpts(o: DockOptions): void {
  opts = o;
  try {
    localStorage.setItem(KEY, JSON.stringify(o));
  } catch {
    // Storage unavailable: kept for this visit.
  }
  for (const l of listeners) l();
}
const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
};

/** The docking moves of the selection as transform invocations (empty when nothing moves). */
export function dockInvocations(doc: GDocument, selected: readonly string[], dir: DockDir, ws: Vec2, o: DockOptions): Invocation[] {
  const units = unitsOf(doc, selected);
  if (units.length === 0) return [];
  const byId = new Map(doc.objects.map((x) => [x.id, x]));
  const visible = new Set(doc.layers.filter((l) => l.visible).map((l) => l.id));
  const shapeOf = (id: string, members: string[]): DockShape | null => {
    const objs = members.map((m) => byId.get(m)).filter((x) => !!x);
    const polys = objs.flatMap((x) => objectOutline(x!));
    const box = polysBox(polys);
    return box ? { id, polys, box } : null;
  };
  const moving: DockShape[] = [];
  const members = new Map<string, string[]>();
  for (const u of units) {
    const m = unitMembers(doc, u);
    members.set(u, m);
    const s = shapeOf(u, m);
    if (s) moving.push(s);
  }
  const taken = new Set([...members.values()].flat());
  const others: DockShape[] = [];
  for (const x of doc.objects) {
    if (taken.has(x.id) || !visible.has(x.layer)) continue;
    const s = shapeOf(x.id, [x.id]);
    if (s) others.push(s);
  }
  return dockMoves(moving, others, dir, ws, o)
    .map((mv) => ({ ids: members.get(mv.id) ?? [], d: [roundMm(mv.d[0]), roundMm(mv.d[1])] as Vec2 }))
    .filter((mv) => mv.ids.length && (mv.d[0] !== 0 || mv.d[1] !== 0))
    .map((mv) => C.transform(mv.ids, { translate: mv.d }));
}

export function useDocking(p: {
  doc: GDocument | null;
  selected: string[];
  workspaceMm: Vec2;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  report: (e: unknown) => void;
  notify: (msg: string, kind?: "info" | "error") => void;
}): { dock: (dir: DockDir) => void; window: JSX.Element; menuItems: MenuItem[] } {
  const t = useT();
  const o = useSyncExternalStore(subscribe, () => opts, () => opts);
  const { doc, selected, workspaceMm, run, report, notify } = p;
  const dock = useCallback(
    (dir: DockDir) => {
      if (!doc || selected.length === 0) {
        notify(t("dock.selectFirst"), "error");
        return;
      }
      const invs = dockInvocations(doc, selected, dir, workspaceMm, opts);
      if (invs.length === 0) {
        notify(t("dock.nothing"));
        return;
      }
      void run(invs.length === 1 ? invs[0] : invs).catch(report);
    },
    [doc, selected, workspaceMm, run, report, notify, t],
  );
  const dirs: { dir: DockDir; label: string; glyph: string }[] = [
    { dir: "left", label: t("dock.left"), glyph: "⇤" },
    { dir: "up", label: t("dock.up"), glyph: "⤒" },
    { dir: "down", label: t("dock.down"), glyph: "⤓" },
    { dir: "right", label: t("dock.right"), glyph: "⇥" },
  ];
  const none = selected.length === 0;
  const window = (
    <Dock help="arranging-and-arrays#docking" id="docking" title={t("dock.title")} defaultCollapsed>
      <div className="docking-buttons" role="group" aria-label={t("dock.title")}>
        {dirs.map((d) => (
          <button key={d.dir} className="btn docking-btn" disabled={none} title={d.label} aria-label={d.label} onClick={() => dock(d.dir)}>
            <span aria-hidden="true">{d.glyph}</span>
          </button>
        ))}
      </div>
      <label className="field field-check">
        <span className="field-label">{t("dock.moveAsGroup")}</span>
        <input type="checkbox" checked={o.moveAsGroup} onChange={(e) => setOpts({ ...o, moveAsGroup: e.target.checked })} />
      </label>
      <label className="field field-check">
        <span className="field-label">{t("dock.lockInner")}</span>
        <input type="checkbox" checked={o.lockInner} disabled={o.moveAsGroup} onChange={(e) => setOpts({ ...o, lockInner: e.target.checked })} />
      </label>
      <NumberField label={t("dock.padding")} unit="mm" value={o.padding} min={0} max={1000} digits={3} onCommit={(v) => v !== null && setOpts({ ...o, padding: v })} />
      <p className="hint">{t("dock.hint")}</p>
    </Dock>
  );
  const menuItems: MenuItem[] = dirs.map((d) => ({ label: d.label, disabled: none, action: () => dock(d.dir) }));
  return { dock, window, menuItems };
}
