// Top bar: the File menu (far left), app name, project name with an unsaved-changes dot, Fusion-style workspace
// switcher, undo/redo, the connection indicator (WebSocket state, engine reachability, and any command ids the UI needs
// that the engine lacks), the machine status chip and the always-visible STOP.
//
// STOP sits in its own slot (`.topbar-stop`, never shrinks, never in an overflow menu or a sheet) above every modal,
// menu and toast (`--z-stop`, docs/ux/ui-plan-2026-11.md 1.1). What shrinks on a small window is the brand, the project
// name and the document tabs; the workspace switcher keeps its width (two 44 px icon segments on tablets and phones).
// On a phone the bar is: menu, document name, status dot, STOP.

import type { ReactNode } from "react";
import type { LinkState } from "../api/useEngine.ts";
import type { MachineStatus } from "../api/types.ts";
import { StatusChip } from "../laser/StatusChip.tsx";
import { FullscreenButton } from "../app/FullscreenButton.tsx";
import { ProjectIndicator } from "./FileMenu.tsx";
import { Icon } from "./ui.tsx";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { ShellIcon } from "../workspace/shellIcons.tsx";
import { usePrefs, useT } from "../workspace/usePrefs.ts";

export type Workspace = "design" | "laser";

export interface TopBarProps {
  /** The File menu button. */
  fileMenu: ReactNode;
  workspace: Workspace;
  onWorkspace: (w: Workspace) => void;
  onUndo: () => void;
  onRedo: () => void;
  onStop: () => void;
  link: LinkState;
  httpError: string | null;
  machine: MachineStatus | null;
  missingCommands: string[];
  docName: string | null;
  /** The saved project the document is, or null. */
  projectName: string | null;
  /** Unsaved changes; null when the engine reports no project state. */
  dirty: boolean | null;
  pendingRuns: number;
  /** Extra controls before undo/redo (Window and Help menus, theme switch, Preferences, or the ⋯ overflow). */
  extra?: ReactNode;
  /** After the project name: the document tabs' "+" while only one document is open (the tab row is hidden then). */
  docExtra?: ReactNode;
  /** Phone: the ≡ button (opens the More sheet). */
  onMenu?: () => void;
}

export function TopBar(p: TopBarProps) {
  const t = useT();
  const { layout } = useLayoutMode();
  const phone = layout === "phone";
  const iconTabs = layout === "tablet" || phone;
  const engineOk = p.link === "open" && !p.httpError;
  const linkText = p.link === "open" ? (p.httpError ? t("top.engineError") : t("top.live")) : p.link === "connecting" ? t("top.connecting") : t("top.offline");
  const beginnerMode = usePrefs().beginnerMode;
  const machineText = !p.machine ? "" : p.machine.connected ? (p.machine.sim ? t("top.simulator") : t("top.laserConnected")) : t("top.noLaser");
  const title = [
    t("top.events", { state: p.link }),
    linkText + (machineText ? ` · ${machineText}` : ""),
    p.httpError ? t("top.lastError", { error: p.httpError }) : "",
    p.missingCommands.length ? t("top.missing", { list: p.missingCommands.join(", ") }) : "",
  ]
    .filter(Boolean)
    .join("\n");
  const conn = (
    <div className={`conn ${engineOk ? "ok" : p.link === "connecting" ? "wait" : "bad"}`} title={title} role="status" aria-label={title}>
      <span className="dot" />
      <span className="conn-text">
        {linkText}
        {machineText && ` · ${machineText}`}
      </span>
      {p.missingCommands.length > 0 && <span className="warn-chip">{t("top.cmdMissing", { n: p.missingCommands.length })}</span>}
    </div>
  );
  return (
    <header className="topbar">
      <div className="topbar-start">
        {phone ? (
          p.onMenu && (
            <button className="icon-btn topbar-menu" title={t("shell.menu")} aria-label={t("shell.menu")} onClick={p.onMenu}>
              <ShellIcon name="menu" />
            </button>
          )
        ) : (
          p.fileMenu
        )}
        <div className="brand">
          <span className="brand-mark" aria-hidden="true" />
          <span className="brand-name">OpenEtch</span>
          <ProjectIndicator name={p.projectName} docName={p.docName} dirty={p.dirty} />
          {!phone && p.docExtra}
          {beginnerMode && !phone && (
            <span className="pill beginner-badge" title={t("prefs.beginnerBadgeTitle")}>
              {t("prefs.beginnerBadge")}
            </span>
          )}
        </div>
      </div>
      {!phone && (
        <nav className="workspaces" role="tablist" aria-label={t("top.workspace")}>
          {(["design", "laser"] as const).map((w) => {
            const label = w === "design" ? t("top.design") : t("top.laser");
            return (
              <button
                key={w}
                role="tab"
                aria-selected={p.workspace === w}
                aria-label={iconTabs ? label : undefined}
                title={iconTabs ? label : undefined}
                className={p.workspace === w ? "active" : ""}
                onClick={() => p.onWorkspace(w)}
              >
                {iconTabs && <ShellIcon name={w === "design" ? "pencil" : "zap"} />}
                <span className="ws-label">{label}</span>
                {w === "laser" && p.pendingRuns > 0 && <span className="pill">{p.pendingRuns}</span>}
              </button>
            );
          })}
        </nav>
      )}
      <div className="topbar-actions">
        {!phone && p.extra}
        {/* P4's toggle (app/FullscreenButton.tsx): follows F11 and shows the one-time tip after Esc left full screen. */}
        {!phone && <FullscreenButton className="icon-btn fullscreen-btn" />}
        {!phone && (
          <>
            <button className="icon-btn" title={t("top.undoTitle")} aria-label={t("top.undo")} onClick={p.onUndo}>
              <Icon name="undo" />
            </button>
            <button className="icon-btn" title={t("top.redoTitle")} aria-label={t("top.redo")} onClick={p.onRedo}>
              <Icon name="redo" />
            </button>
          </>
        )}
        {conn}
      </div>
      <div className="topbar-stop">
        <StatusChip compact={phone} />
        <button className="btn btn-stop" onClick={p.onStop} title={t("top.stopTitle")}>
          <Icon name="stop" /> {t("top.stop")}
        </button>
      </div>
    </header>
  );
}
