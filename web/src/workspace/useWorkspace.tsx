// The workspace layer's state for App.tsx in one hook: theme/scale/language on <html>, the key map (installed here,
// ahead of EditTools' key listener), and the Preferences / Keyboard shortcuts dialogs.

import { useState } from "react";
import { HotkeysDialog } from "./HotkeysDialog.tsx";
import { hintFor } from "./hotkeys.ts";
import { PreferencesDialog } from "./PreferencesDialog.tsx";
import { useKeymap, useKeymapConfig, type KeymapConfig } from "./useKeymap.ts";
import { useApplyWorkspace, usePrefs, useT } from "./usePrefs.ts";

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

/** File menu entries of the workspace layer: Preferences and Edit hotkeys. */
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
