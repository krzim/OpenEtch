// The menu of one tool window (its header's ⋯ button or a right-click on its title): dock it left, right or at the
// bottom, float it, put it in another window's frame as a tab or take it out, move it up or down, hide it. Every entry
// is a layout.ts change.

import type { MenuItem } from "../components/ContextMenu.tsx";
import type { T } from "./i18n.ts";
import { areaUnits, dockTo, getLayout, moveInArea, placeOf, setVisible, tabWith, untab, updateLayout, type DockArea } from "./layout.ts";
import { mountedWindows } from "./windows.ts";

export function windowMenuItems(id: string, t: T): MenuItem[] {
  const l = getLayout();
  const mounted = mountedWindows();
  const known = [...mounted.keys()];
  const w = placeOf(l, id);
  const dock = (area: DockArea, label: string): MenuItem => ({
    label,
    disabled: w.area === area && !w.group,
    action: () => updateLayout((x) => dockTo(x, id, area, known)),
  });
  const units = areaUnits(l, w.area, known);
  const i = units.findIndex((u) => u.members.includes(id));
  const others = known.filter((k) => k !== id && placeOf(l, k).visible && !(w.group && placeOf(l, k).group === w.group));
  const items: MenuItem[] = [
    dock("left", t("win.dockLeft")),
    dock("right", t("win.dockRight")),
    dock("bottom", t("win.dockBottom")),
    dock("float", t("win.float")),
    { sep: true },
    ...others.map((o) => ({ label: t("win.tabWith", { name: mounted.get(o) ?? o }), action: () => updateLayout((x) => tabWith(x, id, o)) })),
  ];
  if (w.group) items.push({ label: t("win.untab"), action: () => updateLayout((x) => untab(x, id)) });
  if (w.area !== "float") {
    items.push(
      { sep: true },
      { label: t("win.moveUp"), disabled: i <= 0, action: () => updateLayout((x) => moveInArea(x, id, -1, known)) },
      { label: t("win.moveDown"), disabled: i < 0 || i >= units.length - 1, action: () => updateLayout((x) => moveInArea(x, id, 1, known)) },
    );
  }
  items.push({ sep: true }, { label: t("win.hide"), action: () => updateLayout((x) => setVisible(x, id, false)) });
  return items;
}
