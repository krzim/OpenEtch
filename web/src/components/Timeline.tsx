// Bottom "Timeline": one chip per history entry, oldest on the left, like Fusion's design timeline. Each chip carries
// an origin badge so it is obvious what the user, an AI agent or a script changed. An edit announced live by an agent
// or script (`document_changed.origin`) briefly flashes its chip.

import { useEffect, useRef } from "react";
import type { HistoryEntry, Origin } from "../api/types.ts";
import type { RemoteEdit } from "../api/useEngine.ts";

/** Display label and CSS class for an origin. */
export function originBadge(o: Origin | undefined): { label: string; cls: string } {
  if (o === undefined || o === null) return { label: "?", cls: "origin-unknown" };
  if (typeof o === "string") {
    if (o === "ui") return { label: "You", cls: "origin-ui" };
    if (o === "script") return { label: "Script", cls: "origin-script" };
    if (o.startsWith("agent")) return { label: o.replace(/^agent[:\s]*/, "") || "AI", cls: "origin-agent" };
    return { label: o, cls: "origin-unknown" };
  }
  switch (o.kind) {
    case "ui":
      return { label: "You", cls: "origin-ui" };
    case "script":
      return { label: "Script", cls: "origin-script" };
    case "agent":
      return { label: "name" in o && o.name ? o.name : "AI agent", cls: "origin-agent" };
    default:
      return { label: String((o as { kind: unknown }).kind), cls: "origin-unknown" };
  }
}

export function Timeline({ history, revision, flash }: { history: HistoryEntry[]; revision: number | null; flash?: RemoteEdit | null }) {
  const stripRef = useRef<HTMLDivElement>(null);
  // Keep the newest entry in view.
  useEffect(() => {
    const el = stripRef.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [history.length]);
  return (
    <div className="timeline">
      <div className="timeline-label">Timeline</div>
      <div className="timeline-strip" ref={stripRef}>
        {history.length === 0 && <span className="muted">No edits yet.</span>}
        {history.map((h, i) => {
          const b = originBadge(h.origin);
          const flashing = !!flash && flash.revision === h.revision;
          return (
            <div
              // A new seq remounts the chip so the flash animation replays.
              key={`${h.revision}-${i}${flashing ? `-f${flash.seq}` : ""}`}
              className={`tl-entry ${b.cls}${revision === h.revision ? " current" : ""}${flashing ? " flash" : ""}`}
              title={`r${h.revision} · ${h.command}\n${h.summary}\nby ${b.label}`}
            >
              <span className={`badge ${b.cls}`}>{b.label}</span>
              <span className="tl-cmd">{shortCommand(h.command)}</span>
              <span className="tl-rev">r{h.revision}</span>
            </div>
          );
        })}
        <div className="tl-marker" aria-hidden="true" />
      </div>
    </div>
  );
}

/** "object.transform, layer.update" -> "transform +1". */
function shortCommand(cmd: string): string {
  const parts = cmd.split(",").map((s) => s.trim()).filter(Boolean);
  const first = parts[0] ?? cmd;
  const name = first.split(".").pop() ?? first;
  return parts.length > 1 ? `${name} +${parts.length - 1}` : name;
}
