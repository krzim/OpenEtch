// The jigs windows (shared Dialog primitive; full-screen sheets on the phone): Fit to stock options, New jig (a grid
// of pockets) and the Jigs window (pockets on/off, frame a pocket, place the selection in every pocket, move the jig,
// keep-out zones of the jig and of the device, delete). Every change is a document command (`jig.*`,
// `object.fit_to_stock`) or, for the device's zones, `PUT /api/prefs` (UI only); framing is light only.

import { useMemo, useState } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import { jobApi } from "../api/jobApi.ts";
import { Dialog } from "../components/Dialog.tsx";
import { CheckField, Icon, NumberField } from "../components/ui.tsx";
import { flattenCmds, geometryCmds } from "../geometry.ts";
import { libsApi } from "../libs/api.ts";
import { ConfirmDialog, errText } from "../libs/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import {
  J,
  gridProblem,
  jigsOf,
  objectsInPocket,
  placementsOf,
  pocketOn,
  sortedPockets,
  type DeviceZone,
  type FitRotate,
  type Jig,
  type Numbering,
  type PocketShape,
} from "./jigModel.ts";
import { loadDeviceZones, useDeviceZones } from "./Preflight.tsx";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
type Notify = (msg: string, kind?: "info" | "error") => void;

const FIT_KEY = "openetch.fitToStock";

/** The last Fit to stock options (this browser). */
export function fitDefaults(): { margin: number; rotate: FitRotate } {
  try {
    const v = JSON.parse(localStorage.getItem(FIT_KEY) ?? "{}") as { margin?: unknown; rotate?: unknown };
    const rotate = v.rotate === "ninety" || v.rotate === "best" ? v.rotate : "none";
    return { margin: typeof v.margin === "number" && v.margin >= 0 ? v.margin : 1, rotate };
  } catch {
    return { margin: 1, rotate: "none" };
  }
}

function saveFitDefaults(v: { margin: number; rotate: FitRotate }) {
  try {
    localStorage.setItem(FIT_KEY, JSON.stringify(v));
  } catch {
    // Private windows: the defaults stay for this page only.
  }
}

/** Run Fit to stock and say how it went. */
export async function fitToStock(run: Run, notify: Notify, t: ReturnType<typeof useT>, ids: readonly string[], o = fitDefaults()): Promise<void> {
  const r = (await run(J.fitToStock(ids, o.margin, o.rotate))) as { scale?: number; rotation_deg?: number } | null;
  const scale = (r?.scale ?? 1).toFixed(3);
  const deg = r?.rotation_deg ?? 0;
  notify(Math.abs(deg) > 1e-6 ? t("fit.doneTurned", { scale, deg: Math.round(deg * 10) / 10 }) : t("fit.done", { scale }));
}

export function FitDialog(p: { ids: string[]; run: Run; notify: Notify; onClose: () => void }) {
  const t = useT();
  const d = fitDefaults();
  const [margin, setMargin] = useState<number | null>(d.margin);
  const [rotate, setRotate] = useState<FitRotate>(d.rotate);
  const [error, setError] = useState<string | null>(null);
  const ok = () => {
    const o = { margin: margin ?? 0, rotate };
    saveFitDefaults(o);
    fitToStock(p.run, p.notify, t, p.ids, o)
      .then(p.onClose)
      .catch((e) => setError(errText(e)));
  };
  return (
    <Dialog
      title={t("fit.title")}
      size="sm"
      onClose={p.onClose}
      onSubmit={ok}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" onClick={ok}>
            {t("fit.menu")}
          </button>
        </>
      }
    >
      <NumberField label={t("fit.margin")} unit="mm" value={margin} min={0} max={1000} digits={2} onCommit={setMargin} stepper />
      <div className="field">
        <span className="field-label">{t("fit.rotate")}</span>
        <div className="segmented jig-seg" role="radiogroup" aria-label={t("fit.rotate")}>
          {(["none", "ninety", "best"] as const).map((r) => (
            <button key={r} type="button" role="radio" aria-checked={rotate === r} className={rotate === r ? "active" : ""} onClick={() => setRotate(r)}>
              {t(r === "none" ? "fit.rotateNone" : r === "ninety" ? "fit.rotateNinety" : "fit.rotateBest")}
            </button>
          ))}
        </div>
      </div>
      <p className="hint">{t("fit.hint")}</p>
    </Dialog>
  );
}

export function NewJigDialog(p: { doc: GDocument | null; run: Run; notify: Notify; onClose: () => void; onCreated: (id: string) => void }) {
  const t = useT();
  const stockC: Vec2 = p.doc?.stock?.center ?? [0, 0];
  const [name, setName] = useState("");
  const [rows, setRows] = useState<number | null>(2);
  const [cols, setCols] = useState<number | null>(3);
  const [kind, setKind] = useState<"circle" | "rect">("circle");
  const [dia, setDia] = useState<number | null>(25);
  const [w, setW] = useState<number | null>(30);
  const [h, setH] = useState<number | null>(20);
  const [cr, setCr] = useState<number | null>(0);
  const [px, setPx] = useState<number | null>(32);
  const [py, setPy] = useState<number | null>(32);
  const [cx, setCx] = useState<number | null>(stockC[0]);
  const [cy, setCy] = useState<number | null>(stockC[1]);
  const [mh, setMh] = useState<number | null>(null);
  const [numbering, setNumbering] = useState<Numbering>("rows");
  const [error, setError] = useState<string | null>(null);
  const size: Vec2 = kind === "circle" ? [dia ?? 0, dia ?? 0] : [w ?? 0, h ?? 0];
  const problem = gridProblem(rows ?? 0, cols ?? 0, px ?? 0, py ?? 0, size);
  const ok = () => {
    if (problem) return setError(t(problem));
    const pocket: PocketShape = kind === "circle" ? { kind: "circle", diameter_mm: dia ?? 0 } : { kind: "rect", width_mm: w ?? 0, height_mm: h ?? 0, corner_radius_mm: cr ?? 0 };
    p.run(J.create({ name: name.trim() || undefined, rows: rows ?? 1, cols: cols ?? 1, pitch_x_mm: px ?? 0, pitch_y_mm: py ?? 0, pocket, center: [cx ?? 0, cy ?? 0], material_height_mm: mh, numbering }))
      .then((r) => {
        const jig = (r as { jig?: { id?: string; name?: string; pockets?: unknown[] } }).jig;
        p.notify(t("jig.created", { name: jig?.name ?? "", n: jig?.pockets?.length ?? 0 }));
        if (jig?.id) p.onCreated(jig.id);
        p.onClose();
      })
      .catch((e) => setError(errText(e)));
  };
  return (
    <Dialog
      title={t("jig.newTitle")}
      size="md"
      onClose={p.onClose}
      onSubmit={ok}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!!problem} onClick={ok}>
            {t("jig.create")}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{t("jig.name")}</span>
        <span className="field-input">
          <input value={name} placeholder={`Jig ${jigsOf(p.doc).length + 1}`} onChange={(e) => setName(e.target.value)} />
        </span>
      </label>
      <div className="jig-two">
        <NumberField label={t("jig.rows")} value={rows} min={1} max={500} digits={0} onCommit={setRows} stepper />
        <NumberField label={t("jig.cols")} value={cols} min={1} max={500} digits={0} onCommit={setCols} stepper />
      </div>
      <div className="field">
        <span className="field-label">{t("jig.shape")}</span>
        <div className="segmented jig-seg" role="radiogroup" aria-label={t("jig.shape")}>
          <button type="button" role="radio" aria-checked={kind === "circle"} className={kind === "circle" ? "active" : ""} onClick={() => setKind("circle")}>
            {t("jig.circle")}
          </button>
          <button type="button" role="radio" aria-checked={kind === "rect"} className={kind === "rect" ? "active" : ""} onClick={() => setKind("rect")}>
            {t("jig.rect")}
          </button>
        </div>
      </div>
      {kind === "circle" ? (
        <NumberField label={t("jig.diameter")} unit="mm" value={dia} min={0.1} max={1000} digits={2} onCommit={setDia} />
      ) : (
        <div className="jig-two">
          <NumberField label={t("jig.width")} unit="mm" value={w} min={0.1} max={1000} digits={2} onCommit={setW} />
          <NumberField label={t("jig.height")} unit="mm" value={h} min={0.1} max={1000} digits={2} onCommit={setH} />
          <NumberField label={t("jig.radius")} unit="mm" value={cr} min={0} max={500} digits={2} onCommit={setCr} />
        </div>
      )}
      <div className="jig-two">
        <NumberField label={t("jig.pitchX")} unit="mm" value={px} min={0} max={10000} digits={2} onCommit={setPx} />
        <NumberField label={t("jig.pitchY")} unit="mm" value={py} min={0} max={10000} digits={2} onCommit={setPy} />
      </div>
      <div className="jig-two">
        <NumberField label={t("jig.centerX")} unit="mm" value={cx} digits={2} onCommit={setCx} signed />
        <NumberField label={t("jig.centerY")} unit="mm" value={cy} digits={2} onCommit={setCy} signed />
      </div>
      <NumberField label={t("jig.materialHeight")} unit="mm" value={mh} min={0} max={1000} digits={2} onCommit={setMh} allowEmpty />
      <NumberingField value={numbering} onChange={setNumbering} />
      {problem && <p className="form-error">{t(problem)}</p>}
    </Dialog>
  );
}

function NumberingField(p: { value: Numbering; onChange: (n: Numbering) => void }) {
  const t = useT();
  return (
    <label className="field">
      <span className="field-label">{t("jig.numbering")}</span>
      <span className="field-input">
        <select value={p.value} onChange={(e) => p.onChange(e.target.value as Numbering)}>
          <option value="rows">{t("jig.numRows")}</option>
          <option value="columns">{t("jig.numColumns")}</option>
          <option value="serpentine">{t("jig.numSerpentine")}</option>
        </select>
      </span>
    </label>
  );
}

/** Closed outlines of objects as field-frame polygons (for the device's keep-out zones). */
function closedRings(doc: GDocument, ids: readonly string[]): Vec2[][] {
  const want = new Set(ids);
  const out: Vec2[][] = [];
  for (const o of doc.objects) {
    if (!want.has(o.id) && !(o.group && want.has(o.group))) continue;
    for (const l of flattenCmds(geometryCmds(o.geometry), o.transform)) {
      const pts = l.pts;
      const closed = l.closed || (pts.length > 3 && Math.hypot(pts[0][0] - pts[pts.length - 1][0], pts[0][1] - pts[pts.length - 1][1]) < 1e-6);
      if (closed && pts.length >= 3) out.push(pts.map((p) => [Math.round(p[0] * 1e4) / 1e4, Math.round(p[1] * 1e4) / 1e4] as Vec2));
    }
  }
  return out;
}

export interface JigsDialogProps {
  doc: GDocument;
  selection: readonly string[];
  jig: string | null;
  onJig: (id: string | null) => void;
  designOnly: boolean;
  run: Run;
  notify: Notify;
  onClose: () => void;
  onNew: () => void;
  onImport: () => void;
  onPocketMode: (jig: string) => void;
  selectIds: (ids: string[]) => void;
}

export function JigsDialog(p: JigsDialogProps) {
  const t = useT();
  const jigs = jigsOf(p.doc);
  const jig: Jig | null = jigs.find((j) => j.id === p.jig) ?? jigs[0] ?? null;
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const zones = useDeviceZones();
  const exec = (inv: Invocation | Invocation[], done?: string) =>
    p
      .run(inv)
      .then((r) => {
        setError(null);
        if (done) p.notify(done);
        return r;
      })
      .catch((e) => {
        setError(errText(e));
        return null;
      });
  const saveZones = (z: DeviceZone[]) =>
    libsApi
      .prefsEdit({ keep_out: z })
      .then(() => {
        loadDeviceZones();
        p.notify(t("jig.deviceSaved"));
      })
      .catch((e) => setError(errText(e)));
  const frame = (body: Record<string, unknown>, msg: string) =>
    jobApi
      .frame({ continuous: true, ...body })
      .then(() => p.notify(msg))
      .catch((e) => setError(errText(e)));
  const placed = jig ? placementsOf(p.doc, jig.id) : [];
  const enabled = jig ? jig.pockets.filter(pocketOn).length : 0;
  // A placed design with serial or CSV text: Advance by should step over every pocket that is on.
  const counting = useMemo(() => {
    if (!jig) return false;
    const ids = new Set(placed.flatMap((x) => x.sources));
    return p.doc.objects.some((o) => ids.has(o.id) && o.geometry.kind === "text" && ["serial", "csv"].includes(String((o.geometry as { variable?: { mode?: string } }).variable?.mode)));
  }, [jig, placed, p.doc]);
  const advanceBy = (p.doc.variables as { advance_by?: number } | null | undefined)?.advance_by ?? 1;
  return (
    <>
      <Dialog
        title={t("jig.title")}
        size="lg"
        onClose={p.onClose}
        footer={
          <>
            {error && <span className="form-error">{error}</span>}
            {jig && (
              <button className="btn btn-danger jig-delete" onClick={() => setConfirmDelete(true)}>
                {t("jig.delete")}
              </button>
            )}
            <button className="btn btn-primary" onClick={p.onClose}>
              {t("common.close")}
            </button>
          </>
        }
      >
        <div className="jig-top">
          {jigs.length > 0 ? (
            <label className="field">
              <span className="field-label">{t("jig.title")}</span>
              <span className="field-input">
                <select value={jig?.id ?? ""} onChange={(e) => p.onJig(e.target.value)}>
                  {jigs.map((j) => (
                    <option key={j.id} value={j.id}>
                      {j.name}
                    </option>
                  ))}
                </select>
              </span>
            </label>
          ) : (
            <p className="hint">{t("jig.none")}</p>
          )}
          <div className="jig-actions">
            <button className="btn" onClick={p.onNew}>
              {t("jig.new")}
            </button>
            <button className="btn" onClick={p.onImport}>
              {t("jig.import")}
            </button>
            <button
              className="btn"
              disabled={p.selection.length === 0}
              title={p.selection.length === 0 ? t("jig.needClosed") : undefined}
              onClick={() =>
                void exec(J.fromObjects(p.selection), "").then((r) => {
                  const j = (r as { jig?: { id?: string; name?: string; pockets?: unknown[] } } | null)?.jig;
                  if (j?.id) {
                    p.onJig(j.id);
                    p.notify(t("jig.created", { name: j.name ?? "", n: j.pockets?.length ?? 0 }));
                  }
                })
              }
            >
              {t("jig.fromSelection")}
            </button>
          </div>
        </div>
        {jig && (
          <>
            <section className="jig-section">
              <h4>
                {t("jig.on", { on: enabled, n: jig.pockets.length })}
                <button className="btn small" onClick={() => void exec(J.setPockets(jig.id, "all", true))} disabled={enabled === jig.pockets.length}>
                  {t("jig.allOn")}
                </button>
                <button className="btn small" disabled={p.designOnly} onClick={() => void frame({ jig_pockets: { jig: jig.id }, mode: "outline" }, t("jig.frameOn"))}>
                  <Icon name="frame" size={14} /> {t("jig.frameOn")}
                </button>
                <button className="btn small" onClick={() => (p.onPocketMode(jig.id), p.onClose())}>
                  {t("jig.pockets")}
                </button>
              </h4>
              <div className="jig-pockets" role="group" aria-label={t("jig.pockets")}>
                {sortedPockets(jig).map((pk) => {
                  const on = pocketOn(pk);
                  const inside = objectsInPocket(p.doc, jig.id, pk.id);
                  return (
                    <span key={pk.id} className={`jig-pocket${on ? "" : " off"}`}>
                      <button
                        type="button"
                        className="jig-pocket-toggle"
                        aria-pressed={on}
                        title={on ? t("jig.pocket", { n: pk.number }) : t("jig.pocketOff", { n: pk.number })}
                        onClick={() => void exec(J.setPockets(jig.id, [pk.number], !on))}
                      >
                        {pk.number}
                      </button>
                      <button
                        type="button"
                        className="icon-btn small jig-pocket-frame"
                        disabled={p.designOnly}
                        title={`${t("jig.frame")} ${pk.number}`}
                        aria-label={`${t("jig.frame")} ${pk.number}`}
                        onClick={() =>
                          void frame(inside.length ? { ids: inside, mode: "outline" } : { jig_pockets: { jig: jig.id, pockets: [pk.number] }, mode: "outline" }, t("jig.framePocket", { n: pk.number }))
                        }
                      >
                        <Icon name="frame" size={14} />
                      </button>
                    </span>
                  );
                })}
              </div>
            </section>
            <section className="jig-section">
              <h4>{t("jig.placements", { n: placed.length })}</h4>
              <button
                className="btn"
                disabled={p.selection.length === 0}
                title={p.selection.length === 0 ? t("jig.selectFirst") : undefined}
                onClick={() =>
                  void exec(J.place(jig.id, p.selection)).then((r) => {
                    const created = (r as { created?: string[] } | null)?.created;
                    if (created) p.notify(t("jig.placed", { n: enabled }));
                  })
                }
              >
                {t("jig.place")}
              </button>
              {counting && advanceBy !== enabled && (
                <p className="hint jig-serial">
                  {t("jig.serials", { n: enabled })}{" "}
                  <button className="btn small" onClick={() => void exec({ id: "var.set_serial", params: { advance_by: enabled } }, t("jig.advanceSet", { n: enabled }))}>
                    {t("jig.setAdvance", { n: enabled })}
                  </button>
                </p>
              )}
            </section>
            <JigEditor key={jig.id} jig={jig} exec={exec} />
            <section className="jig-section">
              <h4>{t("jig.keepOuts")}</h4>
              <p className="hint">{t("jig.keepOutHint")}</p>
              <ul className="jig-zones">
                {(jig.keep_out ?? []).length === 0 && <li className="hint">{t("jig.keepOutNone")}</li>}
                {(jig.keep_out ?? []).map((k) => (
                  <li key={k.id}>
                    <CheckField label={k.name} checked={k.enabled !== false} onChange={(v) => void exec(J.keepOut(jig.id, k.id, v))} />
                    <button className="btn small" onClick={() => void exec(J.keepOut(jig.id, k.id))}>
                      {t("jig.remove")}
                    </button>
                  </li>
                ))}
              </ul>
              <button className="btn" disabled={p.selection.length === 0} onClick={() => void exec(J.addKeepOut(jig.id, p.selection), t("jig.keepOutAdded"))}>
                {t("jig.keepOutAdd")}
              </button>
            </section>
          </>
        )}
        <section className="jig-section">
          <h4>{t("jig.deviceZones")}</h4>
          <p className="hint">{t("jig.deviceZonesHint")}</p>
          <ul className="jig-zones">
            {zones.length === 0 && <li className="hint">{t("jig.keepOutNone")}</li>}
            {zones.map((z, i) => (
              <li key={`${z.name}-${i}`}>
                <CheckField label={z.name} checked={z.enabled !== false} onChange={(v) => void saveZones(zones.map((x, k) => (k === i ? { ...x, enabled: v } : x)))} />
                <button className="btn small" onClick={() => void saveZones(zones.filter((_, k) => k !== i))}>
                  {t("jig.remove")}
                </button>
              </li>
            ))}
          </ul>
          <button
            className="btn"
            disabled={p.selection.length === 0}
            onClick={() => {
              const rings = closedRings(p.doc, p.selection);
              if (!rings.length) return setError(t("jig.needClosed"));
              const base = zones.length;
              void saveZones([...zones, ...rings.map((points, k) => ({ name: `Zone ${base + k + 1}`, points }))]);
            }}
          >
            {t("jig.deviceZoneAdd")}
          </button>
        </section>
      </Dialog>
      {confirmDelete && jig && (
        <ConfirmDialog
          title={t("jig.delete")}
          ok={t("jig.delete")}
          danger
          onClose={() => setConfirmDelete(false)}
          onOk={() => {
            setConfirmDelete(false);
            void exec(J.remove(jig.id), t("jig.deleted")).then(() => p.onJig(null));
          }}
        >
          <p>{t("jig.deleteQ", { name: jig.name })}</p>
        </ConfirmDialog>
      )}
    </>
  );
}

/** Name, material height, numbering, and where the jig sits (datum and rotation). */
function JigEditor(p: { jig: Jig; exec: (inv: Invocation, done?: string) => Promise<unknown> }) {
  const t = useT();
  const j = p.jig;
  const [dx, setDx] = useState<number | null>(j.datum[0]);
  const [dy, setDy] = useState<number | null>(j.datum[1]);
  const [rot, setRot] = useState<number | null>(j.rotation_deg ?? 0);
  const moved = dx !== j.datum[0] || dy !== j.datum[1] || (rot ?? 0) !== (j.rotation_deg ?? 0);
  return (
    <section className="jig-section">
      <label className="field">
        <span className="field-label">{t("jig.name")}</span>
        <span className="field-input">
          <input
            defaultValue={j.name}
            onBlur={(e) => {
              const v = e.target.value.trim();
              if (v && v !== j.name) void p.exec(J.edit(j.id, { name: v }));
            }}
          />
        </span>
      </label>
      <NumberField
        label={t("jig.materialHeight")}
        unit="mm"
        value={j.material_height_mm ?? null}
        min={0}
        max={1000}
        digits={2}
        allowEmpty
        onCommit={(v) => void p.exec(J.edit(j.id, { material_height_mm: v }))}
      />
      <label className="field">
        <span className="field-label">{t("jig.numbering")}</span>
        <span className="field-input">
          <select defaultValue="" onChange={(e) => e.target.value && void p.exec(J.edit(j.id, { numbering: e.target.value as Numbering }))}>
            <option value="" disabled>
              …
            </option>
            <option value="rows">{t("jig.numRows")}</option>
            <option value="columns">{t("jig.numColumns")}</option>
            <option value="serpentine">{t("jig.numSerpentine")}</option>
          </select>
        </span>
      </label>
      <div className="jig-two">
        <NumberField label={t("jig.datumX")} unit="mm" value={dx} digits={3} onCommit={setDx} signed />
        <NumberField label={t("jig.datumY")} unit="mm" value={dy} digits={3} onCommit={setDy} signed />
        <NumberField label={t("jig.rotation")} unit="°" value={rot} digits={2} onCommit={setRot} signed />
      </div>
      <button className="btn" disabled={!moved} onClick={() => void p.exec(J.move(j.id, { datum: [dx ?? 0, dy ?? 0], rotation_deg: rot ?? 0 }), t("jig.moved"))}>
        {t("jig.moveApply")}
      </button>
    </section>
  );
}
