// Bottom "Timeline", like Fusion's design timeline: one chip per recorded step, oldest on the left, and a marker after
// the last applied step. Click a step (or right-click > "Roll marker here") to roll the design back to it; steps right
// of the marker are greyed out, not deleted, and a new edit is inserted at the marker (the engine replays the later
// steps; one that no longer applies is kept and shown red). Drag the marker to scrub. Each chip carries an origin
// badge so it is obvious what the user, an AI agent or a script changed, and an edit announced live by an agent or
// script (`document_changed.origin`) briefly flashes its chip. Older engines without the timeline show a read-only log.

import { useEffect, useRef, useState } from "react";
import { isApplied, markerForSlot, markerSlot, nearestSlot, type TimelineModel, type TimelineStep } from "../api/history.ts";
import type { Origin } from "../api/types.ts";
import type { RemoteEdit } from "../api/useEngine.ts";
import { Icon } from "./ui.tsx";

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

export interface TimelineProps {
  timeline: TimelineModel;
  flash?: RemoteEdit | null;
  onGoto: (marker: number) => void;
  onDelete: (index: number) => void;
}

export function Timeline({ timeline, flash, onGoto, onDelete }: TimelineProps) {
  const stripRef = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<{ step: TimelineStep; x: number; y: number } | null>(null);
  /** Slot the marker is being dragged to, or null. */
  const [dragSlot, setDragSlot] = useState<number | null>(null);
  const slot = markerSlot(timeline);
  const shownSlot = dragSlot ?? slot;
  const { steps, editable } = timeline;

  // Keep the marker in view when it moves or a step is added.
  useEffect(() => {
    const el = stripRef.current;
    const m = el?.querySelector<HTMLElement>(".tl-marker");
    if (el && m) {
      const left = m.offsetLeft - el.offsetLeft;
      if (left < el.scrollLeft + 40 || left > el.scrollLeft + el.clientWidth - 40) el.scrollLeft = left - el.clientWidth + 80;
    }
  }, [steps.length, slot]);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", onKey);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", close);
    };
  }, [menu]);

  const slotAt = (clientX: number): number => {
    const el = stripRef.current;
    if (!el) return slot;
    const chips = [...el.querySelectorAll<HTMLElement>(".tl-entry")];
    return nearestSlot(
      chips.map((c) => c.getBoundingClientRect().right),
      chips.map((c) => c.getBoundingClientRect().left),
      clientX,
    );
  };

  const onMarkerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!editable || e.button !== 0) return;
    e.preventDefault();
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    setDragSlot(slot);
  };
  const onMarkerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (dragSlot === null) return;
    setDragSlot(slotAt(e.clientX));
  };
  const onMarkerUp = () => {
    if (dragSlot === null) return;
    const target = dragSlot;
    setDragSlot(null);
    if (target !== slot) onGoto(markerForSlot(timeline, target));
  };

  const goSlot = (s: number) => {
    const clamped = Math.max(0, Math.min(steps.length, s));
    if (clamped !== slot) onGoto(markerForSlot(timeline, clamped));
  };

  const marker = (
    <div
      key="marker"
      className={`tl-marker${editable ? " draggable" : ""}${dragSlot !== null ? " dragging" : ""}`}
      role={editable ? "slider" : undefined}
      aria-label="History marker"
      aria-valuemin={0}
      aria-valuemax={steps.length}
      aria-valuenow={shownSlot}
      title={editable ? "History marker: drag to roll the design back or forward" : undefined}
      onPointerDown={onMarkerDown}
      onPointerMove={onMarkerMove}
      onPointerUp={onMarkerUp}
      onPointerCancel={() => setDragSlot(null)}
    />
  );

  const chips: React.ReactNode[] = [];
  steps.forEach((h, i) => {
    if (i === shownSlot) chips.push(marker);
    const b = originBadge(h.origin);
    const flashing = !!flash && h.revision !== null && flash.revision === h.revision;
    const applied = dragSlot !== null ? i < dragSlot : isApplied(timeline, h);
    const cls = [
      "tl-entry",
      b.cls,
      applied ? "" : "rolled-back",
      h.state === "failed" ? "failed" : "",
      i === shownSlot - 1 ? "current" : "",
      flashing ? "flash" : "",
      editable ? "clickable" : "",
    ]
      .filter(Boolean)
      .join(" ");
    const title = [
      `${h.commands || h.label}`,
      h.summary,
      `by ${b.label}`,
      h.state === "failed" ? `Failed on replay: ${h.error ?? "the step no longer applies"}` : "",
      !applied ? "Rolled back (after the marker)" : "",
      editable ? "Click to roll the marker here. Right-click for more." : "",
    ]
      .filter(Boolean)
      .join("\n");
    chips.push(
      <button
        type="button"
        // A new seq remounts the chip so the flash animation replays.
        key={`${h.index}-${i}${flashing ? `-f${flash.seq}` : ""}`}
        className={cls}
        title={title}
        onClick={() => editable && goSlot(i + 1)}
        onContextMenu={(e) => {
          if (!editable) return;
          e.preventDefault();
          setMenu({ step: h, x: e.clientX, y: e.clientY });
        }}
      >
        <span className={`badge ${b.cls}`}>{b.label}</span>
        <span className="tl-cmd">{h.label}</span>
      </button>,
    );
  });
  if (shownSlot >= steps.length) chips.push(marker);

  return (
    <div className="timeline">
      <div className="timeline-label">Timeline</div>
      {editable && (
        <div className="tl-controls" role="group" aria-label="Timeline playback">
          <button className="icon-btn small" title="Go to start" aria-label="Go to start" disabled={slot === 0} onClick={() => goSlot(0)}>
            <Icon name="first" size={16} />
          </button>
          <button className="icon-btn small" title="Previous step" aria-label="Previous step" disabled={slot === 0} onClick={() => goSlot(slot - 1)}>
            <Icon name="prev" size={16} />
          </button>
          <button className="icon-btn small" title="Next step" aria-label="Next step" disabled={slot >= steps.length} onClick={() => goSlot(slot + 1)}>
            <Icon name="next" size={16} />
          </button>
          <button className="icon-btn small" title="Go to end" aria-label="Go to end" disabled={slot >= steps.length} onClick={() => goSlot(steps.length)}>
            <Icon name="last" size={16} />
          </button>
        </div>
      )}
      <div className="timeline-strip" ref={stripRef} onPointerMove={onMarkerMove} onPointerUp={onMarkerUp}>
        {steps.length === 0 ? <span className="muted tl-empty">No edits yet.</span> : chips}
      </div>
      {menu && (
        <div className="menu ctx-menu" role="menu" style={{ left: menu.x, top: menu.y }} onPointerDown={(e) => e.stopPropagation()}>
          <button
            role="menuitem"
            className="menu-item"
            onClick={() => {
              setMenu(null);
              onGoto(menu.step.index);
            }}
          >
            Roll marker here
          </button>
          <button
            role="menuitem"
            className="menu-item"
            onClick={() => {
              setMenu(null);
              const i = steps.findIndex((s) => s.index === menu.step.index);
              onGoto(markerForSlot(timeline, i));
            }}
          >
            Roll marker before this step
          </button>
          <div className="menu-sep" role="separator" />
          <button
            role="menuitem"
            className="menu-item danger"
            onClick={() => {
              setMenu(null);
              onDelete(menu.step.index);
            }}
          >
            Delete step
          </button>
        </div>
      )}
    </div>
  );
}
