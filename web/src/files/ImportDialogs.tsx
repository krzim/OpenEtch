// Import of the further formats (PDF, AI, EPS, HPGL/PLT, Gerber/Excellon, 3D models, TIFF/TGA, EzCad, legacy
// .lbrn): the options dialog LightBurn shows for 3D models (Import 3D Model, 2.2) and Gerber files, with the PDF page,
// HPGL precision and width for the others, and "Open as new project" for a single file. Plus LightBurn's Settings >
// Import/Export toggles (group, select, centre imported shapes, import hidden AI layers), kept in this browser. And
// Edit 3D Model, the same model options for a depth map whose model was kept (image.edit_model).

import { useState } from "react";
import { CheckField, Modal } from "../components/ui.tsx";
import { formatBytes } from "../api/project.ts";
import { useT } from "../workspace/usePrefs.ts";
import {
  DEFAULT_GERBER_OPTIONS,
  DEFAULT_MODEL_OPTIONS,
  engineFormatOf,
  formatGroup,
  formatLabel,
  takesKeepPosition,
  type EngineFormat,
  type GerberOptions,
  type ImportPrefs,
  type ModelOptions,
} from "./fileFormats.ts";
import { setImportPrefs, useFilesStore } from "./filesStore.ts";
import { MmBox, NumBox, Row, Segmented, parseBox, useBox } from "./FilesUiParts.tsx";

/** What the dialog hands back for each file. */
export interface ImportChoice {
  mode: "merge" | "replace";
  widthMm: number | null;
  page: number | null;
  unitsPerMm: number | null;
  gerber: GerberOptions;
  model: ModelOptions;
}

/** The import toggles (LightBurn Settings > Import/Export). */
export function ImportPrefsFields() {
  const t = useT();
  const p = useFilesStore().importPrefs;
  const set = (k: keyof ImportPrefs) => (v: boolean) => setImportPrefs({ [k]: v });
  return (
    <>
      <CheckField label={t("files.prefGroup")} checked={p.group} onChange={set("group")} />
      <CheckField label={t("files.prefSelect")} checked={p.select} onChange={set("select")} />
      <CheckField label={t("files.prefCenter")} checked={p.center} onChange={set("center")} />
      <CheckField label={t("files.prefHidden")} checked={p.hiddenLayers} onChange={set("hiddenLayers")} />
      <CheckField label={t("files.prefToolLayers")} checked={p.toolLayers} onChange={set("toolLayers")} />
    </>
  );
}

export function ImportOptionsDialog(p: { onClose: () => void }) {
  const t = useT();
  return (
    <Modal
      title={t("files.importOptionsTitle")}
      onClose={p.onClose}
      footer={
        <button className="btn btn-primary" onClick={p.onClose}>
          {t("common.close")}
        </button>
      }
    >
      <ImportPrefsFields />
      <p className="hint">{t("files.importOptionsHint")}</p>
    </Modal>
  );
}

export function ImportFilesDialog(p: {
  files: File[];
  /** Preselected: Open as new project (File > Open) or import into the design. */
  initialMode: "merge" | "replace";
  busy: boolean;
  onImport: (files: File[], c: ImportChoice) => void;
  onClose: () => void;
}) {
  const t = useT();
  const prefs = useFilesStore().importPrefs;
  const formats = p.files.map((f) => engineFormatOf(f.name)).filter((f): f is EngineFormat => f !== null);
  const groups = new Set(formats.map(formatGroup));
  const single = p.files.length === 1;
  const [mode, setMode] = useState<"merge" | "replace">(single ? p.initialMode : "merge");
  const [width, setWidth] = useBox(null);
  const [page, setPage] = useBox(null);
  const [units, setUnits] = useBox(40);
  const [gerber, setGerber] = useState<GerberOptions>(DEFAULT_GERBER_OPTIONS);
  const [model, setModel] = useState<ModelOptions>(DEFAULT_MODEL_OPTIONS);
  const [rx, setRx] = useBox(0);
  const [ry, setRy] = useBox(0);
  const [rz, setRz] = useBox(0);
  const [dpi, setDpi] = useBox(254);
  const [cutoff, setCutoff] = useBox(null);
  const n = {
    width: parseBox(width),
    page: parseBox(page),
    units: parseBox(units),
    rx: parseBox(rx),
    ry: parseBox(ry),
    rz: parseBox(rz),
    dpi: parseBox(dpi),
    cutoff: parseBox(cutoff),
  };
  const problems: string[] = [];
  if (n.width !== null && !(n.width > 0 && n.width <= 100000)) problems.push(t("files.badWidth"));
  if (groups.has("pdf") && n.page !== null && !(Number.isInteger(n.page) && n.page >= 1)) problems.push(t("files.badPage"));
  if (groups.has("hpgl") && !(n.units !== null && n.units > 0)) problems.push(t("files.badUnits"));
  if (groups.has("model")) {
    if ([n.rx, n.ry, n.rz].some((v) => v === null || !Number.isFinite(v))) problems.push(t("files.badRotation"));
    if (!(n.dpi !== null && n.dpi >= 10 && n.dpi <= 2540)) problems.push(t("files.badDpi"));
    if (n.cutoff !== null && !(n.cutoff > 0)) problems.push(t("files.badCutoff"));
  }
  const ok = problems.length === 0 && !p.busy && p.files.length > 0;
  const submit = () => {
    if (!ok) return;
    p.onImport(p.files, {
      mode: single ? mode : "merge",
      widthMm: n.width,
      page: n.page,
      unitsPerMm: n.units,
      gerber,
      model: { ...model, rotation: [n.rx ?? 0, n.ry ?? 0, n.rz ?? 0], dpi: n.dpi ?? 254, depthCutoffMm: n.cutoff, widthMm: n.width },
    });
  };
  const flag = (k: keyof ModelOptions) => (v: boolean) => setModel((m) => ({ ...m, [k]: v }));
  const keeps = formats.some(takesKeepPosition);
  return (
    <Modal
      title={groups.has("model") ? t("files.import3dTitle") : groups.has("gerber") ? t("files.importGerberTitle") : t("files.importTitle")}
      onClose={p.onClose}
      onSubmit={submit}
      wide={groups.has("model")}
      footer={
        <>
          {problems[0] && <span className="form-error">{problems[0]}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!ok} onClick={submit} autoFocus>
            {mode === "replace" && single ? t("files.openAsNew") : t("files.importBtn")}
          </button>
        </>
      }
    >
      <ul className="files-list">
        {p.files.map((f, i) => {
          const fmt = engineFormatOf(f.name);
          return (
            <li key={`${f.name}-${i}`}>
              <span className="files-list-name">{f.name}</span>
              <span className="muted">
                {fmt ? formatLabel(fmt) : "?"} · {formatBytes(f.size)}
              </span>
            </li>
          );
        })}
      </ul>
      <Segmented
        label={t("files.openOrImport")}
        value={single ? mode : "merge"}
        onChange={setMode}
        options={[
          { value: "merge", label: t("files.importIntoDesign") },
          { value: "replace", label: t("files.openAsNew"), disabled: !single, title: single ? t("files.openAsNewHint") : t("files.severalMerge") },
        ]}
      />
      {groups.has("model") ? (
        <>
          <div className="section-label">{t("files.model3d")}</div>
          <div className="files-grid3">
            <NumBox label={t("files.rotX")} value={rx} onChange={setRx} unit="°" />
            <NumBox label={t("files.rotY")} value={ry} onChange={setRy} unit="°" />
            <NumBox label={t("files.rotZ")} value={rz} onChange={setRz} unit="°" />
          </div>
          <MmBox label={t("files.width")} value={width} onChange={setWidth} title={t("files.widthHint")} />
          <NumBox label={t("files.dpi")} value={dpi} onChange={setDpi} title={t("files.dpiHint")} />
          <MmBox label={t("files.depthCutoff")} value={cutoff} onChange={setCutoff} title={t("files.depthCutoffHint")} />
          <CheckField label={t("files.sixteenBit")} checked={model.sixteenBit} onChange={flag("sixteenBit")} />
          <CheckField label={t("files.invertDepth")} checked={model.invert} onChange={flag("invert")} />
          <CheckField label={t("files.transparentBg")} checked={model.transparentBackground} onChange={flag("transparentBackground")} />
          <CheckField label={t("files.equalize")} checked={model.equalize} onChange={flag("equalize")} />
          <CheckField label={t("files.outline")} checked={model.outline} onChange={flag("outline")} />
          <p className="hint">{t("files.model3dHint")}</p>
        </>
      ) : (
        <MmBox label={t("files.width")} value={width} onChange={setWidth} title={t("files.widthHint")} />
      )}
      {groups.has("gerber") && (
        <>
          <div className="section-label">{t("files.gerber")}</div>
          <Row label={t("files.gerberKind")}>
            <select value={gerber.kind} onChange={(e) => setGerber((g) => ({ ...g, kind: e.target.value as GerberOptions["kind"] }))}>
              <option value="auto">{t("files.gerberAuto")}</option>
              <option value="gerber">{t("files.gerberCopper")}</option>
              <option value="excellon">{t("files.gerberDrill")}</option>
            </select>
          </Row>
          <CheckField label={t("files.gerberInvert")} checked={gerber.invert} onChange={(invert) => setGerber((g) => ({ ...g, invert }))} />
          <p className="hint">{t("files.gerberHint")}</p>
        </>
      )}
      {groups.has("pdf") && (
        <>
          <NumBox label={t("files.pdfPage")} value={page} onChange={setPage} placeholder="1" />
          <CheckField label={t("files.prefHidden")} checked={prefs.hiddenLayers} onChange={(v) => setImportPrefs({ hiddenLayers: v })} />
        </>
      )}
      {groups.has("hpgl") && <NumBox label={t("files.hpglUnits")} value={units} onChange={setUnits} title={t("files.hpglUnitsHint")} />}
      <details className="files-details">
        <summary>{t("files.importOptionsTitle")}</summary>
        <ImportPrefsFields />
      </details>
      {!keeps && !prefs.center && <p className="hint">{t("files.alwaysCentred")}</p>}
    </Modal>
  );
}

/**
 * LightBurn 2.2's Edit 3D Model: the Import 3D model dialog again for a depth map made from a kept model, with the
 * options it was rendered with; OK renders it again in place (`image.edit_model`, one undo step, outline redrawn).
 */
export function EditModelDialog(p: { name: string; initial: ModelOptions; busy: boolean; onApply: (o: ModelOptions) => void; onClose: () => void }) {
  const t = useT();
  const i = p.initial;
  const [model, setModel] = useState<ModelOptions>(i);
  const [width, setWidth] = useBox(i.widthMm);
  const [rx, setRx] = useBox(i.rotation[0]);
  const [ry, setRy] = useBox(i.rotation[1]);
  const [rz, setRz] = useBox(i.rotation[2]);
  const [dpi, setDpi] = useBox(i.dpi);
  const [cutoff, setCutoff] = useBox(i.depthCutoffMm);
  const n = { width: parseBox(width), rx: parseBox(rx), ry: parseBox(ry), rz: parseBox(rz), dpi: parseBox(dpi), cutoff: parseBox(cutoff) };
  const problems: string[] = [];
  if (n.width !== null && !(n.width > 0 && n.width <= 100000)) problems.push(t("files.badWidth"));
  if ([n.rx, n.ry, n.rz].some((v) => v === null || !Number.isFinite(v))) problems.push(t("files.badRotation"));
  if (!(n.dpi !== null && n.dpi >= 10 && n.dpi <= 2540)) problems.push(t("files.badDpi"));
  if (n.cutoff !== null && !(n.cutoff > 0)) problems.push(t("files.badCutoff"));
  const ok = problems.length === 0 && !p.busy;
  const submit = () => {
    if (!ok) return;
    p.onApply({ ...model, rotation: [n.rx ?? 0, n.ry ?? 0, n.rz ?? 0], dpi: n.dpi ?? 254, depthCutoffMm: n.cutoff, widthMm: n.width });
  };
  const flag = (k: keyof ModelOptions) => (v: boolean) => setModel((m) => ({ ...m, [k]: v }));
  return (
    <Modal
      title={t("files.editModelTitle", { name: p.name })}
      onClose={p.onClose}
      onSubmit={submit}
      wide
      footer={
        <>
          {problems[0] && <span className="form-error">{problems[0]}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!ok} onClick={submit} autoFocus>
            {t("files.editModelApply")}
          </button>
        </>
      }
    >
      <div className="files-grid3">
        <NumBox label={t("files.rotX")} value={rx} onChange={setRx} unit="°" />
        <NumBox label={t("files.rotY")} value={ry} onChange={setRy} unit="°" />
        <NumBox label={t("files.rotZ")} value={rz} onChange={setRz} unit="°" />
      </div>
      <MmBox label={t("files.width")} value={width} onChange={setWidth} title={t("files.widthHint")} />
      <NumBox label={t("files.dpi")} value={dpi} onChange={setDpi} title={t("files.dpiHint")} />
      <MmBox label={t("files.depthCutoff")} value={cutoff} onChange={setCutoff} title={t("files.depthCutoffHint")} />
      <CheckField label={t("files.sixteenBit")} checked={model.sixteenBit} onChange={flag("sixteenBit")} />
      <CheckField label={t("files.invertDepth")} checked={model.invert} onChange={flag("invert")} />
      <CheckField label={t("files.transparentBg")} checked={model.transparentBackground} onChange={flag("transparentBackground")} />
      <CheckField label={t("files.equalize")} checked={model.equalize} onChange={flag("equalize")} />
      <CheckField label={t("files.outline")} checked={model.outline} onChange={flag("outline")} />
      <p className="hint">{t("files.editModelHint")}</p>
    </Modal>
  );
}
