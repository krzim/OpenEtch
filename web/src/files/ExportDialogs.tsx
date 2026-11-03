// LightBurn's File > Export (Alt+X) and File > Print. Export writes SVG, DXF, AI (Illustrator 9+, PDF-compatible) or
// PDF of the selection, or of the whole project when nothing is selected (`doc.export`, GET /api/export). Print offers
// LightBurn's "black only" and "keep colours": Print opens the browser's print dialog on the engine's print SVG at
// its physical size (1:1), and Save PDF downloads `doc.print`'s PDF, scaled to fit a paper size when one is chosen.

import { useState } from "react";
import { CheckField, Modal } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import {
  DEFAULT_EXPORT,
  DEFAULT_PRINT,
  PAPER_SIZES,
  exportQuery,
  printPageHtml,
  printQuery,
  type ExportFormat,
  type PrintOptions,
} from "./fileFormats.ts";
import { filesApi, saveBlob } from "./filesApi.ts";
import { MmBox, Row, Segmented, parseBox, useBox } from "./FilesUiParts.tsx";

type Notify = (msg: string, kind?: "info" | "error") => void;

interface Common {
  /** The selected object ids (exports the selection when not empty and "Selection" is chosen). */
  selection: string[];
  baseName: string;
  onClose: () => void;
  notify: Notify;
  report: (e: unknown) => void;
}

export function ExportDialog(p: Common) {
  const t = useT();
  const [format, setFormat] = useState<ExportFormat>("svg");
  const [scope, setScope] = useState<"selection" | "all">(p.selection.length ? "selection" : "all");
  const [colors, setColors] = useState<"layers" | "black">("layers");
  const [area, setArea] = useState<"bounds" | "workspace">("bounds");
  const [stroke, setStroke] = useBox(DEFAULT_EXPORT.strokeMm);
  const [tolerance, setTolerance] = useBox(DEFAULT_EXPORT.toleranceMm);
  const [images, setImages] = useState(true);
  const [keepCircles, setKeepCircles] = useState(true);
  const [busy, setBusy] = useState(false);
  const strokeMm = parseBox(stroke);
  const tolMm = parseBox(tolerance);
  const bad = format === "dxf" ? !(tolMm !== null && tolMm > 0 && tolMm <= 10) : !(strokeMm !== null && strokeMm > 0 && strokeMm <= 10);
  const go = () => {
    if (bad || busy) return;
    setBusy(true);
    const q = exportQuery({
      ...DEFAULT_EXPORT,
      format,
      ids: scope === "selection" ? p.selection : [],
      colors,
      area,
      strokeMm: strokeMm ?? DEFAULT_EXPORT.strokeMm,
      toleranceMm: tolMm ?? DEFAULT_EXPORT.toleranceMm,
      images,
      keepCircles,
    });
    filesApi
      .exportFile(q)
      .then((d) => {
        const name = d.filename ?? `${p.baseName}.${format}`;
        saveBlob(d.blob, name);
        p.notify(d.warnings ? t("files.exportedWarn", { name, n: d.warnings }) : t("files.exported", { name }));
        p.onClose();
      })
      .catch(p.report)
      .finally(() => setBusy(false));
  };
  return (
    <Modal
      help="files-and-projects#export-svg-dxf-ai-pdf"
      title={t("files.exportTitle")}
      onClose={p.onClose}
      onSubmit={go}
      footer={
        <>
          {bad && <span className="form-error">{format === "dxf" ? t("files.badTolerance") : t("files.badStroke")}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={bad || busy} onClick={go} autoFocus>
            {t("files.exportBtn")}
          </button>
        </>
      }
    >
      <div className="section-label">{t("files.format")}</div>
      <Segmented
        label={t("files.format")}
        value={format}
        onChange={setFormat}
        options={[
          { value: "svg", label: "SVG" },
          { value: "dxf", label: "DXF" },
          { value: "ai", label: "AI", title: t("files.aiHint") },
          { value: "pdf", label: "PDF" },
        ]}
      />
      <div className="section-label">{t("files.what")}</div>
      <Segmented
        label={t("files.what")}
        value={scope}
        onChange={setScope}
        options={[
          { value: "selection", label: t("files.selectionN", { n: p.selection.length }), disabled: p.selection.length === 0 },
          { value: "all", label: t("files.wholeProject") },
        ]}
      />
      <div className="section-label">{t("files.colours")}</div>
      <Segmented
        label={t("files.colours")}
        value={colors}
        onChange={setColors}
        options={[
          { value: "layers", label: t("files.layerColours") },
          { value: "black", label: t("files.blackOnly") },
        ]}
      />
      <div className="section-label">{t("files.page")}</div>
      <Segmented
        label={t("files.page")}
        value={area}
        onChange={setArea}
        options={[
          { value: "bounds", label: t("files.designBounds") },
          { value: "workspace", label: t("files.workspaceArea") },
        ]}
      />
      {format === "dxf" ? (
        <>
          <CheckField label={t("files.keepCircles")} checked={keepCircles} onChange={setKeepCircles} />
          <MmBox label={t("files.tolerance")} value={tolerance} onChange={setTolerance} title={t("files.toleranceHint")} />
          <p className="hint">{t("files.dxfHint")}</p>
        </>
      ) : (
        <>
          <MmBox label={t("files.stroke")} value={stroke} onChange={setStroke} />
          <CheckField label={t("files.embedImages")} checked={images} onChange={setImages} />
          {format === "ai" && <p className="hint">{t("files.aiHint")}</p>}
        </>
      )}
      <p className="hint">{t("files.exportHint")}</p>
    </Modal>
  );
}

/** Print the engine's SVG through a hidden frame, so the browser's print dialog opens on it alone. */
async function printSvg(blob: Blob, title: string): Promise<void> {
  const url = await new Promise<string>((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error ?? new Error("cannot read the print file"));
    r.readAsDataURL(blob);
  });
  const frame = document.createElement("iframe");
  frame.setAttribute("aria-hidden", "true");
  frame.style.cssText = "position:fixed;right:0;bottom:0;width:0;height:0;border:0;visibility:hidden";
  // The frame shows only an <img> of the SVG data URL; it runs no script of its own.
  frame.setAttribute("sandbox", "allow-modals allow-same-origin");
  frame.srcdoc = printPageHtml(url, title);
  document.body.appendChild(frame);
  await new Promise<void>((resolve) => {
    frame.onload = () => resolve();
  });
  const img = frame.contentDocument?.querySelector("img");
  if (img && !img.complete) await new Promise((r) => img.addEventListener("load", r, { once: true }));
  frame.contentWindow?.focus();
  frame.contentWindow?.print();
  window.setTimeout(() => frame.remove(), 60_000);
}

export function PrintDialog(p: Common & { colors?: PrintOptions["colors"] }) {
  const t = useT();
  // LightBurn: Print (black only, Ctrl+P) and Print (keep colours, Ctrl+Shift+P) open with that choice.
  const [colors, setColors] = useState<PrintOptions["colors"]>(p.colors ?? "black");
  const [width, setWidth] = useBox(DEFAULT_PRINT.lineWidthMm);
  const [paper, setPaper] = useState("actual");
  const [margin, setMargin] = useBox(DEFAULT_PRINT.marginMm);
  const [area, setArea] = useState<PrintOptions["area"]>("workspace");
  const [selOnly, setSelOnly] = useState(false);
  const [busy, setBusy] = useState(false);
  const lw = parseBox(width);
  const mg = parseBox(margin);
  const bad = !(lw !== null && lw > 0 && lw <= 10) || !(mg !== null && mg >= 0 && mg < 100);
  const opts = (format: "pdf" | "svg"): PrintOptions => ({
    format,
    colors,
    lineWidthMm: lw ?? DEFAULT_PRINT.lineWidthMm,
    paper: format === "pdf" ? (PAPER_SIZES.find((x) => x.id === paper)?.mm ?? null) : null,
    marginMm: mg ?? DEFAULT_PRINT.marginMm,
    area,
    ids: selOnly ? p.selection : [],
  });
  const run = (format: "pdf" | "svg") => {
    if (bad || busy) return;
    setBusy(true);
    filesApi
      .printFile(printQuery(opts(format)))
      .then(async (d) => {
        if (format === "pdf") {
          const name = d.filename ?? `${p.baseName}.pdf`;
          saveBlob(d.blob, name);
          p.notify(t("files.printSaved", { name }));
        } else {
          await printSvg(d.blob, p.baseName);
        }
        p.onClose();
      })
      .catch(p.report)
      .finally(() => setBusy(false));
  };
  return (
    <Modal
      help="files-and-projects#print"
      title={t("files.printTitle")}
      onClose={p.onClose}
      footer={
        <>
          {bad && <span className="form-error">{t("files.badPrint")}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn" disabled={bad || busy} onClick={() => run("pdf")} title={t("files.savePdfHint")}>
            {t("files.savePdf")}
          </button>
          <button className="btn btn-primary" disabled={bad || busy} onClick={() => run("svg")} autoFocus>
            {t("files.printBtn")}
          </button>
        </>
      }
    >
      <Segmented
        label={t("files.colours")}
        value={colors}
        onChange={setColors}
        options={[
          { value: "black", label: t("files.printBlack") },
          { value: "layers", label: t("files.printColours") },
        ]}
      />
      <MmBox label={t("files.lineWidth")} value={width} onChange={setWidth} />
      <div className="section-label">{t("files.page")}</div>
      <Segmented
        label={t("files.page")}
        value={area}
        onChange={setArea}
        options={[
          { value: "workspace", label: t("files.workspaceArea") },
          { value: "bounds", label: t("files.designBounds") },
        ]}
      />
      <CheckField label={t("files.selectionOnly")} checked={selOnly && p.selection.length > 0} onChange={(v) => setSelOnly(v && p.selection.length > 0)} />
      <Row label={t("files.paper")} title={t("files.paperHint")}>
        <select value={paper} onChange={(e) => setPaper(e.target.value)}>
          {PAPER_SIZES.map((s) => (
            <option key={s.id} value={s.id}>
              {s.id === "actual" ? t("files.paperActual") : s.label}
            </option>
          ))}
        </select>
      </Row>
      {paper !== "actual" && <MmBox label={t("files.margin")} value={margin} onChange={setMargin} />}
      <p className="hint">{t("files.printHint")}</p>
    </Modal>
  );
}
