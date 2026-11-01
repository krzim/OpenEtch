// LightBurn's Laser Tools > Material Test, Interval Test and Focus Test (engine `generate.*`, one undo step each): the
// options of LightBurn's dialogs (two tested parameters with Count / Min / Max, box size and spacing, centre, the base
// material setting, labels, border, title; interval range and fill; Z range and steps), a live preview of the layout
// in field mm with LightBurn's safe burn order numbered, then Create (the test is added and selected) or Create and
// request run (a pending run of just the test: it still needs the operator PIN in the Runs card).

import { useMemo, useState, type ReactNode } from "react";
import type { Invocation } from "../api/client.ts";
import { createdIds } from "../api/drop.ts";
import { frequencyRange, pulseWidths } from "../api/laser.ts";
import type { MachineStatus, Vec2 } from "../api/types.ts";
import { CheckField, NumberField, PairField } from "../components/ui.tsx";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { libsApi } from "./api.ts";
import {
  DEFAULT_FOCUS_TEST,
  DEFAULT_INTERVAL_TEST,
  DEFAULT_MATERIAL_TEST,
  PARAMS,
  PARAM_DEFAULT_RANGE,
  focusTestParams,
  focusTestPreview,
  focusTestProblem,
  intervalTestParams,
  intervalTestPreview,
  intervalTestProblem,
  materialTestParams,
  materialTestPreview,
  materialTestProblem,
  previewViewBox,
  show,
  type Axis,
  type BaseSettings,
  type FocusTest,
  type IntervalTest,
  type MaterialTest,
  type TestParam,
  type TestPreview,
} from "./testgen.ts";
import { useActiveDevice } from "./store.ts";
import { errText } from "./ui.tsx";
import type { T } from "../workspace/i18n.ts";
import "./libs.css";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
type Notify = (msg: string, kind?: "info" | "error") => void;

export interface GenProps {
  machine: MachineStatus | null;
  /** Centre of the selection, if any (Use selection centre). */
  selectionCentre: Vec2 | null;
  run: Run;
  notify: Notify;
  report: (e: unknown) => void;
  /** The created objects become the selection. */
  onCreated: (ids: string[]) => void;
  /** Why this device cannot run jobs, or null. */
  designOnly: string | null;
  onClose: () => void;
}

const STORE = "openetch.testgen";

function load<T>(key: string, d: T): T {
  try {
    const all = JSON.parse(localStorage.getItem(STORE) ?? "{}") as Record<string, unknown>;
    const v = all[key];
    return v && typeof v === "object" ? ({ ...d, ...(v as object) } as T) : d;
  } catch {
    return d;
  }
}

function save(key: string, v: unknown): void {
  try {
    const all = JSON.parse(localStorage.getItem(STORE) ?? "{}") as Record<string, unknown>;
    all[key] = v;
    localStorage.setItem(STORE, JSON.stringify(all));
  } catch {
    // Storage unavailable: the values last for this visit.
  }
}

/** Create the test; with `request`, also a pending run of just the test (it still needs the PIN). */
async function create(p: GenProps, t: T, id: string, params: Record<string, unknown>, request: boolean, what: string): Promise<boolean> {
  try {
    const r = await p.run({ id, params });
    const ids = createdIds(r);
    p.onCreated(ids);
    if (request && ids.length) {
      await libsApi.requestRun({ origin: "ui", selection: ids });
      p.notify(t("ltest.addedRun", { what }));
    } else p.notify(t("ltest.added", { what }));
    return true;
  } catch (e) {
    p.report(new Error(errText(e)));
    return false;
  }
}

function GenDialog(p: {
  title: string;
  props: GenProps;
  problem: string | null;
  preview: TestPreview;
  onCreate: (request: boolean) => Promise<boolean>;
  onReset: () => void;
  children: ReactNode;
}) {
  const t = useT();
  const [busy, setBusy] = useState(false);
  const go = async (request: boolean) => {
    setBusy(true);
    const ok = await p.onCreate(request);
    setBusy(false);
    if (ok) p.props.onClose();
  };
  return (
    <WsModal
      title={p.title}
      wide
      onClose={p.props.onClose}
      footer={
        <>
          <button className="btn" onClick={p.onReset}>
            {t("ltest.defaults")}
          </button>
          <button className="btn" onClick={p.props.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn" disabled={busy || !!p.problem || !!p.props.designOnly} title={p.props.designOnly ?? t("ltest.createRunTitle")} onClick={() => void go(true)}>
            {t("ltest.createRun")}
          </button>
          <button className="btn btn-primary" disabled={busy || !!p.problem} onClick={() => void go(false)}>
            {t("ltest.create")}
          </button>
        </>
      }
    >
      <div className="lx-gen">
        <div className="lx-gen-form">{p.children}</div>
        <div className="lx-gen-preview">
          <TestPreviewSvg preview={p.preview} />
          {p.problem ? <p className="form-error">{p.problem}</p> : <p className="hint">{t("ltest.previewHint")}</p>}
        </div>
      </div>
    </WsModal>
  );
}

/** The layout in field mm (+Y up), boxes shaded by how hard they burn and numbered in burn order. */
export function TestPreviewSvg({ preview: pv }: { preview: TestPreview }) {
  const { units } = usePrefs();
  const vb = previewViewBox(pv);
  const w = pv.bounds[2] - pv.bounds[0];
  const h = pv.bounds[3] - pv.bounds[1];
  const few = pv.boxes.length <= 120;
  const unit = units === "in" ? 25.4 : 1;
  return (
    <figure className="lx-preview">
      <svg viewBox={vb} preserveAspectRatio="xMidYMid meet" role="img" aria-label="Test layout preview">
        <g transform="scale(1,-1)">
          {pv.border && <rect className="lx-pv-border" x={pv.border.x} y={pv.border.y} width={pv.border.w} height={pv.border.h} />}
          {pv.boxes.map((b, i) => (
            <rect key={i} className="lx-pv-box" x={b.x} y={b.y} width={b.w} height={b.h} style={{ fillOpacity: 0.15 + 0.75 * b.shade }}>
              <title>{`${b.order}. ${b.label}`}</title>
            </rect>
          ))}
          {pv.lines.map((l, i) => (
            <line key={i} className="lx-pv-line" x1={l.x1} y1={l.y1} x2={l.x2} y2={l.y2}>
              <title>{l.label}</title>
            </line>
          ))}
        </g>
        {few &&
          pv.boxes.map((b, i) => (
            <text key={i} className="lx-pv-order" x={b.x + b.w / 2} y={-(b.y + b.h / 2)} fontSize={Math.min(b.w, b.h) * 0.42} textAnchor="middle" dominantBaseline="central">
              {b.order}
            </text>
          ))}
        {pv.texts.map((tx, i) => (
          <text
            key={i}
            className="lx-pv-label"
            x={tx.x}
            y={-tx.y}
            fontSize={tx.size}
            textAnchor={tx.anchor}
            dominantBaseline="central"
            transform={tx.rotate ? `rotate(${-tx.rotate} ${tx.x} ${-tx.y})` : undefined}
          >
            {tx.text}
          </text>
        ))}
      </svg>
      <figcaption className="muted">{`${show(w / unit)} × ${show(h / unit)} ${units === "in" ? "in" : "mm"}`}</figcaption>
    </figure>
  );
}

function CentreField(p: { value: [number, number]; onChange: (v: [number, number]) => void; selection: Vec2 | null }) {
  const t = useT();
  return (
    <>
      <PairField labels={[t("ltest.centreX"), t("ltest.centreY")]} unit="mm" value={p.value} onCommit={p.onChange} />
      <div className="lx-row">
        <button className="btn small" onClick={() => p.onChange([0, 0])}>
          {t("ltest.fieldCentre")}
        </button>
        <button className="btn small" disabled={!p.selection} onClick={() => p.selection && p.onChange([p.selection[0], p.selection[1]])}>
          {t("ltest.selectionCentre")}
        </button>
      </div>
    </>
  );
}

function PulseField(p: { machine: MachineStatus | null; value: number | null; onChange: (v: number | null) => void }) {
  const t = useT();
  const widths = pulseWidths(p.machine);
  return (
    <label className="field">
      <span className="field-label">{t("ltest.pulse")}</span>
      <span className="field-input">
        <select value={p.value === null ? "" : String(p.value)} onChange={(e) => p.onChange(e.target.value === "" ? null : Number(e.target.value))}>
          <option value="">{t("ltest.sourceDefault")}</option>
          {widths.map((v) => (
            <option key={v} value={String(v)}>
              {v} ns
            </option>
          ))}
        </select>
      </span>
    </label>
  );
}

const unitOf = (p: TestParam) => PARAMS.find((x) => x.value === p)?.unit || undefined;

function AxisFields(p: { title: string; axis: Axis; other: TestParam; onChange: (a: Axis) => void }) {
  const t = useT();
  const a = p.axis;
  const unit = unitOf(a.param);
  const digits = a.param === "interval" ? 4 : a.param === "passes" ? 0 : 2;
  return (
    <fieldset className="job-fieldset">
      <legend>{p.title}</legend>
      <label className="field">
        <span className="field-label">{t("ltest.param")}</span>
        <span className="field-input">
          <select
            value={a.param}
            onChange={(e) => {
              const param = e.target.value as TestParam;
              const [min, max] = PARAM_DEFAULT_RANGE[param];
              p.onChange({ ...a, param, min, max });
            }}
          >
            {PARAMS.map((x) => (
              <option key={x.value} value={x.value} disabled={x.value === p.other}>
                {x.label}
              </option>
            ))}
          </select>
        </span>
      </label>
      <NumberField label={t("ltest.count")} value={a.count} min={1} max={20} digits={0} onCommit={(v) => v !== null && p.onChange({ ...a, count: Math.round(v) })} />
      <NumberField label={t("ltest.min")} unit={unit} value={a.min} digits={digits} onCommit={(v) => v !== null && p.onChange({ ...a, min: v })} />
      <NumberField label={t("ltest.max")} unit={unit} value={a.max} digits={digits} onCommit={(v) => v !== null && p.onChange({ ...a, max: v })} />
    </fieldset>
  );
}

function BaseFields(p: { machine: MachineStatus | null; base: BaseSettings; onChange: (b: BaseSettings) => void; tested: TestParam[] }) {
  const t = useT();
  const b = p.base;
  const freq = frequencyRange(p.machine);
  const off = (x: TestParam) => p.tested.includes(x);
  const set = (x: Partial<BaseSettings>) => p.onChange({ ...b, ...x });
  return (
    <fieldset className="job-fieldset">
      <legend>{t("ltest.material")}</legend>
      <label className="field">
        <span className="field-label">{t("lib.mode")}</span>
        <span className="field-input">
          <select value={b.mode} onChange={(e) => set({ mode: e.target.value as BaseSettings["mode"] })}>
            <option value="fill">{t("lib.modeFill")}</option>
            <option value="line">{t("lib.modeLine")}</option>
            <option value="fill_then_line">{t("lib.modeFillLine")}</option>
            <option value="offset_fill">{t("lib.modeOffset")}</option>
          </select>
        </span>
      </label>
      {!off("power") && <NumberField label={t("lib.power")} unit="%" value={b.power_pct} min={0} max={100} digits={1} onCommit={(v) => v !== null && set({ power_pct: v })} />}
      {!off("speed") && <NumberField label={t("lib.speed")} unit="mm/s" value={b.speed_mm_s} min={0.01} digits={1} onCommit={(v) => v !== null && set({ speed_mm_s: v })} />}
      {!off("frequency") && (
        <NumberField label={t("lib.frequency")} unit="kHz" value={b.frequency_khz} min={freq.min} max={freq.max} digits={1} onCommit={(v) => v !== null && set({ frequency_khz: v })} />
      )}
      {!off("pulse_width") && <PulseField machine={p.machine} value={b.pulse_width_ns} onChange={(v) => set({ pulse_width_ns: v })} />}
      {!off("passes") && <NumberField label={t("lib.passes")} value={b.passes} min={1} max={1000} digits={0} onCommit={(v) => v !== null && set({ passes: Math.round(v) })} />}
      {!off("interval") && b.mode !== "line" && (
        <NumberField label={t("lib.interval")} unit="mm" value={b.interval_mm} min={0.001} digits={4} onCommit={(v) => v !== null && set({ interval_mm: v })} />
      )}
    </fieldset>
  );
}

export function MaterialTestDialog(p: GenProps) {
  const t = useT();
  const [m, setM] = useState<MaterialTest>(() => load("material", DEFAULT_MATERIAL_TEST));
  const upd = (x: Partial<MaterialTest>) =>
    setM((v) => {
      const n = { ...v, ...x };
      save("material", n);
      return n;
    });
  const preview = useMemo(() => materialTestPreview(m), [m]);
  const problem = materialTestProblem(m, pulseWidths(p.machine));
  const boxes = m.rows.count * m.columns.count;
  return (
    <GenDialog
      title={t("ltest.materialTitle")}
      props={p}
      problem={problem}
      preview={preview}
      onReset={() => upd(DEFAULT_MATERIAL_TEST)}
      onCreate={(req) => create(p, t, "generate.material_test", materialTestParams(m), req, t("ltest.materialWhat", { boxes }))}
    >
      <AxisFields title={t("ltest.rows")} axis={m.rows} other={m.columns.param} onChange={(rows) => upd({ rows })} />
      <AxisFields title={t("ltest.columns")} axis={m.columns} other={m.rows.param} onChange={(columns) => upd({ columns })} />
      <fieldset className="job-fieldset">
        <legend>{t("ltest.layout")}</legend>
        <PairField labels={[t("ltest.width"), t("ltest.height")]} unit="mm" value={[m.cell_width_mm, m.cell_height_mm]} onCommit={([cell_width_mm, cell_height_mm]) => upd({ cell_width_mm, cell_height_mm })} />
        <NumberField label={t("ltest.spacing")} unit="mm" value={m.spacing_mm} min={0} digits={2} onCommit={(v) => v !== null && upd({ spacing_mm: v })} />
        <CentreField value={m.center} selection={p.selectionCentre} onChange={(center) => upd({ center })} />
      </fieldset>
      <BaseFields machine={p.machine} base={m.base} tested={[m.rows.param, m.columns.param]} onChange={(base) => upd({ base })} />
      <fieldset className="job-fieldset">
        <legend>{t("ltest.text")}</legend>
        <CheckField label={t("ltest.enableText")} checked={m.labels} onChange={(labels) => upd({ labels })} />
        {m.labels && (
          <>
            <NumberField label={t("ltest.textHeight")} unit="mm" value={m.label_height_mm} min={0.1} digits={2} onCommit={(v) => v !== null && upd({ label_height_mm: v })} />
            <NumberField label={t("ltest.textPower")} unit="%" value={m.label_power_pct} min={0} max={100} digits={1} onCommit={(v) => v !== null && upd({ label_power_pct: v })} />
            <NumberField label={t("ltest.textSpeed")} unit="mm/s" value={m.label_speed_mm_s} min={0.01} digits={1} onCommit={(v) => v !== null && upd({ label_speed_mm_s: v })} />
            <label className="field">
              <span className="field-label">{t("ltest.title")}</span>
              <span className="field-input">
                <input type="text" value={m.title} maxLength={200} onChange={(e) => upd({ title: e.target.value })} />
              </span>
            </label>
          </>
        )}
        <CheckField label={t("ltest.enableBorder")} checked={m.border} onChange={(border) => upd({ border })} />
        {m.border && (
          <>
            <NumberField label={t("ltest.borderPower")} unit="%" value={m.border_power_pct} min={0} max={100} digits={1} onCommit={(v) => v !== null && upd({ border_power_pct: v })} />
            <NumberField label={t("ltest.borderSpeed")} unit="mm/s" value={m.border_speed_mm_s} min={0.01} digits={1} onCommit={(v) => v !== null && upd({ border_speed_mm_s: v })} />
          </>
        )}
      </fieldset>
      <p className="hint">{t("ltest.materialHint")}</p>
    </GenDialog>
  );
}

export function IntervalTestDialog(p: GenProps) {
  const t = useT();
  const [m, setM] = useState<IntervalTest>(() => load("interval", DEFAULT_INTERVAL_TEST));
  const upd = (x: Partial<IntervalTest>) =>
    setM((v) => {
      const n = { ...v, ...x };
      save("interval", n);
      return n;
    });
  const preview = useMemo(() => intervalTestPreview(m), [m]);
  const freq = frequencyRange(p.machine);
  return (
    <GenDialog
      title={t("ltest.intervalTitle")}
      props={p}
      problem={intervalTestProblem(m)}
      preview={preview}
      onReset={() => upd(DEFAULT_INTERVAL_TEST)}
      onCreate={(req) => create(p, t, "generate.interval_test", intervalTestParams(m), req, t("ltest.intervalWhat", { steps: m.steps }))}
    >
      <fieldset className="job-fieldset">
        <legend>{t("ltest.intervals")}</legend>
        <NumberField label={t("ltest.minInterval")} unit="mm" value={m.min_interval_mm} min={0.001} digits={4} onCommit={(v) => v !== null && upd({ min_interval_mm: v })} />
        <NumberField label={t("ltest.maxInterval")} unit="mm" value={m.max_interval_mm} min={0.001} digits={4} onCommit={(v) => v !== null && upd({ max_interval_mm: v })} />
        <NumberField label={t("ltest.steps")} value={m.steps} min={2} max={50} digits={0} onCommit={(v) => v !== null && upd({ steps: Math.round(v) })} />
        <label className="field">
          <span className="field-label">{t("ltest.fill")}</span>
          <span className="field-input">
            <select value={m.fill} onChange={(e) => upd({ fill: e.target.value === "dithered" ? "dithered" : "simple" })}>
              <option value="simple">{t("ltest.simpleFill")}</option>
              <option value="dithered">{t("ltest.dithered")}</option>
            </select>
          </span>
        </label>
        <NumberField label={t("ltest.size")} unit="mm" value={m.size_mm} min={0.5} digits={2} onCommit={(v) => v !== null && upd({ size_mm: v })} />
        <CentreField value={m.center} selection={p.selectionCentre} onChange={(center) => upd({ center })} />
      </fieldset>
      <fieldset className="job-fieldset">
        <legend>{t("ltest.material")}</legend>
        <NumberField label={t("lib.power")} unit="%" value={m.power_pct} min={0} max={100} digits={1} onCommit={(v) => v !== null && upd({ power_pct: v })} />
        <NumberField label={t("lib.speed")} unit="mm/s" value={m.speed_mm_s} min={0.01} digits={1} onCommit={(v) => v !== null && upd({ speed_mm_s: v })} />
        <NumberField label={t("lib.frequency")} unit="kHz" value={m.frequency_khz} min={freq.min} max={freq.max} digits={1} onCommit={(v) => v !== null && upd({ frequency_khz: v })} />
        <PulseField machine={p.machine} value={m.pulse_width_ns} onChange={(pulse_width_ns) => upd({ pulse_width_ns })} />
        <CheckField label={t("ltest.enableText")} checked={m.labels} onChange={(labels) => upd({ labels })} />
      </fieldset>
      <p className="hint">{t("ltest.intervalHint")}</p>
    </GenDialog>
  );
}

export function FocusTestDialog(p: GenProps) {
  const t = useT();
  const [m, setM] = useState<FocusTest>(() => load("focus", DEFAULT_FOCUS_TEST));
  const upd = (x: Partial<FocusTest>) =>
    setM((v) => {
      const n = { ...v, ...x };
      save("focus", n);
      return n;
    });
  const preview = useMemo(() => focusTestPreview(m), [m]);
  const freq = frequencyRange(p.machine);
  const dev = useActiveDevice();
  // Only a Z lift moves the focus; a linear or rotating table axis (role "linear" / "rotary") does not.
  const axis = dev?.profile && typeof dev.profile.axis === "object" && dev.profile.axis !== null ? (dev.profile.axis as { role?: unknown }) : null;
  const zAxis = !!axis && (axis.role === undefined || axis.role === null || axis.role === "z");
  return (
    <GenDialog
      title={t("ltest.focusTitle")}
      props={p}
      problem={focusTestProblem(m)}
      preview={preview}
      onReset={() => upd(DEFAULT_FOCUS_TEST)}
      onCreate={(req) => {
        // Without a Z lift the planner ignores the Z offsets: say so when the test is made, not only in the dialog.
        if (!zAxis) p.notify(t("ltest.focusNoZWarn"));
        return create(p, t, "generate.focus_test", focusTestParams(m), req, t("ltest.focusWhat", { lines: m.steps + 1 }));
      }}
    >
      <fieldset className="job-fieldset">
        <legend>{t("ltest.zRange")}</legend>
        <NumberField label={t("ltest.startZ")} unit="mm" value={m.start_z_mm} min={-100} max={100} digits={3} onCommit={(v) => v !== null && upd({ start_z_mm: v })} />
        <NumberField label={t("ltest.endZ")} unit="mm" value={m.end_z_mm} min={-100} max={100} digits={3} onCommit={(v) => v !== null && upd({ end_z_mm: v })} />
        <NumberField label={t("ltest.steps")} value={m.steps} min={1} max={50} digits={0} onCommit={(v) => v !== null && upd({ steps: Math.round(v) })} />
        <p className="hint">{t("ltest.zStep", { step: show((m.end_z_mm - m.start_z_mm) / Math.max(1, m.steps)) })}</p>
      </fieldset>
      <fieldset className="job-fieldset">
        <legend>{t("ltest.layout")}</legend>
        <NumberField label={t("ltest.lineLength")} unit="mm" value={m.line_length_mm} min={1} digits={2} onCommit={(v) => v !== null && upd({ line_length_mm: v })} />
        <NumberField label={t("ltest.lineSpacing")} unit="mm" value={m.spacing_mm} min={0.1} digits={2} onCommit={(v) => v !== null && upd({ spacing_mm: v })} />
        <CentreField value={m.center} selection={p.selectionCentre} onChange={(center) => upd({ center })} />
      </fieldset>
      <fieldset className="job-fieldset">
        <legend>{t("ltest.material")}</legend>
        <NumberField label={t("lib.power")} unit="%" value={m.power_pct} min={0} max={100} digits={1} onCommit={(v) => v !== null && upd({ power_pct: v })} />
        <NumberField label={t("lib.speed")} unit="mm/s" value={m.speed_mm_s} min={0.01} digits={1} onCommit={(v) => v !== null && upd({ speed_mm_s: v })} />
        <NumberField label={t("lib.frequency")} unit="kHz" value={m.frequency_khz} min={freq.min} max={freq.max} digits={1} onCommit={(v) => v !== null && upd({ frequency_khz: v })} />
        <CheckField label={t("ltest.enableText")} checked={m.labels} onChange={(labels) => upd({ labels })} />
      </fieldset>
      <p className={zAxis ? "hint" : "run-hint"}>{zAxis ? t("ltest.focusHint") : t("ltest.focusNoZ")}</p>
    </GenDialog>
  );
}
