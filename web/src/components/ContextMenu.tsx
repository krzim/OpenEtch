// Right-click menu for the canvas: edit, group, align and flip the selection. Closes on a click elsewhere, Esc, or
// after an item runs. Kept inside the window.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

export type MenuItem = { sep: true } | { label: string; hint?: string; disabled?: boolean; action: () => void; sep?: false };

export function ContextMenu(p: { x: number; y: number; items: MenuItem[]; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: p.x, top: p.y });
  const { onClose } = p;

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({
      left: Math.max(4, Math.min(p.x, window.innerWidth - r.width - 4)),
      top: Math.max(4, Math.min(p.y, window.innerHeight - r.height - 4)),
    });
  }, [p.x, p.y]);

  useEffect(() => {
    const down = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("pointerdown", down, true);
    window.addEventListener("keydown", key);
    window.addEventListener("blur", onClose);
    return () => {
      document.removeEventListener("pointerdown", down, true);
      window.removeEventListener("keydown", key);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  return createPortal(
    <div ref={ref} className="menu ctx-menu" role="menu" style={pos} onContextMenu={(e) => e.preventDefault()}>
      {p.items.map((it, i) =>
        it.sep ? (
          <div key={i} className="menu-sep" role="separator" />
        ) : (
          <button
            key={i}
            role="menuitem"
            className="menu-item"
            disabled={it.disabled}
            onClick={() => {
              onClose();
              it.action();
            }}
          >
            <span>{it.label}</span>
            {it.hint && <span className="menu-hint">{it.hint}</span>}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}
