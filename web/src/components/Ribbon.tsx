// The ribbon (docs/ux/ui-plan-2026-11.md P2 "Ribbon"): named groups that never scroll.
// - horizontal (desktop): groups sit inline; when they do not fit, the lowest-priority ones collapse into `GROUP ▾`
//   buttons that open the group's tools in a flyout (useRibbonOverflow + ribbonModel.ts `fitGroups`), so
//   scrollWidth <= clientWidth and every tool is two clicks away at most.
// - vertical (tablet tool rail): pinned groups stack inline, the others are `▸` buttons with flyouts to the side.
// - strip (phone): every group inline in one row that scrolls sideways inside the Canvas sheet.
//
// A group's tools can come from several places: `<RibbonSlot group="create" order={20}>` anywhere inside the ribbon
// puts its children into that group (the text hook's Text and Barcode, the edit tools' Line / Pen / Polygon), ordered
// by `order`. When the group is collapsed they render in its flyout instead.

import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type CSSProperties, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { installEscape, layers } from "./dialogStack.ts";
import { isInMenu } from "./Menu.tsx";
import { placeMenu } from "./menuModel.ts";
import { fitGroups, sameSet, samePos, type GroupFit } from "./ribbonModel.ts";
import { Icon } from "./ui.tsx";
import "./ribbon.css";

export type RibbonOrientation = "horizontal" | "vertical" | "strip";

export interface RibbonProps {
  /** horizontal: the desktop ribbon; vertical: the tablet tool rail; strip: the phone's tool strip. */
  orientation: RibbonOrientation;
  children?: ReactNode;
  className?: string;
  /** Accessible name (default none; the toolbar names itself). */
  label?: string;
  /** Right-click on the ribbon's empty space (the ribbon's own options). */
  onContextMenu?: (x: number, y: number) => void;
}

interface RibbonCtx {
  orientation: RibbonOrientation;
  collapsed: ReadonlySet<string>;
  /** The element a group's slot content renders into (its inline body, or its open flyout), by group id. */
  target: (id: string) => HTMLElement | null;
  setTarget: (id: string, el: HTMLElement | null) => void;
  report: (id: string, fit: Partial<GroupFit>) => void;
  /** Bumps whenever a target changes, so slots re-render. */
  version: number;
}

const Ctx = createContext<RibbonCtx | null>(null);

const DEFAULT_COLLAPSED_W = 64;

/**
 * Measure the ribbon and its groups and decide which groups collapse (ResizeObserver on the ribbon and every group).
 * Widths of collapsed groups are the ones last seen inline.
 */
export function useRibbonOverflow(ref: { current: HTMLElement | null }, enabled: boolean) {
  const fits = useRef(new Map<string, GroupFit>());
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const live = useRef(collapsed);
  live.current = collapsed;
  const on = useRef(enabled);
  on.current = enabled;
  const frame = useRef(0);

  // Stable across renders (the groups' effects depend on `report`).
  const compute = useRef(() => {
    cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => {
      const el = ref.current;
      if (!el || !on.current) {
        if (live.current.size) setCollapsed(new Set());
        return;
      }
      // Space for the groups: the ribbon's content box minus everything that is not a group (end caps, spacer).
      // Only the ribbon's own row counts: a detached rail (position: fixed) is not in it.
      const cs = getComputedStyle(el);
      const pad = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
      const gap = parseFloat(cs.columnGap) || 0;
      let fixed = 0;
      let n = 0;
      const ids: string[] = [];
      for (const c of Array.from(el.children) as HTMLElement[]) {
        if (getComputedStyle(c).position === "fixed") continue;
        n++;
        if (c.classList.contains("ribbon-group")) ids.push(c.dataset.group ?? "");
        else if (!c.classList.contains("tool-spacer")) fixed += c.offsetWidth;
      }
      const available = el.clientWidth - pad - fixed - gap * Math.max(0, n - 1) - 2;
      const groups = ids.map((id) => fits.current.get(id)).filter((g): g is GroupFit => !!g);
      const next = fitGroups(groups, available);
      if (!sameSet(next, live.current)) setCollapsed(next);
    });
  }).current;

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(compute);
    ro.observe(el);
    compute();
    return () => {
      ro.disconnect();
      cancelAnimationFrame(frame.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled]);

  const report = useRef((id: string, f: Partial<GroupFit>) => {
    const cur = fits.current.get(id) ?? { id, width: 0, collapsedWidth: DEFAULT_COLLAPSED_W, priority: 0 };
    const next = { ...cur, ...f, id };
    const changed = next.width !== cur.width || next.collapsedWidth !== cur.collapsedWidth || next.priority !== cur.priority || next.pinned !== cur.pinned;
    fits.current.set(id, next);
    if (changed) compute();
  }).current;
  return { collapsed, report };
}

export function Ribbon(p: RibbonProps) {
  const ref = useRef<HTMLDivElement>(null);
  const ov = useRibbonOverflow(ref, p.orientation === "horizontal");
  const targets = useRef(new Map<string, HTMLElement | null>());
  const [version, setVersion] = useState(0);
  const setTarget = useRef((id: string, el: HTMLElement | null) => {
    if (targets.current.get(id) === el) return;
    targets.current.set(id, el);
    setVersion((v) => v + 1);
  }).current;
  const ctx = useMemo<RibbonCtx>(
    () => ({
      orientation: p.orientation,
      collapsed: ov.collapsed,
      target: (id) => targets.current.get(id) ?? null,
      setTarget,
      report: ov.report,
      version,
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [p.orientation, ov.collapsed, version],
  );
  return (
    <Ctx.Provider value={ctx}>
      <div
        ref={ref}
        className={`toolbar ribbon ribbon-${p.orientation}${p.className ? ` ${p.className}` : ""}`}
        role="toolbar"
        aria-label={p.label}
        aria-orientation={p.orientation === "vertical" ? "vertical" : "horizontal"}
        onContextMenu={(e) => {
          if (!p.onContextMenu || e.target !== e.currentTarget) return;
          e.preventDefault();
          p.onContextMenu(e.clientX, e.clientY);
        }}
      >
        {p.children}
      </div>
    </Ctx.Provider>
  );
}

export interface RibbonGroupProps {
  /** Stable id ("create", "modify", "arrange", "insert", "inspect", "laser"). */
  id: string;
  /** Caption under the group (and the collapsed `GROUP ▾` button's label). */
  label: string;
  /** Icon name (ui.tsx ICONS) for the collapsed button and the vertical rail. */
  icon: string;
  /** Higher stays inline longer; the lowest-priority groups collapse first. */
  priority: number;
  /** Never collapses. */
  pinned?: boolean;
  children?: ReactNode;
}

export function RibbonGroup(p: RibbonGroupProps) {
  const ctx = useContext(Ctx);
  const box = useRef<HTMLDivElement>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const orientation = ctx?.orientation ?? "horizontal";
  // Vertical rail: pinned groups inline, the rest behind a button. Horizontal: as the overflow rule says.
  const collapsed = orientation === "vertical" ? !p.pinned : orientation === "horizontal" ? !!ctx?.collapsed.has(p.id) : false;
  const report = ctx?.report;

  useEffect(() => {
    report?.(p.id, { priority: p.priority, pinned: p.pinned });
  }, [report, p.id, p.priority, p.pinned]);

  // Measure: the inline width while inline, the button's while collapsed.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el || !report || typeof ResizeObserver === "undefined") return;
    const measure = () => report(p.id, collapsed ? { collapsedWidth: el.offsetWidth } : { width: el.offsetWidth });
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [collapsed, report, p.id]);

  useEffect(() => {
    if (!collapsed) setOpen(false);
  }, [collapsed]);

  const setTarget = ctx?.setTarget;
  const body = useCallback((el: HTMLDivElement | null) => setTarget?.(p.id, el), [setTarget, p.id]);

  if (!ctx) return <>{p.children}</>;

  if (!collapsed) {
    return (
      <div ref={box} className={`ribbon-group tool-group rg-${p.id}`} data-group={p.id} role="group" aria-label={p.label}>
        <div ref={body} className="ribbon-group-body">
          {p.children}
        </div>
      </div>
    );
  }
  return (
    <div ref={box} className={`ribbon-group ribbon-group-collapsed tool-group rg-${p.id}`} data-group={p.id}>
      <button
        ref={btn}
        type="button"
        className={`tool-btn ribbon-group-btn${open ? " active" : ""}`}
        title={p.label}
        aria-label={p.label}
        aria-haspopup="true"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name={p.icon} size={20} />
        <span className="ribbon-group-btn-label">
          {p.label}
          <span className="et-caret" aria-hidden="true">
            <Icon name={orientation === "vertical" ? "chevron" : "chevronDown"} size={10} />
          </span>
        </span>
      </button>
      {open && (
        <RibbonFlyout anchor={btn.current} side={orientation === "vertical" ? "right" : "below"} label={p.label} onClose={() => setOpen(false)}>
          <div ref={body} className="ribbon-group-body ribbon-flyout-body">
            {p.children}
          </div>
        </RibbonFlyout>
      )}
    </div>
  );
}

/** A collapsed group's tools, beside or under its button; closes on an outside press, Esc, or after a tool is picked. */
function RibbonFlyout(p: { anchor: HTMLElement | null; side: "right" | "below"; label: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<CSSProperties>({ left: -10000, top: -10000, visibility: "hidden" });
  const P = useRef(p);
  P.current = p;
  useLayoutEffect(() => {
    const el = ref.current;
    const a = p.anchor?.getBoundingClientRect();
    if (!el || !a) return;
    const r = placeMenu(a, { width: el.offsetWidth, height: el.scrollHeight }, { width: window.innerWidth, height: window.innerHeight }, p.side);
    // Runs after every render: set the position only when it moved, or each render would schedule another one
    // (React error #185, the whole page gone the moment a group flyout opened on the tablet rail).
    setPos((old) => (samePos(old, r) ? old : { left: r.left, top: r.top, maxHeight: r.maxHeight }));
  });
  useEffect(() => {
    installEscape();
    const id = layers.push("menu", () => {
      P.current.onClose();
      P.current.anchor?.focus();
    });
    const inside = (n: Node) => !!ref.current?.contains(n) || isInMenu(n);
    const down = (e: PointerEvent) => {
      const t = e.target as Node;
      if (inside(t) || P.current.anchor?.contains(t)) return;
      P.current.onClose();
    };
    // A tool picked in the flyout (or in a dropdown opened from it) closes it; a button that opens a menu does not.
    const click = (e: MouseEvent) => {
      const t = e.target as Element;
      if (!inside(t)) return;
      const b = t.closest?.("button,[role=menuitem]");
      if (!b || b.getAttribute("aria-haspopup") || (b as HTMLButtonElement).disabled) return;
      window.setTimeout(() => P.current.onClose(), 0);
    };
    document.addEventListener("pointerdown", down, true);
    document.addEventListener("click", click, true);
    return () => {
      layers.remove(id);
      document.removeEventListener("pointerdown", down, true);
      document.removeEventListener("click", click, true);
    };
  }, []);
  // A group whose only tool is a dropdown (Modify, Edit, Arrange on the tablet rail): open that dropdown at once, so
  // its commands are two taps away (rail button, item) rather than three. The slots portal in a render later, hence
  // the short retry.
  useEffect(() => {
    let tries = 0;
    let timer = 0;
    const openSingle = () => {
      const el = ref.current;
      if (!el) return;
      const btns = [...el.querySelectorAll<HTMLButtonElement>("button")].filter((b) => !b.disabled);
      if (btns.length === 1 && btns[0].getAttribute("aria-haspopup") === "menu") btns[0].click();
      else if (btns.length === 0 && tries++ < 5) timer = window.setTimeout(openSingle, 16);
    };
    timer = window.setTimeout(openSingle, 0);
    return () => window.clearTimeout(timer);
  }, []);
  return createPortal(
    <div ref={ref} className="ribbon-flyout" role="group" aria-label={p.label} style={pos}>
      {p.children}
    </div>,
    document.body,
  );
}

/** Puts its children into ribbon group `group` (inline or in its flyout), at `order` among the group's tools. */
export function RibbonSlot(p: { group: string; order?: number; children?: ReactNode }) {
  const ctx = useContext(Ctx);
  if (!ctx) return <>{p.children}</>;
  const target = ctx.target(p.group);
  if (!target) return null;
  return createPortal(
    <div className="ribbon-slot" style={{ order: p.order ?? 0 }}>
      {p.children}
    </div>,
    target,
  );
}

/** The current ribbon's orientation (horizontal outside a ribbon). */
export function useRibbonOrientation(): RibbonOrientation {
  return useContext(Ctx)?.orientation ?? "horizontal";
}

// --- shared item lists ------------------------------------------------------------------------------------------
// Menus assembled from several hooks: the edit tools publish their Arrange items, the toolbar's Arrange ▾ shows them
// after its own align / distribute / group / flip entries.

type Listener = () => void;
const shared = new Map<string, unknown>();
const listeners = new Set<Listener>();

/** Publish `value` under `key` while the calling component is mounted. */
export function usePublish<T>(key: string, value: T): void {
  useEffect(() => {
    if (shared.get(key) === value) return;
    shared.set(key, value);
    for (const l of listeners) l();
  });
  useEffect(
    () => () => {
      shared.delete(key);
      for (const l of listeners) l();
    },
    [key],
  );
}

/** The value published under `key`, or undefined. */
export function usePublished<T>(key: string): T | undefined {
  return useSyncExternalStore(
    (fn) => {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
    () => shared.get(key) as T | undefined,
    () => shared.get(key) as T | undefined,
  );
}
