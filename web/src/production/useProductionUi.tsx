// The production features' hook for App.tsx (a few lines there): the cockpit as a mode of the app (`#/cockpit`, which
// the installable app's shortcut opens), the serial ledger and maintenance windows, the hotkey actions for command
// search and the ribbon (`production.cockpit`, `production.ledger`), the Production window and the layer pause HUD.

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { Icon } from "../components/ui.tsx";
import { WsModal } from "../workspace/WsModal.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { prodApi } from "./api.ts";
import { Cockpit } from "./Cockpit.tsx";
import { LedgerDialog } from "./LedgerDialog.tsx";
import { COCKPIT_HASH, isCockpitHash, reminderLeft } from "./model.ts";
import { LayerPauseHud, MaintenanceChip, ProductionDock } from "./ProductionDock.tsx";
import { refreshProduction, useProduction } from "./store.ts";

type Notify = (m: string, kind?: "info" | "error") => void;

function hashNow(): string {
  return typeof location === "undefined" ? "" : location.hash;
}

export function useProductionUi(p: { notify: Notify; report: (e: unknown) => void; revision: number | null }) {
  const [cockpit, setCockpit] = useState(() => isCockpitHash(hashNow()));
  const [win, setWin] = useState<"ledger" | "maintenance" | null>(null);
  useEffect(() => {
    const on = () => setCockpit(isCockpitHash(hashNow()));
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  const openCockpit = useCallback(() => {
    if (!isCockpitHash(hashNow())) history.pushState(null, "", COCKPIT_HASH);
    setCockpit(true);
  }, []);
  const closeCockpit = useCallback(() => {
    if (isCockpitHash(hashNow())) history.replaceState(null, "", `${location.pathname}${location.search}`);
    setCockpit(false);
  }, []);
  const openLedger = useCallback(() => setWin("ledger"), []);

  const handlers: Record<string, () => boolean | void> = {
    "production.cockpit": openCockpit,
    "production.ledger": openLedger,
  };

  const dialogs =
    win === "ledger" ? (
      <LedgerDialog revision={p.revision} notify={p.notify} report={p.report} onClose={() => setWin(null)} />
    ) : win === "maintenance" ? (
      <MaintenanceDialog report={p.report} onClose={() => setWin(null)} />
    ) : null;

  return {
    cockpitOpen: cockpit,
    /** The cockpit with the app's overlays (toasts, dialogs) over it. */
    renderCockpit: (overlays: ReactNode) => (
      <Cockpit onExit={closeCockpit} notify={p.notify} report={p.report}>
        {overlays}
      </Cockpit>
    ),
    handlers,
    dialogs,
    dock: <ProductionDock onCockpit={openCockpit} onLedger={openLedger} notify={p.notify} report={p.report} />,
    hud: <LayerPauseHud notify={p.notify} report={p.report} />,
    chip: (compact: boolean) => <MaintenanceChip compact={compact} onOpen={() => setWin("maintenance")} />,
  };
}

/** Due maintenance with Done, from the chip. */
function MaintenanceDialog(p: { report: (e: unknown) => void; onClose: () => void }) {
  const t = useT();
  const { status: s } = useProduction();
  const m = s?.maintenance;
  return (
    <WsModal
      title={t("prod.maintenance")}
      size="sm"
      onClose={p.onClose}
      footer={
        <button className="btn btn-primary" onClick={p.onClose}>
          {t("common.close")}
        </button>
      }
    >
      {!m || m.reminders.length === 0 ? (
        <p className="hint">{t("prod.remNone")}</p>
      ) : (
        <ul className="prod-remlist">
          {m.reminders.map((r) => (
            <li key={r.id} className={r.due ? "prod-due" : ""}>
              <Icon name="wrench" size={16} />
              <span className="prod-remname">{r.name}</span>
              <span className="hint">{reminderLeft(r)}</span>
              <button className="btn small" onClick={() => void prodApi.reminderDone(r.id).then(() => refreshProduction(), p.report)}>
                {t("prod.remMarkDone")}
              </button>
            </li>
          ))}
        </ul>
      )}
      <p className="hint">{t("prod.remSettingsHint")}</p>
    </WsModal>
  );
}
