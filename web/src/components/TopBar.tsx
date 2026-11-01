// Top bar: the File menu (far left), app name, project name with an unsaved-changes dot, Fusion-style workspace
// switcher, undo/redo, the always-visible Stop button, and the connection indicator (WebSocket state, engine
// reachability, and any command ids the UI needs that the engine lacks).

import type { ReactNode } from "react";
import type { LinkState } from "../api/useEngine.ts";
import type { MachineStatus } from "../api/types.ts";
import { ProjectIndicator } from "./FileMenu.tsx";
import { Icon } from "./ui.tsx";
import { useT } from "../workspace/usePrefs.ts";

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
  /** Extra controls before undo/redo (theme switch, Preferences, Help menu: workspace/HelpMenu.tsx). */
  extra?: ReactNode;
}

export function TopBar(p: TopBarProps) {
  const t = useT();
  const engineOk = p.link === "open" && !p.httpError;
  const linkText = p.link === "open" ? (p.httpError ? t("top.engineError") : t("top.live")) : p.link === "connecting" ? t("top.connecting") : t("top.offline");
  const machineText = !p.machine ? "" : p.machine.connected ? (p.machine.sim ? t("top.simulator") : t("top.laserConnected")) : t("top.noLaser");
  const title = [
    t("top.events", { state: p.link }),
    p.httpError ? t("top.lastError", { error: p.httpError }) : "",
    p.missingCommands.length ? t("top.missing", { list: p.missingCommands.join(", ") }) : "",
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <header className="topbar">
      {p.fileMenu}
      <div className="brand">
        <span className="brand-mark" aria-hidden="true" />
        <span className="brand-name">OpenEtch</span>
        <ProjectIndicator name={p.projectName} docName={p.docName} dirty={p.dirty} />
      </div>
      <nav className="workspaces" role="tablist" aria-label={t("top.workspace")}>
        {(["design", "laser"] as const).map((w) => (
          <button key={w} role="tab" aria-selected={p.workspace === w} className={p.workspace === w ? "active" : ""} onClick={() => p.onWorkspace(w)}>
            {w === "design" ? t("top.design") : t("top.laser")}
            {w === "laser" && p.pendingRuns > 0 && <span className="pill">{p.pendingRuns}</span>}
          </button>
        ))}
      </nav>
      <div className="topbar-actions">
        {p.extra}
        <button className="icon-btn" title={t("top.undoTitle")} aria-label={t("top.undo")} onClick={p.onUndo}>
          <Icon name="undo" />
        </button>
        <button className="icon-btn" title={t("top.redoTitle")} aria-label={t("top.redo")} onClick={p.onRedo}>
          <Icon name="redo" />
        </button>
        <div className={`conn ${engineOk ? "ok" : p.link === "connecting" ? "wait" : "bad"}`} title={title}>
          <span className="dot" />
          <span className="conn-text">
            {linkText}
            {machineText && ` · ${machineText}`}
          </span>
          {p.missingCommands.length > 0 && <span className="warn-chip">{t("top.cmdMissing", { n: p.missingCommands.length })}</span>}
        </div>
        <button className="btn btn-stop" onClick={p.onStop} title={t("top.stopTitle")}>
          <Icon name="stop" /> {t("top.stop")}
        </button>
      </div>
    </header>
  );
}
