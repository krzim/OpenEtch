// Bottom "Timeline", like Fusion's design timeline: one chip per recorded step, oldest on the left, and a marker after
// the last applied step. Click a step (or right-click > "Roll marker here") to roll the design back to it; steps right
// of the marker are greyed out, not deleted, and a new edit is inserted at the marker (the engine replays the later
// steps; one that no longer applies is kept and shown red). Drag the marker to scrub. Clicks, drags and the buttons all
// move the marker with `history.goto`; Ctrl+Z / Ctrl+Y are the engine's undo / redo, and steps undone that way are
// discarded by the next edit (LightBurn), so they are drawn dashed and struck through and their tooltip says so (see
// api/history.ts). Each chip carries an origin badge so it is obvious what the user, an AI agent or a script changed,
// and an edit announced live by an agent or script (`document_changed.origin`) briefly flashes its chip. Older engines
// without the timeline show a read-only log.
//
// Chips say what a step did ("Rectangle", "Move", Text "Hello": timelineLabel.ts). A tap with a finger or pen, a
// right-click or a long press opens the step's popover (details, Roll here / Roll before / Delete) instead of hiding the
// details in a tooltip; a mouse click still rolls the marker. The marker has a 32 px hit area with a visible grip. On a
// phone the timeline is a list (`variant="list"`, More > History).

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
import { useLongPress } from "../touch/useLongPress.ts";
import { Menu, type MenuAnchor, type MenuItem } from "./Menu.tsx";
import { timelineLabel, type StepIcon } from "./timelineLabel.ts";
import { Icon } from "./ui.tsx";
import { ShellIcon, type ShellIconName } from "../workspace/shellIcons.tsx";
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
  /** "strip" (the bottom row, default) or "list" (the phone's History in More). */
  variant?: "strip" | "list";
}

/** A step's icon from components/ui.tsx or workspace/shellIcons.tsx. */
function StepIconView({ icon }: { icon: StepIcon }) {
  return icon.set === "shell" ? <ShellIcon name={icon.name as ShellIconName} size={14} /> : <Icon name={icon.name} size={14} />;
}

export function Timeline({ timeline, flash, onGoto, onDelete, variant = "strip" }: TimelineProps) {
  const t = useT();
  const stripRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLOListElement>(null);
  const [menu, setMenu] = useState<{ step: TimelineStep; at: MenuAnchor } | null>(null);
  /** Slot the marker is being dragged to, or null. */
  const [dragSlot, setDragSlot] = useState<number | null>(null);
  /** The kind of pointer that pressed a chip last: a mouse click rolls the marker, a tap opens the step's popover. */
  const pressedWith = useRef<string>("mouse");
  const slot = markerSlot(timeline);
  const shownSlot = dragSlot ?? slot;
  const { steps, editable } = timeline;
  // The touch right-click: a long press on a chip opens its popover (touch/useLongPress.ts).
  const longPress = useLongPress((x, y, target) => {
    const i = Number((target as HTMLElement).dataset.step);
    const step = steps[i];
    if (step) setMenu({ step, at: { x, y } });
  });

  // Keep the marker in view when it moves or a step is added.
  useEffect(() => {
    if (variant === "list") {
      listRef.current?.querySelector(".tl-now")?.scrollIntoView?.({ block: "nearest" });
      return;
    }
    const el = stripRef.current;
    const m = el?.querySelector<HTMLElement>(".tl-marker");
    if (el && m) {
      const left = m.offsetLeft - el.offsetLeft;
      if (left < el.scrollLeft + 40 || left > el.scrollLeft + el.clientWidth - 40) el.scrollLeft = left - el.clientWidth + 80;
    }
  }, [steps.length, slot, variant]);

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

  /** The step's popover: what it is, who made it, its state, and Roll here / Roll before / Delete. */
  const stepItems = (h: TimelineStep): MenuItem[] => {
    const i = steps.findIndex((s) => s.index === h.index);
    const b = originBadge(h.origin, t);
    const lbl = timelineLabel(h, t);
    const summary = h.summary.length > 90 ? `${h.summary.slice(0, 89)}…` : h.summary;
    const applied = isApplied(timeline, h);
    const items: MenuItem[] = [
      { label: lbl.text, disabled: true },
      ...(summary ? [{ label: summary, disabled: true }] : []),
      { label: t("shell.tlBy", { name: b.label }), disabled: true },
    ];
    if (h.state === "failed") items.push({ label: `${t("shell.tlFailedShort")}: ${h.error ?? t("tl.theStepNoLongerApplies")}`, disabled: true });
    else if (!applied) items.push({ label: discardedByNextEdit(timeline, h) ? t("shell.tlUndone") : t("shell.tlRolledBack"), disabled: true });
    if (!editable) return items;
    items.push(
      { sep: true },
      { label: t("tl.rollMarkerHere"), action: () => onGoto(h.index) },
      { label: t("tl.rollMarkerBeforeThisStep"), action: () => onGoto(markerForSlot(timeline, i)) },
      { sep: true },
      { label: t("tl.deleteStep"), danger: true, action: () => onDelete(h.index) },
    );
    return items;
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
    >
      {editable && <span className="tl-grip" aria-hidden="true" />}
    </div>
  );

  const entryClass = (h: TimelineStep, i: number) => {
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
      // The friendly name first ("Rectangle"), the engine's command id after it for the curious.
      `${timelineLabel(h, t).text} (${h.commands || h.label})`,
      h.summary,
      `by ${b.label}`,
      h.state === "failed" ? `Failed on replay: ${h.error ?? t("tl.theStepNoLongerApplies")}` : "",
      !applied ? afterMarkerNote(dragSlot === null ? timeline : { ...timeline, discardThrough: timeline.marker }, h) : "",
      editable ? t("tl.clickToRollTheMarkerHere") : "",
    ]
      .filter(Boolean)
      .join("\n");
    return { cls, title, badge: b, flashing };
  };

  const menuView = menu && <Menu items={stepItems(menu.step)} anchor={menu.at} placement="above" onClose={() => setMenu(null)} />;

  if (variant === "list") {
    const rows: React.ReactNode[] = [];
    const now = (
      <li key="now" className="tl-now" aria-current="step">
        <span>{t("shell.tlNow")}</span>
      </li>
    );
    steps.forEach((h, i) => {
      if (i === slot) rows.push(now);
      const { cls, badge } = entryClass(h, i);
      const lbl = timelineLabel(h, t);
      rows.push(
        <li key={`${h.index}-${i}`} className={`tl-list-row ${cls}`}>
          <button type="button" className="tl-list-main" onClick={() => editable && goSlot(i + 1)}>
            <StepIconView icon={lbl.icon} />
            <span className="tl-cmd">{lbl.text}</span>
            <span className={`badge ${badge.cls}`}>{badge.label}</span>
          </button>
          <button
            type="button"
            className="icon-btn small tl-list-more"
            aria-label={t("shell.tlStepMenu")}
            title={t("shell.tlStepMenu")}
            onClick={(e) => setMenu({ step: h, at: e.currentTarget.getBoundingClientRect() })}
          >
            <Icon name="more" size={16} />
          </button>
        </li>,
      );
    });
    if (slot >= steps.length) rows.push(now);
    return (
      <div className="timeline-list-wrap">
        {steps.length === 0 ? <p className="muted tl-empty">{t("tl.noEditsYet")}</p> : <ol className="timeline-list" ref={listRef}>{rows}</ol>}
        {menuView}
      </div>
    );
  }

  const chips: React.ReactNode[] = [];
  steps.forEach((h, i) => {
    if (i === shownSlot) chips.push(marker);
    const { cls, title, badge, flashing } = entryClass(h, i);
    const lbl = timelineLabel(h, t);
    chips.push(
      <button
        type="button"
        // A new seq remounts the chip so the flash animation replays.
        key={`${h.index}-${i}${flashing ? `-f${flash?.seq}` : ""}`}
        className={cls}
        title={title}
        data-step={i}
        {...longPress}
        onPointerDown={(e) => {
          pressedWith.current = e.pointerType;
          longPress.onPointerDown?.(e);
        }}
        onClick={(e) => {
          // A tap (finger or pen) shows the step and its actions; a mouse click rolls the marker there (Fusion).
          if (pressedWith.current === "touch" || pressedWith.current === "pen") setMenu({ step: h, at: e.currentTarget.getBoundingClientRect() });
          else if (editable) goSlot(i + 1);
        }}
        onContextMenu={(e) => {
          longPress.onContextMenu?.(e);
          e.preventDefault();
          setMenu({ step: h, at: { x: e.clientX, y: e.clientY } });
        }}
      >
        <span className={`badge ${badge.cls}`}>{badge.label}</span>
        <span className="tl-cmd">
          <StepIconView icon={lbl.icon} />
          {lbl.text}
        </span>
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
      {menuView}
    </div>
  );
}
