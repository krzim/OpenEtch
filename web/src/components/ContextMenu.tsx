// Right-click (and touch long-press) menu at a point: the canvas, the layer and object lists, the docks. Built on the
// Menu primitive (Menu.tsx): submenus, keyboard navigation, kept inside the window, Esc closes only the menu. A long
// menu (the canvas's) is folded into Arrange / Modify / Laser / Select submenus (contextMenu.ts).

import { useMemo } from "react";
import { Menu, type MenuItem } from "./Menu.tsx";
import { structureMenu } from "./contextMenu.ts";
import { useT } from "../workspace/usePrefs.ts";

export type { MenuItem } from "./Menu.tsx";

export function ContextMenu(p: { x: number; y: number; items: MenuItem[]; onClose: () => void; label?: string }) {
  const t = useT();
  const items = useMemo(
    () =>
      structureMenu(p.items, {
        modify: t("menu.section.modify"),
        arrange: t("menu.section.arrange"),
        select: t("menu.section.select"),
        laser: t("menu.section.laser"),
        more: t("menu.section.more"),
      }),
    [p.items, t],
  );
  const anchor = useMemo(() => ({ x: p.x, y: p.y }), [p.x, p.y]);
  return <Menu items={items} anchor={anchor} onClose={p.onClose} placement="point" label={p.label} />;
}
