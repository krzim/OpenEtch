// The empty-document card (owner: P5, docs/ux/ui-plan-2026-11.md P5, UX-11): drop a file, or Import, Open project,
// Draw a rectangle or text, Templates, and a device status line ("Laser: ComMarker B6 MOPA · Idle (simulator)"). P1
// mounts it in `.center-stage` while the document is empty; it also hides itself as soon as a source reports objects.
// Only the card takes pointer events, so the canvas around it (and file drops) work as before.

import { HudItem } from "../hud/CanvasHud.tsx";
import { Icon } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { chipState } from "./statusChip.ts";
import { useLaserLive } from "./useLaserLive.ts";
import "./laser.css";
import { HelpLink } from "../guide/HelpLink.tsx";

export interface EmptyCanvasCardProps {
  onImport: () => void;
  onOpen: () => void;
  /** Pick a drawing tool. */
  onDraw: (tool: "rect" | "text") => void;
  onTemplates: () => void;
  /** New from example (templates/Examples.tsx); absent on engines without examples. */
  onExamples?: () => void;
}

/** The device name GET /api/machine reports, or null. */
export function deviceName(m: Record<string, unknown> | null): string | null {
  if (!m) return null;
  const d = m.device && typeof m.device === "object" ? (m.device as { display_name?: unknown }).display_name : null;
  const name = typeof d === "string" && d ? d : m.profile;
  return typeof name === "string" && name ? name : null;
}

export function EmptyCanvasCard(p: EmptyCanvasCardProps) {
  const t = useT();
  const { facts } = useLaserLive();
  if (facts.docEmpty === false) return null;
  const chip = chipState(facts.machine, facts.link, null, facts.httpError);
  const device = deviceName(facts.machine);
  return (
    // The centre slot of the canvas HUD: the largest free stretch, clear of the corner buttons (scrolls when shorter).
    <HudItem id="empty-card" slot="c" rank={70} minW={260} maxW={620} minH={160}>
      <section className="empty-card" aria-labelledby="empty-card-title">
        <h2 id="empty-card-title" className="empty-card-title">
          {t("lz.emptyTitle")}
          <HelpLink to="getting-started#start-a-design" topic={t("lz.emptyTitle")} />
        </h2>
        <p className="empty-card-drop">{t("lz.emptyDrop")}</p>
        <div className="empty-card-actions">
          <button className="btn btn-primary" onClick={p.onImport}>
            <Icon name="import" size={18} /> {t("lz.emptyImport")}
          </button>
          <button className="btn" onClick={p.onOpen}>
            <Icon name="upload" size={18} /> {t("lz.emptyOpen")}
          </button>
          <button className="btn" onClick={() => p.onDraw("rect")}>
            <Icon name="rect" size={18} /> {t("lz.emptyRect")}
          </button>
          <button className="btn" onClick={() => p.onDraw("text")}>
            <span className="empty-card-t" aria-hidden="true">
              T
            </span>{" "}
            {t("lz.emptyText")}
          </button>
          <button className="btn" onClick={p.onTemplates}>
            <Icon name="array" size={18} /> {t("lz.emptyTemplates")}
          </button>
          {p.onExamples && (
            <button className="btn" onClick={p.onExamples}>
              <Icon name="book" size={18} /> {t("tpl.emptyExamples")}
            </button>
          )}
        </div>
        <p className={`empty-card-laser tone-${chip.tone}`} role="status">
          <span className="status-chip-dot" aria-hidden="true" />
          {device ? t("lz.emptyLaser", { device, state: chip.text }) : t("lz.emptyLaserUnknown", { state: chip.text })}
        </p>
      </section>
    </HudItem>
  );
}
