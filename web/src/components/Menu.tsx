// STAGE A STUB (owner: P2). The one menu primitive of docs/ux/ui-plan-2026-11.md P2: submenus, roving focus, arrows,
// Home/End and type-ahead, flip-and-clamp placement, Esc closes only the menu. The signatures below are final.
//
// Until P2 lands, `Menu` falls back to today's ContextMenu (flat list; a submenu's items are listed after its label)
// and `MenuButton` to a plain button that opens it, so code that adopts them early keeps working.

import { useRef, useState } from "react";
import { ContextMenu, type MenuItem as FlatMenuItem } from "./ContextMenu.tsx";
import { Icon } from "./ui.tsx";

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
}

function isRect(a: MenuAnchor): a is DOMRect {
  return "width" in a && "height" in a;
}

function flatten(items: MenuItem[], depth = 0): FlatMenuItem[] {
  const out: FlatMenuItem[] = [];
  const pad = " ".repeat(depth);
  for (const it of items) {
    if (it.sep) {
      out.push({ sep: true });
      continue;
    }
    const mark = it.checked === undefined ? "" : it.checked ? "✓ " : " ";
    out.push({ label: `${pad}${mark}${it.label}`, hint: it.hint, disabled: it.disabled || !it.action, action: it.action ?? (() => {}) });
    if (it.submenu) out.push(...flatten(it.submenu, depth + 1));
  }
  return out;
}

export function Menu(p: MenuProps) {
  const a = p.anchor;
  const at = isRect(a) ? (p.placement === "right" ? { x: a.right, y: a.top } : { x: a.left, y: a.bottom + 2 }) : { x: a.x, y: a.y };
  return <ContextMenu x={at.x} y={at.y} items={flatten(p.items)} onClose={p.onClose} />;
}

export interface MenuButtonProps {
  label: string;
  /** Icon name (ui.tsx ICONS). */
  icon?: string;
  items: MenuItem[];
  title?: string;
  className?: string;
  disabled?: boolean;
}

/** A button that opens a menu under itself (toolbar dropdowns, the top bar's overflow). */
export function MenuButton(p: MenuButtonProps) {
  const ref = useRef<HTMLButtonElement>(null);
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  // The press on this button that closed the menu (an outside press) must not reopen it.
  const closedAt = useRef(0);
  return (
    <>
      <button
        ref={ref}
        type="button"
        className={p.className ?? "tool-btn"}
        title={p.title ?? p.label}
        aria-haspopup="menu"
        aria-expanded={!!anchor}
        disabled={p.disabled}
        onClick={() => {
          if (Date.now() - closedAt.current < 250) return;
          setAnchor(ref.current?.getBoundingClientRect() ?? null);
        }}
      >
        {p.icon && <Icon name={p.icon} />}
        <span>{p.label}</span>
      </button>
      {anchor && (
        <Menu
          items={p.items}
          anchor={anchor}
          onClose={() => {
            closedAt.current = Date.now();
            setAnchor(null);
          }}
        />
      )}
    </>
  );
}
