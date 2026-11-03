// The File menu's structure (docs/ux/ui-plan-2026-11.md P2 "File menu restructure"): about a dozen top-level rows,
// the rest in submenus, every action once. Pure (the items carry their own actions); FileMenu.tsx gathers the parts
// from the files, libs and workspace layers and renders the result with the Menu primitive. Tested in
// fileMenu.test.ts.

import type { MenuItem } from "./Menu.tsx";

/** Where a contributed File menu entry goes. */
export type FileSection = "new" | "export" | "project" | "lightburn" | "laser" | "settings" | "top";

export const FILE_MENU_MAX_TOP = 14;

export interface FileMenuInput {
  busy: boolean;
  newDocument: () => void;
  open: () => void;
  /** Open Recent's live list (rendered inside its submenu). */
  recent?: MenuItem;
  save: () => void;
  saveAs: () => void;
  importFiles: () => void;
  importLightBurn: () => void;
  exportLightBurn: () => void;
  downloadOpenetch: () => void;
  saveGcode?: () => void;
  /** Save RD file: only while the active device is a Ruida gantry. */
  saveRd?: () => void;
  devices?: () => void;
  /** Entries other layers contribute, by section. */
  parts: Partial<Record<FileSection, MenuItem[]>>;
  /** Labels (translated). */
  labels: {
    new: string;
    open: string;
    openRecent: string;
    save: string;
    saveAs: string;
    import: string;
    export: string;
    exportLightBurn: string;
    saveGcode: string;
    saveRd: string;
    downloadOpenetch: string;
    project: string;
    lightburn: string;
    importLightBurn: string;
    laser: string;
    devices: string;
    settings: string;
  };
}

function sub(label: string, items: MenuItem[], icon?: string): MenuItem | null {
  const real = items.filter((x) => !x.sep);
  if (!real.length) return null;
  return { label, icon, submenu: items };
}

/** The File menu: New, Open, Open Recent ▸, Save, Save As, Import, Export ▸, Project ▸, LightBurn ▸, Laser ▸, Settings. */
export function buildFileMenu(i: FileMenuInput): MenuItem[] {
  const L = i.labels;
  const d = i.busy;
  const parts = (s: FileSection) => i.parts[s] ?? [];
  const exportItems: MenuItem[] = [
    ...parts("export"),
    ...(i.saveGcode ? [{ label: L.saveGcode, hint: ".gcode", disabled: d, action: i.saveGcode } as MenuItem] : []),
    ...(i.saveRd ? [{ label: L.saveRd, hint: ".rd", disabled: d, action: i.saveRd } as MenuItem] : []),
    { sep: true },
    { label: L.downloadOpenetch, hint: ".openetch", disabled: d, action: i.downloadOpenetch },
  ];
  const lightburn: MenuItem[] = [
    { label: L.importLightBurn, hint: ".lbrn2", disabled: d, action: i.importLightBurn },
    { label: L.exportLightBurn, hint: ".lbrn2", disabled: d, action: i.exportLightBurn },
    ...(parts("lightburn").length ? [{ sep: true } as MenuItem, ...parts("lightburn")] : []),
  ];
  const laser: MenuItem[] = [...(i.devices ? [{ label: L.devices, actionId: "window.devices", action: i.devices } as MenuItem, { sep: true } as MenuItem] : []), ...parts("laser")];
  const settings = parts("settings");
  const settingsRow: MenuItem | null = settings.length === 1 ? settings[0] : sub(L.settings, settings, "modify");
  const rows: (MenuItem | null)[] = [
    { label: L.new, actionId: "file.new", disabled: d, action: i.newDocument },
    // New from example (templates/useTemplateUi.tsx), right under New.
    ...parts("new"),
    { label: L.open, actionId: "file.open", disabled: d, action: i.open },
    i.recent ? { label: L.openRecent, icon: "history", submenu: [i.recent] } : null,
    { sep: true },
    { label: L.save, icon: "save", actionId: "file.save", disabled: d, action: i.save },
    { label: L.saveAs, actionId: "file.saveAs", disabled: d, action: i.saveAs },
    { sep: true },
    { label: L.import, icon: "import", actionId: "file.import", disabled: d, action: i.importFiles },
    sub(L.export, exportItems, "upload"),
    sub(L.project, parts("project")),
    sub(L.lightburn, lightburn),
    sub(L.laser, laser, "laser"),
    { sep: true },
    ...parts("top"),
    settingsRow,
  ];
  return rows.filter((x): x is MenuItem => !!x);
}

/** Every action id in a menu tree (for the no-duplicates check). */
export function actionIdsOf(items: readonly MenuItem[]): string[] {
  return items.flatMap((x) => (x.sep ? [] : [...(x.actionId ? [x.actionId] : []), ...actionIdsOf(x.submenu ?? [])]));
}

/** Every label path in a menu tree ("Export › Print…"). */
export function labelPaths(items: readonly MenuItem[], prefix = ""): string[] {
  return items.flatMap((x) => (x.sep ? [] : x.submenu ? labelPaths(x.submenu, `${prefix}${x.label} › `) : [`${prefix}${x.label}`]));
}

/** Top-level rows (separators not counted). */
export function topLevelCount(items: readonly MenuItem[]): number {
  return items.filter((x) => !x.sep).length;
}
