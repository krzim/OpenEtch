// The Production window of the Laser workspace (also the phone's Job sheet and the tablet's dock): open the cockpit,
// the serial ledger, Continue a run waiting at a layer, what the next run marks, the design lock (Unlock when a cockpit
// left it on), maintenance due and the extractor. The status chip next to STOP (MaintenanceChip) and the canvas HUD
// (LayerPauseHud) show the urgent parts everywhere else.

import { useState } from "react";
import { Icon } from "../components/ui.tsx";
import { HudItem } from "../hud/CanvasHud.tsx";
import { Dock } from "../workspace/Dock.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { prodApi } from "./api.ts";
import { counterText, nextValues, shortDuration } from "./model.ts";
import { refreshProduction, useProduction } from "./store.ts";
import "./production.css";
import { HelpLink } from "../guide/HelpLink.tsx";

type Report = (e: unknown) => void;
type Notify = (m: string, kind?: "info" | "error") => void;

/** Continue the run waiting at a layer (one person's press, like the foot switch). */
function useContinue(notify: Notify, report: Report): [boolean, (runId: string | null) => void] {
  const t = useT();
  const [busy, setBusy] = useState(false);
  const go = (runId: string | null) => {
    setBusy(true);
    prodApi
      .continueRun(runId)
      .then(() => notify(t("prod.continued")))
      .catch(report)
      .finally(() => {
        setBusy(false);
        void refreshProduction();
      });
  };
  return [busy, go];
}

export function ProductionDock(p: { onCockpit: () => void; onLedger: () => void; notify: Notify; report: Report }) {
  const t = useT();
  const { status: s, unsupported } = useProduction();
  const [busy, cont] = useContinue(p.notify, p.report);
  if (unsupported) return null;
  const c = s ? counterText(s.cycle) : null;
  return (
    <Dock help="production-cockpit#production-window" id="production" title={t("prod.title")}>
      <div className="prod-dock">
        <div className="prod-row">
          <button className="btn btn-primary" onClick={p.onCockpit} title={t("prod.cockpitTitle")}>
            <Icon name="gauge" size={18} /> {t("prod.cockpit")}
          </button>
          <button className="btn" onClick={p.onLedger}>
            <Icon name="ledger" size={18} /> {t("prod.ledger")}
          </button>
        </div>
        {s?.layer_pause.waiting && (
          <div className="prod-wait" role="alert">
            <strong>{t("prod.phWaiting")}</strong>
            <span>{t("prod.layerShort", { layer: s.layer_pause.layer ?? "" })}</span>
            <button className="btn prod-continue" disabled={busy} onClick={() => cont(s.layer_pause.run_id)}>
              <Icon name="skipForward" size={18} /> {t("prod.continue")}
            </button>
          </div>
        )}
        {s && (
          <dl className="kv prod-kv">
            <dt>{t("prod.done")}</dt>
            <dd>{c?.text}</dd>
            {s.slots.slots.length > 0 && (
              <>
                <dt>{t("prod.next")}</dt>
                <dd>{nextValues(s.slots)}</dd>
              </>
            )}
            <dt>{t("prod.lastCycle")}</dt>
            <dd>{shortDuration(s.cycle.last_cycle_s ?? s.cycle.last_burn_s)}</dd>
          </dl>
        )}
        {s?.slots_refusal && (
          <p className="run-hint" role="alert">
            {s.slots_refusal}
          </p>
        )}
        {s?.lock.locked && (
          <div className="prod-row prod-lockline">
            <Icon name="lock" size={16} />
            <span>{t("prod.lockedBy", { label: s.lock.label || "?" })}</span>
            <button className="btn small" onClick={() => void prodApi.lock(false, "", "", true).then(() => refreshProduction(), p.report)}>
              {t("prod.unlock")}
            </button>
          </div>
        )}
        {s?.maintenance.due.map((r) => (
          <div key={r.id} className="prod-row prod-due">
            <Icon name="wrench" size={16} />
            <span>{t("prod.dueLine", { name: r.name })}</span>
          </div>
        ))}
        {s?.extractor.configured && (
          <div className="prod-row">
            <Icon name="fan" size={16} />
            <span>{s.extractor.state.on ? t("prod.exOn") : t("prod.exOff")}</span>
          </div>
        )}
      </div>
    </Dock>
  );
}

/** Over the canvas while a run waits at a layer: the layer and Continue (every workspace, every layout). */
export function LayerPauseHud(p: { notify: Notify; report: Report }) {
  const t = useT();
  const { status: s } = useProduction();
  const [busy, cont] = useContinue(p.notify, p.report);
  if (!s?.layer_pause.waiting) return null;
  return (
    <HudItem id="layer-pause" slot="tc" rank={5} minW={240}>
      <div className="vp-overlay prod-hud" role="alert">
        <Icon name="pause" size={18} />
        <span>
          <strong>{t("prod.phWaiting")}</strong> {t("prod.layerShort", { layer: s.layer_pause.layer ?? "" })}
        </span>
        <HelpLink to="running-a-job#pause-at-a-layer" topic={t("prod.phWaiting")} />
        <button className="btn prod-continue" disabled={busy} onClick={() => cont(s.layer_pause.run_id)}>
          {t("prod.continue")}
        </button>
      </div>
    </HudItem>
  );
}

/** Left of the machine status chip: maintenance due (and a run waiting at a layer), one tap from anywhere. */
export function MaintenanceChip(p: { compact?: boolean; onOpen: () => void }) {
  const t = useT();
  const { status: s } = useProduction();
  const due = s?.maintenance.due ?? [];
  if (due.length === 0) return null;
  const label = due.length === 1 ? t("prod.dueLine", { name: due[0].name }) : t("prod.dueMany", { n: due.length });
  return (
    <button className="prod-chip" onClick={p.onOpen} title={label} aria-label={label}>
      <Icon name="wrench" size={16} />
      {!p.compact && <span>{label}</span>}
    </button>
  );
}
