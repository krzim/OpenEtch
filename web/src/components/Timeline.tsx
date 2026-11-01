// Bottom "Timeline", like Fusion's design timeline: one chip per recorded step, oldest on the left, and a marker after
// the last applied step. Click a step (or right-click > "Roll marker here") to roll the design back to it; steps right
// of the marker are greyed out, not deleted, and a new edit is inserted at the marker (the engine replays the later
// steps; one that no longer applies is kept and shown red). Drag the marker to scrub. Clicks, drags and the buttons all
// move the marker with `history.goto`; Ctrl+Z / Ctrl+Y are the engine's undo / redo, and steps undone that way are
// discarded by the next edit (LightBurn), so they are drawn dashed and struck through and their tooltip says so (see
// api/history.ts). Each chip carries an origin badge so it is obvious what the user, an AI agent or a script changed,
// and an edit announced live by an agent or script (`document_changed.origin`) briefly flashes its chip. Older engines
// without the timeline show a read-only log.

import { useEffect, useRef, useState } from "react";
import {
  afterMarkerNote,
  discardedByNextEdit,
  isApplied,
  markerForSlot,
  markerSlot,
  nearestSlot,
  type TimelineModel,
  type TimelineStep,
} from "../api/history.ts";
import type { Origin } from "../api/types.ts";
import type { RemoteEdit } from "../api/useEngine.ts";
import { capturePointer } from "../pointer.ts";
import { Icon } from "./ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import type { T } from "../workspace/i18n.ts";

/** Display label and CSS class for an origin. */
export function originBadge(o: Origin | undefined, t: T): { label: string; cls: string } {
  if (o === undefined || o === null) return { label: "?", cls: "origin-unknown" };
  if (typeof o === "string") {
    if (o === "ui") return { label: t("tl.you"), cls: "origin-ui" };
    if (o === "script") return { label: t("tl.script"), cls: "origin-script" };
    if (o.startsWith("agent")) return { label: o.replace(/^agent[:\s]*/, "") || t("tl.ai"), cls: "origin-agent" };
    return { label: o, cls: "origin-unknown" };
  }
  switch (o.kind) {
    case "ui":
      return { label: t("tl.you"), cls: "origin-ui" };
    case "script":
      return { label: t("tl.script"), cls: "origin-script" };
    case "agent":
      return { label: "name" in o && o.name ? o.name : t("tl.aiAgent"), cls: "origin-agent" };
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
  const t = useT();
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
    // Unguarded, a capture that fails ("No active pointer", e.g. on touch) threw and the drag never started.
    capturePointer(e.currentTarget, e.pointerId);
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
      aria-label={t("tl.historyMarker")}
      aria-valuemin={0}
      aria-valuemax={steps.length}
      aria-valuenow={shownSlot}
      title={editable ? t("tl.historyMarkerDragToRollThe") : undefined}
      onPointerDown={onMarkerDown}
      onPointerMove={onMarkerMove}
      onPointerUp={onMarkerUp}
      onPointerCancel={() => setDragSlot(null)}
    />
  );

  const chips: React.ReactNode[] = [];
  steps.forEach((h, i) => {
    if (i === shownSlot) chips.push(marker);
    const b = originBadge(h.origin, t);
    const flashing = !!flash && h.revision !== null && flash.revision === h.revision;
    const applied = dragSlot !== null ? i < dragSlot : isApplied(timeline, h);
    // While scrubbing, the drop is a goto: the steps after the marker will be kept, not discarded.
    const undone = !applied && dragSlot === null && discardedByNextEdit(timeline, h);
    const cls = [
      "tl-entry",
      b.cls,
      applied ? "" : "rolled-back",
      undone ? "undone" : "",
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
      h.state === "failed" ? `Failed on replay: ${h.error ?? t("tl.theStepNoLongerApplies")}` : "",
      !applied ? afterMarkerNote(dragSlot === null ? timeline : { ...timeline, discardThrough: timeline.marker }, h) : "",
      editable ? t("tl.clickToRollTheMarkerHere") : "",
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
      <div className="timeline-label">{t("tl.timeline")}</div>
      {editable && (
        <div className="tl-controls" role="group" aria-label={t("tl.timelinePlayback")}>
          <button className="icon-btn small" title={t("tl.goToStart")} aria-label={t("tl.goToStart")} disabled={slot === 0} onClick={() => goSlot(0)}>
            <Icon name="first" size={16} />
          </button>
          <button className="icon-btn small" title={t("tl.previousStep")} aria-label={t("tl.previousStep")} disabled={slot === 0} onClick={() => goSlot(slot - 1)}>
            <Icon name="prev" size={16} />
          </button>
          <button className="icon-btn small" title={t("tl.nextStep")} aria-label={t("tl.nextStep")} disabled={slot >= steps.length} onClick={() => goSlot(slot + 1)}>
            <Icon name="next" size={16} />
          </button>
          <button className="icon-btn small" title={t("tl.goToEnd")} aria-label={t("tl.goToEnd")} disabled={slot >= steps.length} onClick={() => goSlot(steps.length)}>
            <Icon name="last" size={16} />
          </button>
        </div>
      )}
      <div className="timeline-strip" ref={stripRef} onPointerMove={onMarkerMove} onPointerUp={onMarkerUp}>
        {steps.length === 0 ? <span className="muted tl-empty">{t("tl.noEditsYet")}</span> : chips}
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
            {t("tl.rollMarkerHere")}
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
            {t("tl.rollMarkerBeforeThisStep")}
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
            {t("tl.deleteStep")}
          </button>
        </div>
      )}
    </div>
  );
}
