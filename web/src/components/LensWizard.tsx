// LightBurn's Calibrate Galvo Lens, on the engine's lens calibration routes: (1) add the test pattern (a square with
// centre lines and diagonals) to the design (POST /api/calibration/pattern); (2) mark it: "Request marking" creates a
// pending run of only the pattern, which still needs the operator PIN in the Laser window (nothing fires from here);
// (3) measure the marked lines with calipers; (4) solve (POST /api/calibration/solve) and compare the current and new
// corrections; (5) apply them to the active device (UI only; the built-in device is read-only). Repeat to refine.

import { useState } from "react";
import { ApiError } from "../api/client.ts";
import type { DeviceSummary } from "../api/devices.ts";
import { runRequestBody, DEFAULT_RUN_OPTIONS } from "../api/job.ts";
import { jobApi } from "../api/jobApi.ts";
import type { GDocument } from "../api/types.ts";
import { useT } from "../workspace/usePrefs.ts";
import { Modal, NumberField } from "./ui.tsx";
import "./job.css";
import { errorText } from "../api/errorText.ts";

/** The measurements the engine solves from, in the order the wizard asks for them. */
export const LENS_MEASURES = ["top_mm", "bottom_mm", "middle_x_mm", "left_mm", "right_mm", "middle_y_mm", "diagonal_up_mm", "diagonal_down_mm"] as const;
export type LensMeasure = (typeof LENS_MEASURES)[number];

/**
 * Optional corner shifts that tell the X and Y skew apart (the diagonals only give their sum): the top-left corner's
 * X offset from the bottom-left one, and the bottom-right corner's Y offset from the bottom-left one, each measured
 * against a straight edge along the machine's axis. Signed; an empty field is left out.
 */
export const LENS_SHIFTS = ["top_shift_mm", "right_shift_mm"] as const;
export type LensShift = (typeof LENS_SHIFTS)[number];

/** The measurements to send: the eight lines plus the corner shifts that were entered. */
export function lensMeasurements(meas: Partial<Record<LensMeasure | LensShift, number>>): Record<string, number> {
  const out: Record<string, number> = {};
  for (const k of [...LENS_MEASURES, ...LENS_SHIFTS]) {
    const v = meas[k];
    if (typeof v === "number" && Number.isFinite(v)) out[k] = v;
  }
  return out;
}

/** The expected length of each measured line for a pattern of `size` mm. */
export function expectedLength(m: LensMeasure, size: number): number {
  return m.startsWith("diagonal") ? size * Math.SQRT2 : size;
}

/** Calibration keys shown in the comparison, in LightBurn's order. */
const CAL_KEYS = ["offset_x_mm", "offset_y_mm", "angle_deg", "scale_x", "scale_y", "bulge_x", "bulge_y", "skew_x", "skew_y", "trapezoid_x", "trapezoid_y"];

type Step = "pattern" | "mark" | "measure" | "result";

export function LensWizard(p: {
  doc: GDocument | null;
  device: DeviceSummary;
  notify: (msg: string, kind?: "info" | "error") => void;
  report: (e: unknown) => void;
  onApplied: () => void;
  onClose: () => void;
}) {
  const t = useT();
  const [step, setStep] = useState<Step>("pattern");
  const [size, setSize] = useState(() => Math.round(Math.min(100, (p.device.workspace_mm[0] || 150) * 0.65)));
  const [layer, setLayer] = useState<string>(p.doc?.layers[0]?.id ?? "");
  const [created, setCreated] = useState<string[]>([]);
  const [meas, setMeas] = useState<Partial<Record<LensMeasure | LensShift, number>>>({});
  const [solved, setSolved] = useState<{ current: Record<string, number>; calibration: Record<string, number> } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fail = (e: unknown) => setError(errorText(e));

  const addPattern = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = (await jobApi.calibrationPattern(size, layer || null)) as { created?: unknown[]; group?: { created?: unknown[] } };
      const ids = (r.created ?? []).filter((x): x is string => typeof x === "string");
      setCreated(ids);
      p.notify(t("lens.added"));
      setStep("mark");
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };
  const requestMark = async () => {
    setBusy(true);
    setError(null);
    try {
      await jobApi.requestRun(runRequestBody({ ...DEFAULT_RUN_OPTIONS, cutSelected: true }, { selection: created }));
      p.notify(t("lens.requested"));
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };
  const complete = LENS_MEASURES.every((m) => typeof meas[m] === "number" && (meas[m] as number) > 0);
  const solve = async (apply: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const r = (await jobApi.calibrationSolve(size, lensMeasurements(meas), apply)) as {
        current?: Record<string, number>;
        calibration?: Record<string, number>;
        applied?: boolean;
      };
      setSolved({ current: r.current ?? {}, calibration: r.calibration ?? {} });
      if (apply && r.applied) {
        p.notify(t("lens.applied"));
        p.onApplied();
      } else setStep("result");
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) setError(`${e.detail ?? e.message} ${t("lens.duplicateHint")}`);
      else fail(e);
    } finally {
      setBusy(false);
    }
  };
  const steps: { v: Step; label: string }[] = [
    { v: "pattern", label: t("lens.step1") },
    { v: "mark", label: t("lens.step2") },
    { v: "measure", label: t("lens.step3") },
    { v: "result", label: t("lens.step4") },
  ];
  return (
    <Modal
      help="devices-and-calibration#lens-calibration"
      title={t("lens.title")}
      wide
      onClose={p.onClose}
      footer={
        <>
          {step !== "pattern" && (
            <button className="btn" onClick={() => setStep(steps[steps.findIndex((s) => s.v === step) - 1].v)}>
              {t("lens.back")}
            </button>
          )}
          {step === "pattern" && (
            <>
              <button className="btn" onClick={() => setStep("measure")} title={t("lens.skipTitle")}>
                {t("lens.skip")}
              </button>
              <button className="btn btn-primary" disabled={busy || !(size >= 5)} onClick={() => void addPattern()}>
                {t("lens.addPattern")}
              </button>
            </>
          )}
          {step === "mark" && (
            <button className="btn btn-primary" onClick={() => setStep("measure")}>
              {t("lens.next")}
            </button>
          )}
          {step === "measure" && (
            <button className="btn btn-primary" disabled={busy || !complete} onClick={() => void solve(false)}>
              {t("lens.solve")}
            </button>
          )}
          {step === "result" && (
            <button className="btn btn-danger" disabled={busy || !solved} onClick={() => void solve(true)} title={t("lens.applyTitle")}>
              {t("lens.apply")}
            </button>
          )}
          <button className="btn" onClick={p.onClose}>
            {t("common.close")}
          </button>
        </>
      }
    >
      <ol className="lens-steps">
        {steps.map((s) => (
          <li key={s.v} className={s.v === step ? "active" : ""}>
            {s.label}
          </li>
        ))}
      </ol>
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {step === "pattern" && (
        <>
          <p>{t("lens.patternText", { device: p.device.display_name })}</p>
          <NumberField label={t("lens.size")} unit="mm" value={size} min={5} max={(p.device.workspace_mm[0] || 150) * 0.94} digits={1} onCommit={(v) => v !== null && setSize(v)} />
          <label className="field">
            <span className="field-label">{t("lens.layer")}</span>
            <span className="field-input">
              <select value={layer} onChange={(e) => setLayer(e.target.value)}>
                {(p.doc?.layers ?? []).map((l) => (
                  <option key={l.id} value={l.id}>
                    {l.name}
                  </option>
                ))}
              </select>
            </span>
          </label>
          <p className="hint">{t("lens.layerHint")}</p>
        </>
      )}
      {step === "mark" && (
        <>
          <p>{t("lens.markText")}</p>
          <button className="btn" disabled={busy || created.length === 0} onClick={() => void requestMark()}>
            {t("lens.requestMark")}
          </button>
          <p className="hint">{t("lens.markHint")}</p>
        </>
      )}
      {step === "measure" && (
        <div className="lens-measure">
          <LensDiagram />
          <div>
            <p className="hint">{t("lens.measureText", { size })}</p>
            {LENS_MEASURES.map((m, i) => (
              <NumberField
                key={m}
                label={`${String.fromCharCode(65 + i)}. ${t(`lens.m.${m}` as "lens.m.top_mm")}`}
                unit="mm"
                value={meas[m] ?? null}
                min={0.1}
                digits={3}
                placeholder={String(Number(expectedLength(m, size).toFixed(3)))}
                onCommit={(v) => setMeas((x) => ({ ...x, [m]: v ?? undefined }))}
              />
            ))}
            <fieldset className="job-fieldset lens-shifts">
              <legend>{t("lens.shifts")}</legend>
              <p className="hint">{t("lens.shiftsHint")}</p>
              {LENS_SHIFTS.map((m, i) => (
                <NumberField
                  key={m}
                  label={`${String.fromCharCode(73 + i)}. ${t(`lens.m.${m}` as "lens.m.top_mm")}`}
                  unit="mm"
                  value={meas[m] ?? null}
                  min={-size * 0.2}
                  max={size * 0.2}
                  digits={3}
                  allowEmpty
                  placeholder={t("lens.optional")}
                  onCommit={(v) => setMeas((x) => ({ ...x, [m]: v ?? undefined }))}
                />
              ))}
            </fieldset>
          </div>
        </div>
      )}
      {step === "result" && solved && (
        <>
          <table className="lb-diff-table lens-table">
            <thead>
              <tr>
                <th scope="col">{t("lens.setting")}</th>
                <th scope="col">{t("lens.current")}</th>
                <th scope="col">{t("lens.new")}</th>
              </tr>
            </thead>
            <tbody>
              {CAL_KEYS.map((k) => {
                const a = solved.current[k];
                const b = solved.calibration[k];
                const changed = typeof a === "number" && typeof b === "number" && Math.abs(a - b) > 1e-9;
                return (
                  <tr key={k} className={changed ? "changed" : ""}>
                    <td>{k}</td>
                    <td>{typeof a === "number" ? Number(a.toFixed(6)) : "–"}</td>
                    <td>{typeof b === "number" ? Number(b.toFixed(6)) : "–"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          <p className="hint">{t("lens.resultHint")}</p>
        </>
      )}
    </Modal>
  );
}

/** The test pattern with its measured lines lettered A-H (the order of the fields). */
function LensDiagram() {
  const s = 120;
  const o = 20;
  const lab = (x: number, y: number, txt: string) => (
    <text x={x} y={y} className="lens-lab" textAnchor="middle" dominantBaseline="middle">
      {txt}
    </text>
  );
  return (
    <svg className="lens-diagram" viewBox={`0 0 ${s + 2 * o} ${s + 2 * o}`} role="img" aria-label="Test pattern: A top, B bottom, C middle horizontal, D left, E right, F middle vertical, G diagonal up, H diagonal down, I top-left corner shift, J bottom-right corner shift">
      <rect x={o} y={o} width={s} height={s} className="lens-line" />
      <line x1={o} y1={o + s / 2} x2={o + s} y2={o + s / 2} className="lens-line" />
      <line x1={o + s / 2} y1={o} x2={o + s / 2} y2={o + s} className="lens-line" />
      <line x1={o} y1={o + s} x2={o + s} y2={o} className="lens-line thin" />
      <line x1={o} y1={o} x2={o + s} y2={o + s} className="lens-line thin" />
      {lab(o + s / 2, o - 9, "A")}
      {lab(o + s / 2, o + s + 9, "B")}
      {lab(o + s / 4, o + s / 2 - 7, "C")}
      {lab(o - 9, o + s / 2, "D")}
      {lab(o + s + 9, o + s / 2, "E")}
      {lab(o + s / 2 + 8, o + s / 4, "F")}
      {lab(o + (s * 3) / 4 + 8, o + s / 4 + 4, "G")}
      {lab(o + (s * 3) / 4 + 8, o + (s * 3) / 4 - 4, "H")}
      {/* I and J: the optional corner shifts against a straight edge along the machine's Y and X axes. */}
      <line x1={o} y1={o + s + 4} x2={o} y2={o - 6} className="lens-ref" />
      <line x1={o - 6} y1={o + s} x2={o + s + 6} y2={o + s} className="lens-ref" />
      {lab(o - 9, o - 2, "I")}
      {lab(o + s + 4, o + s + 11, "J")}
    </svg>
  );
}
