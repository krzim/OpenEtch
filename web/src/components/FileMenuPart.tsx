// A layer's contribution to the File menu as data: FileMenu.tsx reads `section` and `items` and files them into its
// submenus (fileMenuModel.ts). Rendered on its own (outside the File menu) it shows the items as plain menu rows.

import type { MenuItem } from "./Menu.tsx";
import type { FileSection } from "./fileMenuModel.ts";

export function FileMenuPart(p: { section: FileSection; items: MenuItem[]; close?: () => void }) {
  return (
    <>
      {p.items.map((it, i) =>
        it.sep ? (
          <div key={i} className="menu-sep" role="separator" />
        ) : it.content ? (
          <div key={i}>{it.content(p.close ?? (() => {}))}</div>
        ) : (
          <button
            key={i}
            role="menuitem"
            className="menu-item"
            disabled={it.disabled || !it.action}
            title={it.title}
            onClick={() => {
              p.close?.();
              it.action?.();
            }}
          >
            <span>{it.label}</span>
            {it.hint && <span className="menu-hint">{it.hint}</span>}
          </button>
        ),
      )}
    </>
  );
}
