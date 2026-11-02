// The workspace layer's state for App.tsx in one hook: theme/scale/language on <html>, the key map (installed here,
// ahead of EditTools' key listener), and the Preferences / Keyboard shortcuts dialogs.

import { useEffect, useState } from "react";
import { setActionRunner } from "../components/commands.ts";
import { HotkeysDialog } from "./HotkeysDialog.tsx";
import { hintFor } from "./hotkeys.ts";
import { PreferencesDialog } from "./PreferencesDialog.tsx";
import { runKeymapAction, useKeymap, useKeymapConfig, type KeymapConfig } from "./useKeymap.ts";
import { useApplyWorkspace, usePrefs, useT } from "./usePrefs.ts";
import type { T } from "./i18n.ts";
import type { MenuItem } from "../components/Menu.tsx";

export type WsDialog = "prefs" | "hotkeys" | "hotkeysEdit" | null;

export interface WorkspaceUi {
  /** Fill on every render with the current workspace key handlers (see useKeymap.ts). */
  keymap: { current: KeymapConfig };
  dialog: WsDialog;
  open: (d: WsDialog) => void;
  /** The open workspace dialog, to render once. */
  dialogs: JSX.Element | null;
}

export function useWorkspaceUi(): WorkspaceUi {
  useApplyWorkspace();
  const keymap = useKeymapConfig();
  useKeymap(keymap);
  // Menus, ribbon buttons and command search run workspace actions through the same handlers as their keys, also
  // the actions that have no key bound (components/commands.ts). A missing handler or one that declines (false) falls
  // back to the key press route.
  useEffect(() => {
    setActionRunner((id) => runKeymapAction(keymap.current, id));
    return () => setActionRunner(null);
  }, [keymap]);
  const [dialog, setDialog] = useState<WsDialog>(null);
  const close = () => setDialog(null);
  const dialogs =
    dialog === "prefs" ? (
      <PreferencesDialog onClose={close} onHotkeys={() => setDialog("hotkeysEdit")} />
    ) : dialog === "hotkeys" || dialog === "hotkeysEdit" ? (
      <HotkeysDialog key={dialog} edit={dialog === "hotkeysEdit"} onClose={close} />
    ) : null;
  return { keymap, dialog, open: setDialog, dialogs };
}

/** File > Settings entries of the workspace layer as menu data (components/FileMenu.tsx reads them). */
export function workspaceFileMenuItems(open: (d: WsDialog) => void, t: T): MenuItem[] {
  return [
    { label: t("file.preferences"), actionId: "app.preferences", action: () => open("prefs") },
    { label: t("file.editHotkeys"), action: () => open("hotkeysEdit") },
  ];
}

/** File menu entries of the workspace layer: Preferences and Edit hotkeys (plain rows when rendered on their own). */
export function WorkspaceFileItems(p: { close: () => void; open: (d: WsDialog) => void }) {
  const t = useT();
  const hotkeys = usePrefs().hotkeys;
  const item = (label: string, d: WsDialog, hint?: string) => (
    <button
      role="menuitem"
      className="menu-item"
      onClick={() => {
        p.close();
        p.open(d);
      }}
    >
      <span>{label}</span>
      {hint && <span className="menu-hint">{hint}</span>}
    </button>
  );
  return (
    <>
      {item(t("file.preferences"), "prefs", hintFor("app.preferences", hotkeys))}
      {item(t("file.editHotkeys"), "hotkeysEdit")}
    </>
  );
}
