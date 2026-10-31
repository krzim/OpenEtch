// Top bar: app name, Fusion-style workspace switcher, undo/redo, the always-visible Stop button, and the connection
// indicator (WebSocket state, engine reachability, and any command ids the UI needs that the engine lacks).

import type { LinkState } from "../api/useEngine.ts";
import type { MachineStatus } from "../api/types.ts";
import { Icon } from "./ui.tsx";

export type Workspace = "design" | "laser";

export interface TopBarProps {
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
  pendingRuns: number;
}

export function TopBar(p: TopBarProps) {
  const engineOk = p.link === "open" && !p.httpError;
  const linkText = p.link === "open" ? (p.httpError ? "Engine error" : "Live") : p.link === "connecting" ? "Connecting…" : "Offline";
  const machineText = !p.machine ? "" : p.machine.connected ? (p.machine.sim ? "Simulator" : "Laser connected") : "No laser";
  const title = [
    `Events: ${p.link}`,
    p.httpError ? `Last error: ${p.httpError}` : "",
    p.missingCommands.length ? `Engine is missing commands: ${p.missingCommands.join(", ")}` : "",
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <header className="topbar">
      <div className="brand">
        <span className="brand-mark" aria-hidden="true" />
        <span className="brand-name">OpenEtch</span>
        {p.docName && <span className="doc-name">{p.docName}</span>}
      </div>
      <nav className="workspaces" role="tablist" aria-label="Workspace">
        {(["design", "laser"] as const).map((w) => (
          <button key={w} role="tab" aria-selected={p.workspace === w} className={p.workspace === w ? "active" : ""} onClick={() => p.onWorkspace(w)}>
            {w === "design" ? "Design" : "Laser"}
            {w === "laser" && p.pendingRuns > 0 && <span className="pill">{p.pendingRuns}</span>}
          </button>
        ))}
      </nav>
      <div className="topbar-actions">
        <button className="icon-btn" title="Undo (Ctrl+Z)" aria-label="Undo" onClick={p.onUndo}>
          <Icon name="undo" />
        </button>
        <button className="icon-btn" title="Redo (Ctrl+Shift+Z)" aria-label="Redo" onClick={p.onRedo}>
          <Icon name="redo" />
        </button>
        <div className={`conn ${engineOk ? "ok" : p.link === "connecting" ? "wait" : "bad"}`} title={title}>
          <span className="dot" />
          <span className="conn-text">
            {linkText}
            {machineText && ` · ${machineText}`}
          </span>
          {p.missingCommands.length > 0 && <span className="warn-chip">{p.missingCommands.length} cmd missing</span>}
        </div>
        <button className="btn btn-stop" onClick={p.onStop} title="Stop the machine now. Always allowed.">
          <Icon name="stop" /> STOP
        </button>
      </div>
    </header>
  );
}
