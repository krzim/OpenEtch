// The one menu primitive (docs/ux/ui-plan-2026-11.md P2 "Menu"): context menus, toolbar dropdowns, the File menu and
// every submenu. Submenus; roving focus with arrows, Home/End and type-ahead (menuModel.ts); focus on the first item
// when it opens and back on the opener when it closes; flip-and-clamp placement with a max-height so it scrolls
// inside the window; 32 px rows on a fine pointer and 44 on a coarse one (menu.css); Esc closes only the menu (or
// only the submenu) through the shared Esc stack (dialogStack.ts), so the selection behind it stays.
//
// A press that opens a menu (a touch long-press, a Mac right-click on mouse-down) never also picks an item: rows act
// only after a press that started while the menu was open, or from the keyboard.

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { hintFor } from "../workspace/hotkeys.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { useLongPress, type LongPressHandlers } from "../touch/useLongPress.ts";
import { installEscape, layers } from "./dialogStack.ts";
import { formatChord } from "./formatChord.ts";
import { menuNav, placeMenu, typeAhead, type NavItem, type Placement } from "./menuModel.ts";
import { Icon } from "./ui.tsx";
import "./menu.css";

/** A menu entry: today's ContextMenu item, plus submenus, a hotkey action id, a danger style and a check mark. */
export type MenuItem =
  | { sep: true }
  | {
      label: string;
      hint?: string;
      disabled?: boolean;
      /** Runs the item (and closes the menu). Optional for an item that only opens a submenu. */
      action?: () => void;
      sep?: false;
      /** Items of a submenu that opens beside this one. */
      submenu?: MenuItem[];
      /** The hotkeys.ts action this item runs: its chord is shown as the hint, and command search finds it. */
      actionId?: string;
      /** Destructive (Delete, Discard): drawn in the danger colour. */
      danger?: boolean;
      /** A toggle's state: true or false draws a check box, undefined none. */
      checked?: boolean;
      /** Icon name (ui.tsx ICONS). */
      icon?: string;
      /** Hover text (a disabled item's reason). */
      title?: string;
      /** Left out on the phone (workspace/desktopOnly.ts): More > Desktop features offers it there. */
      desktopOnly?: boolean;
      /**
       * Where a long context menu files this item ("arrange", "modify", "laser", "select", ...; see contextMenu.ts).
       * Ignored by short menus.
       */
      section?: string;
      /**
       * Custom rows instead of a button (Open Recent's live project list): given the close function. Buttons in it
       * with role="menuitem" join the keyboard navigation. `label` still names it for command search.
       */
      content?: (close: () => void) => ReactNode;
    };

/** A point (context menus at the pointer) or an element's rectangle (dropdowns under their button). */
export type MenuAnchor = { x: number; y: number } | DOMRect;

/** Where the menu opens relative to the anchor; it flips and clamps to stay inside the window. */
export type MenuPlacement = "below" | "above" | "right" | "left" | "point";

export interface MenuProps {
  items: MenuItem[];
  anchor: MenuAnchor;
  onClose: () => void;
  /** Default: "point" for a point anchor, "below" for a rectangle. */
  placement?: MenuPlacement;
  /** Accessible name of the menu. */
  label?: string;
  /** Extra class on the menu panel. */
  className?: string;
  /** The button that opened it: presses on it are not "outside" (it toggles the menu itself) and focus returns to it. */
  owner?: HTMLElement | null;
  /** Focus the last item instead of the first (ArrowUp on the opener). */
  focusLast?: boolean;
  /** A dropdown lines up its right edge with the button's ("end") instead of its left. */
  align?: "start" | "end";
}

function isPoint(a: MenuAnchor): boolean {
  return !("width" in a && "height" in a);
}

/** Every panel of every open menu tree, for "is this press outside?". */
const openPanels = new Set<HTMLElement>();

/** Whether `node` is inside an open menu (a flyout that opened it should not count a press there as outside). */
export function isInMenu(node: Node | null): boolean {
  if (!node) return false;
  for (const el of openPanels) if (el.contains(node)) return true;
  return false;
}

const ROW_SEL = '[role="menuitem"],[role="menuitemcheckbox"],[role="menuitemradio"]';

function rowsOf(panel: HTMLElement): HTMLElement[] {
  // Only this panel's rows, not a nested panel's (submenus are portalled, so they are never inside).
  return [...panel.querySelectorAll<HTMLElement>(ROW_SEL)];
}

function navItemOf(el: HTMLElement): NavItem {
  return {
    label: el.dataset.label ?? el.textContent ?? "",
    disabled: (el as HTMLButtonElement).disabled || el.getAttribute("aria-disabled") === "true",
    submenu: el.getAttribute("aria-haspopup") === "menu",
  };
}

/** The hint shown for an item: its own text, or its action's current chord; formatted for the platform. */
export function itemHint(it: { hint?: string; actionId?: string }, hotkeys: Readonly<Record<string, string[]>>): string {
  if (it.hint !== undefined) return formatChord(it.hint);
  return it.actionId ? formatChord(hintFor(it.actionId, hotkeys)) : "";
}

interface PanelProps {
  items: MenuItem[];
  anchor: MenuAnchor;
  placement: Placement;
  align?: "start" | "end";
  level: number;
  label?: string;
  className?: string;
  owner?: HTMLElement | null;
  /** Close the whole tree (after an item ran, on an outside press, Tab, blur). */
  closeAll: (restore: boolean) => void;
  /** Close this panel only (a submenu's Left / Esc); the top panel closes the tree. */
  closeSelf: () => void;
  /** Move focus into the panel when it opens (the top panel and keyboard-opened submenus). */
  autoFocus: "first" | "last" | "none";
  /** Set once a press has started while the tree was open (rows ignore the press that opened it). */
  armed: { current: boolean };
}

function MenuPanel(p: PanelProps) {
  const t = useT();
  const hotkeys = usePrefs().hotkeys;
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number; maxHeight?: number } | null>(null);
  const [sub, setSub] = useState<{ index: number; rect: DOMRect; keyboard: boolean } | null>(null);
  const hoverTimer = useRef<number | undefined>(undefined);
  const typed = useRef({ text: "", at: 0 });
  const P = useRef(p);
  P.current = p;

  // Place it (before paint): its natural size, then flip and clamp into the window.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const a = p.anchor;
    const frame = el.offsetHeight - el.clientHeight;
    const size = { width: el.offsetWidth, height: el.scrollHeight + frame };
    const anchor = isPoint(a) ? { x: a.x, y: a.y } : { left: (a as DOMRect).left, top: (a as DOMRect).top, right: (a as DOMRect).right, bottom: (a as DOMRect).bottom };
    const r = placeMenu(anchor, size, { width: window.innerWidth, height: window.innerHeight }, p.placement, 4, p.align);
    setPos({ left: r.left, top: r.top, maxHeight: r.maxHeight });
    // Re-place when the content changes size (Open Recent's list arriving).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.anchor, p.placement, p.items]);

  // Register for outside-press checks, and on the Esc stack.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    openPanels.add(el);
    installEscape();
    const id = layers.push("menu", () => P.current.closeSelf());
    return () => {
      openPanels.delete(el);
      layers.remove(id);
    };
  }, []);

  // Focus on open.
  useEffect(() => {
    const el = ref.current;
    if (!el || !pos || p.autoFocus === "none") return;
    if (el.contains(document.activeElement)) return;
    const rows = rowsOf(el).filter((r) => !navItemOf(r).disabled);
    const target = p.autoFocus === "last" ? rows[rows.length - 1] : rows[0];
    (target ?? el).focus({ preventScroll: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pos !== null]);

  useEffect(() => () => window.clearTimeout(hoverTimer.current), []);

  const openSub = (index: number, row: HTMLElement, keyboard: boolean) => {
    window.clearTimeout(hoverTimer.current);
    setSub({ index, rect: row.getBoundingClientRect(), keyboard });
  };

  const focusRow = (i: number) => {
    const el = ref.current;
    if (!el) return;
    rowsOf(el)[i]?.focus({ preventScroll: false });
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const el = ref.current;
    if (!el) return;
    // Keys inside a nested panel's React tree bubble here too; that panel handles them.
    if (!el.contains(e.target as Node) && e.target !== el) return;
    const rows = rowsOf(el);
    const nav = rows.map(navItemOf);
    const active = rows.indexOf(document.activeElement as HTMLElement);
    if (e.key.length === 1 && e.key !== " " && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const now = Date.now();
      const text = now - typed.current.at < 600 ? typed.current.text + e.key : e.key;
      typed.current = { text, at: now };
      const i = typeAhead(nav, active, text);
      if (i >= 0) focusRow(i);
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    const act = menuNav(nav, active, e.key, { level: p.level, rtl: getComputedStyle(el).direction === "rtl" });
    switch (act.kind) {
      case "none":
        return;
      case "move":
        focusRow(act.index);
        break;
      case "openSubmenu": {
        const row = rows[act.index];
        const i = Number(row.dataset.index);
        if (Number.isFinite(i)) openSub(i, row, true);
        break;
      }
      case "activate":
        rows[act.index].click();
        break;
      case "closeSubmenu":
      case "close":
        p.closeSelf();
        break;
      case "closeAll":
        // Tab: close and let the focus move on from the opener.
        p.closeAll(true);
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  const close = () => p.closeAll(true);
  let index = -1;
  const style: CSSProperties = pos ? { left: pos.left, top: pos.top, maxHeight: pos.maxHeight } : { left: -10000, top: -10000, visibility: "hidden" };
  const subItem = sub ? p.items[sub.index] : null;

  return (
    <>
      {createPortal(
        <div
          ref={ref}
          className={`menu ctx-menu oe-menu${p.level ? " oe-submenu" : ""}${p.className ? ` ${p.className}` : ""}`}
          role="menu"
          aria-label={p.label}
          aria-orientation="vertical"
          tabIndex={-1}
          style={style}
          onKeyDown={onKeyDown}
          onContextMenu={(e) => e.preventDefault()}
        >
          {p.items.map((it, i) => {
            if (it.sep) {
              // No separator first, last or twice in a row.
              const prev = p.items[i - 1];
              if (i === 0 || i === p.items.length - 1 || !prev || prev.sep) return null;
              return <div key={i} className="menu-sep" role="separator" />;
            }
            if (it.content) return <div key={i} className="oe-menu-content" role="none">{it.content(close)}</div>;
            index++;
            const hasSub = !!it.submenu?.length;
            const disabled = !!it.disabled || (!it.action && !hasSub);
            const hint = itemHint(it, hotkeys);
            const open = sub?.index === i;
            return (
              <button
                key={i}
                type="button"
                role={it.checked === undefined ? "menuitem" : "menuitemcheckbox"}
                aria-checked={it.checked === undefined ? undefined : it.checked}
                aria-haspopup={hasSub ? "menu" : undefined}
                aria-expanded={hasSub ? open : undefined}
                className={`menu-item oe-menu-item${it.danger ? " danger" : ""}${open ? " sub-open" : ""}`}
                tabIndex={-1}
                disabled={disabled}
                title={it.title}
                data-index={i}
                data-label={it.label}
                onPointerMove={(e) => {
                  if (e.pointerType === "touch") return;
                  const row = e.currentTarget;
                  if (document.activeElement !== row && !disabled) row.focus({ preventScroll: true });
                  if (hasSub && !open) {
                    window.clearTimeout(hoverTimer.current);
                    hoverTimer.current = window.setTimeout(() => openSub(i, row, false), 120);
                  } else if (!hasSub && sub) {
                    window.clearTimeout(hoverTimer.current);
                    hoverTimer.current = window.setTimeout(() => setSub(null), 200);
                  }
                }}
                onPointerLeave={() => window.clearTimeout(hoverTimer.current)}
                onClick={(e: ReactMouseEvent<HTMLButtonElement>) => {
                  // The press that opened the menu (long-press, Mac right-click) must not also pick a row.
                  if (e.detail !== 0 && !p.armed.current) return;
                  if (hasSub) {
                    if (open && e.detail !== 0) setSub(null);
                    else openSub(i, e.currentTarget, e.detail === 0);
                    return;
                  }
                  // The keyboard gets its focus back on the opener; a pointer pick leaves it free, so the canvas
                  // letter keys work at once (workspace/keyFocus.ts), as after a LightBurn menu.
                  p.closeAll(e.detail === 0);
                  it.action?.();
                }}
              >
                <span className="oe-menu-check" aria-hidden="true">
                  {it.checked ? "✓" : ""}
                </span>
                {it.icon ? <Icon name={it.icon} size={16} /> : null}
                <span className="oe-menu-label">{it.label}</span>
                {hint && <span className="menu-hint">{hint}</span>}
                {hasSub && (
                  <span className="oe-menu-chevron" aria-hidden="true">
                    <Icon name="chevron" size={14} />
                  </span>
                )}
              </button>
            );
          })}
          {index < 0 && !p.items.some((x) => !x.sep && x.content) && <div className="menu-item muted oe-menu-empty">{t("menu.empty")}</div>}
        </div>,
        document.body,
      )}
      {sub && subItem && !subItem.sep && subItem.submenu && (
        <MenuPanel
          items={subItem.submenu}
          anchor={sub.rect}
          placement="right"
          level={p.level + 1}
          label={subItem.label}
          owner={null}
          closeAll={p.closeAll}
          closeSelf={() => {
            const i = sub.index;
            setSub(null);
            // Back to the row that opened it.
            const row = ref.current?.querySelector<HTMLElement>(`[data-index="${i}"]`);
            row?.focus({ preventScroll: true });
          }}
          autoFocus={sub.keyboard ? "first" : "none"}
          armed={p.armed}
        />
      )}
    </>
  );
}

/** A menu at a point or under / beside an element, closed by an item, an outside press, Esc, Tab or leaving the window. */
export function Menu(p: MenuProps) {
  const placement: Placement = p.placement ?? (isPoint(p.anchor) ? "point" : "below");
  const P = useRef(p);
  P.current = p;
  const armed = useRef(false);
  // Where the focus was before the menu took it, to give it back.
  const opener = useRef<Element | null>(typeof document === "undefined" ? null : (p.owner ?? document.activeElement));
  const closed = useRef(false);

  const closeAll = (restore: boolean) => {
    if (closed.current) return;
    closed.current = true;
    if (restore) {
      const o = opener.current;
      if (o instanceof HTMLElement && o.isConnected && o !== document.body) o.focus({ preventScroll: true });
    }
    P.current.onClose();
  };

  useEffect(() => {
    const down = (e: PointerEvent) => {
      const target = e.target as Node;
      for (const el of openPanels) if (el.contains(target)) return void (armed.current = true);
      if (P.current.owner?.contains(target)) return;
      closeAll(false);
    };
    // Not on resize: phones resize the window when the address bar or the keyboard moves.
    const blur = () => closeAll(false);
    document.addEventListener("pointerdown", down, true);
    window.addEventListener("blur", blur);
    return () => {
      document.removeEventListener("pointerdown", down, true);
      window.removeEventListener("blur", blur);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <MenuPanel
      items={p.items}
      anchor={p.anchor}
      placement={placement}
      align={p.align}
      level={0}
      label={p.label}
      className={p.className}
      owner={p.owner}
      closeAll={closeAll}
      closeSelf={() => closeAll(true)}
      autoFocus={p.focusLast ? "last" : "first"}
      armed={armed}
    />
  );
}

export interface MenuButtonProps {
  label: string;
  /** Icon name (ui.tsx ICONS). */
  icon?: string;
  items: MenuItem[];
  title?: string;
  className?: string;
  disabled?: boolean;
  /** Icon size in px (default 20). */
  iconSize?: number;
  /** Show a ▾ after the label (toolbar dropdowns). */
  caret?: boolean;
  /** Where the menu opens (default below the button). */
  placement?: MenuPlacement;
  /** Custom button content instead of the icon and label (the label still names it). */
  children?: ReactNode;
  /** Accessible name when it differs from `label` (icon-only buttons). */
  ariaLabel?: string;
  /** Line the menu up with the button's right edge. */
  align?: "start" | "end";
}

/** A button that opens a menu under itself (toolbar dropdowns, the top bar's overflow); a press on it toggles. */
export function MenuButton(p: MenuButtonProps) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState<{ rect: DOMRect; last: boolean } | null>(null);
  const show = (last: boolean) => {
    const r = ref.current?.getBoundingClientRect();
    if (r) setOpen({ rect: r, last });
  };
  return (
    <>
      <button
        ref={ref}
        type="button"
        className={`${p.className ?? "tool-btn"}${open ? " active open" : ""}`}
        title={p.title ?? p.label}
        aria-label={p.ariaLabel}
        aria-haspopup="menu"
        aria-expanded={!!open}
        disabled={p.disabled}
        onClick={() => (open ? setOpen(null) : show(false))}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            show(e.key === "ArrowUp");
          }
        }}
      >
        {p.children ?? (
          <>
            {p.icon && <Icon name={p.icon} size={p.iconSize ?? 20} />}
            <span className="oe-mb-label">
              {p.label}
              {p.caret && (
                <span className="et-caret" aria-hidden="true">
                  <Icon name="chevronDown" size={10} />
                </span>
              )}
            </span>
          </>
        )}
      </button>
      {open && (
        <Menu
          items={p.items}
          anchor={open.rect}
          // On the tablet's vertical tool rail a dropdown opens beside the rail, not down over the rail's own buttons
          // (the Frame options menu covered Start there).
          placement={p.placement ?? (ref.current?.closest(".ribbon-vertical") ? "right" : undefined)}
          align={p.align}
          label={p.label}
          owner={ref.current}
          focusLast={open.last}
          onClose={() => setOpen(null)}
        />
      )}
    </>
  );
}

/**
 * Right-click and touch long-press both open a context menu: spread `handlers` on the element. `onOpen` gets the
 * client position and the element.
 */
export function useContextMenuTrigger(onOpen: (x: number, y: number, target: Element) => void): LongPressHandlers {
  const long = useLongPress(onOpen);
  return {
    ...long,
    onContextMenu: (e) => {
      long.onContextMenu?.(e);
      if (e.defaultPrevented) return;
      e.preventDefault();
      onOpen(e.clientX, e.clientY, e.currentTarget);
    },
  };
}
