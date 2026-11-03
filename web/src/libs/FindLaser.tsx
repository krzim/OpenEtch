// LightBurn's Find My Laser / Create Manually for a galvo, as one wizard: scan the USB bus for BJJCZ boards
// (GET /api/devices/scan; builds with the `usb` feature), then import EzCad's `markcfg7` and the lens correction `.cor`
// from the laser's EzCad folder, name the device and optionally make it active (POST /api/devices/wizard, UI only:
// the device is built from the markcfg, never with raised limits). Unverified on hardware.

import { useEffect, useState } from "react";
import { CheckField } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { libsApi } from "./api.ts";
import { hex4, parseScan, wizardDeviceName, type FoundBoard } from "./model.ts";
import { FilePick, errText, fileBase64 } from "./ui.tsx";
import "./libs.css";

export function FindLaserWizard(p: { notify: (m: string, k?: "info" | "error") => void; report: (e: unknown) => void; onClose: () => void; onAdded: () => void }) {
  const t = useT();
  const [scan, setScan] = useState<{ usb: boolean; boards: FoundBoard[]; note: string | null } | null>(null);
  const [scanning, setScanning] = useState(false);
  const [markcfg, setMarkcfg] = useState<File | null>(null);
  const [cor, setCor] = useState<File | null>(null);
  const [name, setName] = useState("");
  const [activate, setActivate] = useState(false);
  const [busy, setBusy] = useState(false);
  const doScan = () => {
    setScanning(true);
    libsApi
      .scan()
      .then((r) => setScan(parseScan(r)))
      .catch((e) => p.report(new Error(errText(e))))
      .finally(() => setScanning(false));
  };
  useEffect(doScan, []);
  useEffect(() => {
    if (markcfg && !name) setName(wizardDeviceName(markcfg.name, cor?.name ?? null));
  }, [markcfg, cor]);
  const create = async () => {
    if (!markcfg) return;
    setBusy(true);
    try {
      const body: Parameters<typeof libsApi.wizard>[0] = { markcfg_base64: await fileBase64(markcfg), activate };
      if (name.trim()) body.name = name.trim();
      if (cor) {
        body.cor_name = cor.name;
        body.cor_base64 = await fileBase64(cor);
      }
      const r = (await libsApi.wizard(body)) as { device?: { display_name?: string } };
      p.notify(t(activate ? "fml.createdActive" : "fml.created", { name: r?.device?.display_name ?? name }));
      p.onAdded();
      p.onClose();
    } catch (e) {
      p.report(new Error(errText(e)));
    } finally {
      setBusy(false);
    }
  };
  return (
    <WsModal
      help="devices-and-calibration#find-my-laser"
      title={t("fml.title")}
      wide
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || !markcfg} onClick={() => void create()}>
            {t("fml.create")}
          </button>
        </>
      }
    >
      <h4>{t("fml.step1")}</h4>
      <div className="lx-row">
        <button className="btn" disabled={scanning} onClick={doScan}>
          {scanning ? t("fml.scanning") : t("fml.scan")}
        </button>
      </div>
      {scan && !scan.usb && <p className="hint">{t("fml.noUsb")}</p>}
      {scan?.usb && scan.boards.length === 0 && <p className="hint">{t("fml.noBoards")}</p>}
      {scan && scan.boards.length > 0 && (
        <table className="lx-table">
          <thead>
            <tr>
              <th>{t("fml.board")}</th>
              <th>{t("fml.location")}</th>
              <th>{t("fml.serial")}</th>
              <th>{t("fml.status")}</th>
            </tr>
          </thead>
          <tbody>
            {scan.boards.map((b, i) => (
              <tr key={i}>
                <td>{`${hex4(b.vid)}:${hex4(b.pid)}`}</td>
                <td>{b.location}</td>
                <td>{b.serial ?? "—"}</td>
                <td>{b.supported ? t("fml.supported") : b.note || t("fml.unsupported")}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h4>{t("fml.step2")}</h4>
      <p className="hint">{t("fml.filesHint")}</p>
      <div className="lx-row">
        <FilePick label={markcfg ? t("fml.markcfgPicked", { name: markcfg.name }) : t("fml.markcfg")} accept="*/*" onFile={setMarkcfg} disabled={busy} />
        <FilePick label={cor ? t("fml.corPicked", { name: cor.name }) : t("fml.cor")} accept=".cor" onFile={setCor} disabled={busy} />
        {cor && (
          <button className="btn small" onClick={() => setCor(null)}>
            {t("fml.noCor")}
          </button>
        )}
      </div>
      <h4>{t("fml.step3")}</h4>
      <label className="field">
        <span className="field-label">{t("dev.displayName")}</span>
        <span className="field-input">
          <input type="text" value={name} maxLength={120} onChange={(e) => setName(e.target.value)} />
        </span>
      </label>
      <CheckField label={t("fml.activate")} checked={activate} onChange={setActivate} />
      <p className="hint">{t("fml.note")}</p>
    </WsModal>
  );
}
