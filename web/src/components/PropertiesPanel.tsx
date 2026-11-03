// Right "Properties" panel: the selected object(s), the stock, and the selected layer's passes (main settings and
// sub-layers). Every field commits one command (`object.transform`, `object.set_layer`, `stock.move`, `layer.update`,
// `layer.sub_update`) on Enter or when focus leaves it; X and Y commit together as one step. Numbers accept simple
// maths (`20/3`).
// With objects selected the layer shows as one row ("All settings…" opens its full settings here); the Cuts / Layers
// quick row edits speed and power, so the cut settings are no longer repeated below the shape (V12). On a coarse
// pointer the position section has a nudge pad (touch/NudgePad.tsx), and the signed fields (X, Y, rotation) bring up
// the full keyboard, so "-12.5" and "20/3" can be typed on a phone.

import { useState } from "react";
import { NudgePad } from "../touch/NudgePad.tsx";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { gantryOf } from "../api/grbl.ts";
import { boxOutside, frequencyRange, nearestPulseWidth, pulseWidths, sourceFields } from "../api/laser.ts";
import { useCircleLock } from "../circleLock.ts";
import { MAX_SUB_LAYERS, clampPass, cleanupPass, passAfterDelete, passAfterMove, passColor, passEntries, passFromResult, settingsSummary } from "../api/passes.ts";
import type { Cleanup, CutSettings, FillMode, GDocument, Layer, MachineStatus, Vec2 } from "../api/types.ts";
import { roundMm } from "../editing.ts";
import { normDeg, objectBounds, transformParts, unionBoxes } from "../geometry.ts";
import { topGroup, unitsOf, type Selection } from "../selection.ts";
import { circleOf, shapeKindName } from "../snapping.ts";
import { ANCHORS, anchorPoint, useAnchor } from "../workspace/numeric.ts";
import { passLabelText, shapeKindText, usePrefs, useT, useTD } from "../workspace/usePrefs.ts";
import { formatLength, lengthDigits, lengthToDisplay, trimNumber } from "../workspace/units.ts";
import { CutOptionsSection, ShapeCutProperties } from "./CutOptions.tsx";
import { layerHasImages } from "../api/cutOptions.ts";
import { ImageSettings } from "./ImageSettings.tsx";
import { LayerPresetBar } from "../libs/LayerDefaults.tsx";
import { layerLink } from "../libs/model.ts";
import type { ArrangeAction } from "./Toolbar.tsx";
import { CheckField, ColorField, Icon, NumberField, PairField, TextField } from "./ui.tsx";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export interface PropertiesPanelProps {
  doc: GDocument | null;
  selection: Selection;
  machine: MachineStatus | null;
  /** The active device's workspace (W x H mm, centred on the field origin; a galvo field is square). */
  workspaceMm: Vec2;
  has: (cmd: string) => boolean;
  run: Run;
  report: (e: unknown) => void;
  onEditStock: () => void;
  onArrange: (a: ArrangeAction) => void;
}

export function PropertiesPanel(props: PropertiesPanelProps) {
  const t = useT();
  const { doc, selection } = props;
  if (!doc) return <div className="panel-empty">{t("pp.noDocument")}</div>;
  const layer = selection.layer ? doc.layers.find((l) => l.id === selection.layer) : undefined;
  const nothing = selection.objects.length === 0 && !layer && !selection.stock;
  return (
    <div className="props">
      {selection.objects.length > 0 && <ObjectSection {...props} doc={doc} />}
      {selection.objects.length > 0 && <ShapeCutProperties doc={doc} ids={selection.objects} run={props.run} report={props.report} has={props.has} />}
      {selection.stock && doc.stock && <StockSection {...props} doc={doc} />}
      {layer && selection.objects.length > 0 && <LayerRow key={layer.id} layer={layer} machine={props.machine} run={props.run} report={props.report} doc={doc} has={props.has} />}
      {layer && selection.objects.length === 0 && <LayerSection layer={layer} machine={props.machine} run={props.run} report={props.report} doc={doc} has={props.has} />}
      {nothing && (
        <div className="panel-empty">
          {t("pp.nothing")}
        </div>
      )}
    </div>
  );
}

/**
 * The selected objects' layer as one row: its colour, name and main speed / power, with "All settings…" to open the
 * full layer section in place (the Cuts / Layers quick row covers the everyday fields).
 */
function LayerRow(p: { layer: Layer; machine: MachineStatus | null; run: Run; report: (e: unknown) => void; doc: GDocument; has: (cmd: string) => boolean }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const s = p.layer.settings;
  return (
    <>
      <section className="prop-section prop-layer-row">
        <div className="prop-layer-line">
          <span className="swatch" style={{ background: p.layer.color }} />
          <span className="prop-layer-name">{p.layer.name}</span>
          <span className="muted">{p.layer.tool ? p.layer.tool : `${trimNumber(s.speed_mm_s, 1)} mm/s · ${trimNumber(s.power_pct, 1)}%`}</span>
          <button className="btn small" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
            {t("layers.more")}
          </button>
        </div>
      </section>
      {open && <LayerSection layer={p.layer} machine={p.machine} run={p.run} report={p.report} doc={p.doc} has={p.has} />}
    </>
  );
}

function ObjectSection({ doc, selection, run, report, onArrange, workspaceMm }: PropertiesPanelProps & { doc: GDocument }) {
  const t = useT();
  const td = useTD();
  const lm = useLayoutMode();
  const coarse = lm.pointer === "coarse" || lm.density === "touch";
  const [lockAspect, setLockAspect] = useState(true);
  // LightBurn's 9-point anchor, the same one the numeric toolbar uses.
  const [anchor, setAnchor] = useAnchor();
  const { units: lu, nudge } = usePrefs();
  const [keepRound, setKeepRound] = useCircleLock();
  const objs = doc.objects.filter((o) => selection.objects.includes(o.id));
  const bounds = unionBoxes(objs.map(objectBounds));
  if (objs.length === 0 || !bounds) return null;
  const single = objs.length === 1 ? objs[0] : undefined;
  // A lone round ellipse is a circle: Diameter plus "Keep round" (Fusion's circle has only a diameter).
  const circle = single ? circleOf(single) : null;
  const ids = objs.map((o) => o.id);
  const units = unitsOf(doc, ids);
  const groupId = units.length === 1 && single === undefined ? topGroup(doc, ids[0]) : null;
  const group = groupId ? doc.groups?.find((g) => g.id === groupId) : undefined;
  const [ax, ay] = anchorPoint(bounds, anchor);
  const [w, h] = [bounds.x1 - bounds.x0, bounds.y1 - bounds.y0];
  // A mirrored object shows its flip apart from the rotation: a horizontal flip is "Flip: horizontal, rotation 0°".
  const parts = single ? transformParts(single.transform) : null;
  const rot = parts ? parts.rotationDeg : 0;
  const exec = (inv: Invocation | Invocation[]) => run(inv).catch(report);
  const layers = new Set(objs.map((o) => o.layer));
  const outside = boxOutside(bounds, workspaceMm[0] / 2, workspaceMm[1] / 2);

  const setPos = (nx: number, ny: number) => {
    const d: Vec2 = [roundMm(nx - ax), roundMm(ny - ay)];
    if (d[0] !== 0 || d[1] !== 0) exec(C.transform(ids, { translate: d }));
  };

  const setSize = (nw: number | null, nh: number | null) => {
    let sx = nw !== null && w > 1e-9 ? nw / w : 1;
    let sy = nh !== null && h > 1e-9 ? nh / h : 1;
    if (lockAspect || (circle && keepRound)) {
      if (nw !== null) sy = sx;
      else sx = sy;
    }
    if (!(sx > 0 && sy > 0)) return;
    // Scale about the anchor, so the reference point stays where it is.
    exec(C.transform(ids, { scale: [round6(sx), round6(sy)], about: [roundMm(ax), roundMm(ay)] }));
  };

  /** New circle diameter, about the circle's centre. */
  const setDiameter = (d: number | null) => {
    if (!circle || d === null || !(d > 0) || circle.diameter <= 1e-9) return;
    const f = round6(d / circle.diameter);
    if (f !== 1) exec(C.transform(ids, { scale: [f, f], about: [roundMm(circle.center[0]), roundMm(circle.center[1])] }));
  };

  return (
    <section className="prop-section">
      <h3>
        {single
          ? shapeKindText(td, shapeKindName(single))
          : group
            ? t("pp.groupNamed", { name: group.name })
            : `${units.length > 1 ? t("pp.nItems", { n: units.length }) : t("pp.group")} · ${t("pp.nObjects", { n: objs.length })}`}
      </h3>
      {outside && <p className="warn-text">{t("pp.outside")}</p>}
      {single && <TextField label={t("pp.name")} value={single.name} onCommit={(name) => exec(C.rename(single.id, name))} />}
      <label className="field">
        <span className="field-label">{t("pp.layer")}</span>
        <span className="field-input">
          <select
            value={layers.size === 1 ? objs[0].layer : ""}
            onChange={(e) => e.target.value && exec(C.setLayer(ids, e.target.value))}
          >
            {layers.size > 1 && <option value="">{t("pp.severalLayers")}</option>}
            {doc.layers.map((l) => (
              <option key={l.id} value={l.id}>
                {l.name}
              </option>
            ))}
          </select>
        </span>
      </label>
      <div className="pos-row">
        <span className="pos-label">{t("pp.reference")}</span>
        <div className="anchor-grid small" role="radiogroup" aria-label={t("pp.referencePoint")}>
          {ANCHORS.map((a) => (
            <button key={a} role="radio" aria-checked={a === anchor} className={a === anchor ? "active" : ""} title={t("pp.anchorTitle", { a })} onClick={() => setAnchor(a)}>
              <span className="anchor-dot" />
            </button>
          ))}
        </div>
      </div>
      <PairField labels={["X", "Y"]} unit="mm" signed value={[ax, ay]} onCommit={(v) => setPos(v[0], v[1])} />
      {coarse && <NudgePad steps={nudge} unit={lu} />}
      <div className="field-grid">
        <NumberField label="W" unit="mm" value={w} min={0.01} stepper onCommit={(v) => setSize(v, null)} />
        <NumberField label="H" unit="mm" value={h} min={0.01} stepper onCommit={(v) => setSize(null, v)} />
      </div>
      <CheckField label={t("pp.lockAspect")} checked={lockAspect} onChange={setLockAspect} />
      {circle && (
        <>
          <NumberField label={t("pp.diameter")} unit="mm" value={circle.diameter} min={0.01} onCommit={setDiameter} />
          <CheckField label={t("pp.keepRound")} checked={keepRound} onChange={setKeepRound} />
        </>
      )}
      <div>
        <NumberField
          label={single ? t("pp.rotation") : t("pp.rotateBy")}
          signed
          unit="°"
          value={rot}
          digits={2}
          onCommit={(v) => {
            if (v === null) return;
            const delta = single ? normDeg(v - rot) : v;
            if (Math.abs(delta) > 1e-9) exec(C.transform(ids, { rotate_deg: round6(delta), about: [roundMm(ax), roundMm(ay)] }));
          }}
        />
      </div>
      {parts && (
        <dl className="kv">
          <dt>{t("pp.flip")}</dt>
          <dd>{parts.flip === "horizontal" ? t("pp.horizontal") : parts.flip === "vertical" ? t("pp.vertical") : t("pp.none")}</dd>
        </dl>
      )}
      <div className="btn-row compact">
        <button className="icon-btn small" title={t("tb.flipH")} aria-label={t("app.flipH")} onClick={() => onArrange("flipH")}>
          <Icon name="flipH" size={18} />
        </button>
        <button className="icon-btn small" title={t("tb.flipV")} aria-label={t("app.flipV")} onClick={() => onArrange("flipV")}>
          <Icon name="flipV" size={18} />
        </button>
        <button className="icon-btn small" title={t("tb.centreStock")} aria-label={t("tb.centreStock")} disabled={!doc.stock} onClick={() => onArrange("stock")}>
          <Icon name="centerStock" size={18} />
        </button>
        <button className="icon-btn small" title={t("tb.centreField")} aria-label={t("app.centreField")} onClick={() => onArrange("field")}>
          <Icon name="centerField" size={18} />
        </button>
        {units.length >= 2 && (
          <>
            <button className="icon-btn small" title={t("pp.alignLeftKey")} aria-label={t("app.alignLeft")} onClick={() => onArrange("left")}>
              <Icon name="alignLeft" size={18} />
            </button>
            <button className="icon-btn small" title={t("tb.alignHCenter")} aria-label={t("app.alignHCenter")} onClick={() => onArrange("hcenter")}>
              <Icon name="alignHCenter" size={18} />
            </button>
            <button className="icon-btn small" title={t("tb.alignVCenter")} aria-label={t("app.alignVCenter")} onClick={() => onArrange("vcenter")}>
              <Icon name="alignVCenter" size={18} />
            </button>
          </>
        )}
      </div>
      <p className="hint" hidden={coarse}>
        {t("pp.nudgeHint", {
          normal: formatLength(nudge.normal, lu, 3),
          ctrl: formatLength(nudge.ctrl, lu, 3),
          shift: formatLength(nudge.shift, lu, 3),
          ctrlShift: formatLength(nudge.ctrlShift, lu, 3),
        })}{" "}
        <kbd>.</kbd> <kbd>,</kbd> {t("pp.rotateKeys")}
      </p>
    </section>
  );
}

function StockSection({ doc, has, run, report, onEditStock }: PropertiesPanelProps & { doc: GDocument }) {
  const t = useT();
  const s = doc.stock;
  const lu = usePrefs().units;
  if (!s) return null;
  const exec = (inv: Invocation) => run(inv).catch(report);
  const move = (p: { center?: Vec2; rotation_deg?: number }) => {
    if (has(C.OPT.stockMove)) exec(C.stockMove(p));
    else exec(C.stockDefine({ ...s, center: p.center ?? s.center, rotation_deg: p.rotation_deg ?? s.rotation_deg }));
  };
  const dims =
    s.shape.kind === "rect"
      ? `${trimNumber(lengthToDisplay(s.shape.width_mm, lu), lengthDigits(lu, 3))} × ${formatLength(s.shape.height_mm, lu, 3)}`
      : `Ø ${formatLength(s.shape.diameter_mm, lu, 3)}`;
  return (
    <section className="prop-section">
      <h3>{t("pp.stock")}</h3>
      <dl className="kv">
        <dt>{t("pp.name")}</dt>
        <dd>{s.name}</dd>
        <dt>{t("pp.shape")}</dt>
        <dd>{dims}</dd>
        <dt>{t("pp.thickness")}</dt>
        <dd>{formatLength(s.thickness_mm, lu, 3)}</dd>
        <dt>{t("pp.material")}</dt>
        <dd>{s.material ?? "—"}</dd>
      </dl>
      <PairField labels={[t("pp.centreX"), t("pp.centreY")]} unit="mm" signed value={s.center} onCommit={(c) => move({ center: [roundMm(c[0]), roundMm(c[1])] })} />
      <NumberField label={t("pp.rotation")} signed unit="°" value={s.rotation_deg} digits={2} onCommit={(v) => v !== null && move({ rotation_deg: round6(normDeg(v)) })} />
      <p className="hint">{t("pp.stockHint")}</p>
      <div className="btn-row">
        <button className="btn" onClick={onEditStock}>
          {t("pp.editStock")}
        </button>
        {has(C.OPT.stockClear) && (
          <button className="btn" onClick={() => exec(C.stockClear())}>
            {t("pp.removeStock")}
          </button>
        )}
      </div>
    </section>
  );
}

const MODES: { value: FillMode; label: string }[] = [
  { value: "line", label: "Line" },
  { value: "fill", label: "Fill" },
  { value: "fill_then_line", label: "Fill + line" },
  { value: "offset_fill", label: "Offset Fill" },
];

/**
 * One layer: name, colour, show/output, and its passes like LightBurn's sub-layer tabs: "Main" (the layer's own
 * settings, `layer.update`) plus each sub-layer (`layer.sub_update`), run in that order. The open pass is edited with
 * the full cut-settings form; the buttons add, duplicate, delete and reorder sub-layers.
 */
export function LayerSection({
  layer,
  machine,
  run,
  report,
  doc,
  has,
  hasImages,
}: {
  layer: Layer;
  machine: MachineStatus | null;
  run: Run;
  report: (e: unknown) => void;
  doc?: GDocument;
  has?: (cmd: string) => boolean;
  /** The layer holds an image (for the overscan field); read from `doc` when not given. */
  hasImages?: boolean;
}) {
  const t = useT();
  const setL = (patch: { name?: string; color?: string; visible?: boolean; output?: boolean }) => run(C.layerUpdate(layer.id, patch)).catch(report);
  const others = doc ? doc.layers.filter((l) => l.id !== layer.id) : [];
  const count = doc ? doc.objects.filter((o) => o.layer === layer.id).length : 0;
  const [confirmDelete, setConfirmDelete] = useState(false);
  return (
    <section className="prop-section">
      <h3>
        <span className="swatch" style={{ background: layer.color }} /> {t("pp.layer")}
      </h3>
      <TextField label={t("pp.name")} value={layer.name} onCommit={(name) => setL({ name })} />
      <ColorField label={t("pp.colour")} value={toHex(layer.color)} onCommit={(color) => setL({ color })} />
      <CheckField label={t("pp.show")} checked={layer.visible} onChange={(visible) => setL({ visible })} />
      <CheckField label={t("pp.output")} checked={layer.output} onChange={(output) => setL({ output })} />

      <LayerPresetBar layer={layer} run={run} report={report} />
      <PassEditor layer={layer} machine={machine} run={run} report={report} has={has} hasImages={hasImages ?? layerHasImages(doc, layer.id)} />
      <ImageSettings layer={layer} doc={doc} run={run} report={report} has={has} />

      {doc && has?.(C.OPT.layerDelete) && others.length > 0 && (
        <div className="btn-row">
          {!confirmDelete ? (
            <button className="btn" onClick={() => setConfirmDelete(true)}>
              {t("pp.deleteLayer")}
            </button>
          ) : (
            <>
              <span className="hint">{count ? t(count === 1 ? "pp.moves1" : "pp.movesN", { n: count, name: others[0].name }) : t("pp.layerEmpty")}</span>
              <button className="btn" onClick={() => setConfirmDelete(false)}>
                {t("common.cancel")}
              </button>
              <button
                className="btn btn-warn"
                onClick={() => {
                  setConfirmDelete(false);
                  run(C.layerDelete(layer.id, count ? others[0].id : undefined)).catch(report);
                }}
              >
                {t("pp.delete")}
              </button>
            </>
          )}
        </div>
      )}
    </section>
  );
}

/** The pass list (Main + sub-layers), the sub-layer buttons, and the open pass's settings form. */
function PassEditor({
  layer,
  machine,
  run,
  report,
  has,
  hasImages,
}: {
  layer: Layer;
  machine: MachineStatus | null;
  run: Run;
  report: (e: unknown) => void;
  has?: (cmd: string) => boolean;
  hasImages: boolean;
}) {
  const t = useT();
  // The open pass, per layer: selecting another layer opens its Main tab.
  const [open, setOpen] = useState<{ layer: string; pass: number }>({ layer: layer.id, pass: 0 });
  const { speedUnit, beginnerMode } = usePrefs();
  const pass = clampPass(layer, open.layer === layer.id ? open.pass : 0);
  const select = (p: number) => setOpen({ layer: layer.id, pass: p });
  const entries = passEntries(layer);
  const entry = entries[pass];
  const sub = entry.sub;
  const subs = layer.sub_layers?.length ?? 0;
  const can = (id: string) => has?.(id) ?? false;
  const editSubs = can(C.OPT.layerSubUpdate);
  const freq = frequencyRange(machine);
  // Resolves to `{ result }`, or null when the command failed (already reported).
  const exec = (inv: Invocation): Promise<{ result: unknown } | null> =>
    run(inv).then(
      (result) => ({ result }),
      (e) => {
        report(e);
        return null;
      },
    );

  const patch = (p: C.CutSettingsPatch) => (sub === null ? exec(C.layerUpdate(layer.id, { settings: p })) : exec(C.layerSubUpdate(layer.id, sub, { settings: p })));
  const setSub = (p: { name?: string | null; cleanup?: Cleanup | null }) => sub !== null && void exec(C.layerSubUpdate(layer.id, sub, p));
  const add = async (p: Parameters<typeof C.layerSubAdd>[1], fallback: number) => {
    const r = await exec(C.layerSubAdd(layer.id, p));
    if (r) select(passFromResult(r.result, fallback));
  };
  const duplicate = async () => {
    // Duplicating Main adds a copy of it as the first sub-layer (a new sub-layer starts from the main settings).
    if (sub === null) return add({ index: 0 }, 1);
    const r = await exec(C.layerSubDuplicate(layer.id, sub));
    if (r) select(passFromResult(r.result, pass + 1));
  };
  const remove = async () => {
    if (sub === null) return;
    const r = await exec(C.layerSubDelete(layer.id, sub));
    if (r) select(passAfterDelete(sub, subs));
  };
  const move = async (delta: -1 | 1) => {
    if (sub === null) return;
    const to = sub + delta;
    if (to < 0 || to >= subs) return;
    const r = await exec(C.layerSubMove(layer.id, sub, to));
    if (r) select(passAfterMove(pass, sub, to));
  };
  // A layer linked to a library preset follows it and cannot be edited until unlinked (LightBurn's Link).
  const linked = layerLink(layer) !== null;
  const canEdit = (sub === null || editSubs) && !linked;
  const canAdd = can(C.OPT.layerSubAdd) && !linked;
  const full = subs >= MAX_SUB_LAYERS;

  return (
    <>
      <h4>{t("pp.cutSettings")}</h4>
      <div className="pass-tabs" role="tablist" aria-label={t("pp.passesTabs")}>
        {entries.map((e) => (
          <button
            key={e.pass}
            role="tab"
            aria-selected={e.pass === pass}
            className={`pass-tab${e.pass === pass ? " active" : ""}`}
            title={`${passLabelText(t, e.label)}: ${settingsSummary(e.settings, speedUnit)}`}
            onClick={() => select(e.pass)}
          >
            <span className="pass-tab-dot" style={{ background: passColor(layer.color, e.pass) }} />
            <span className="pass-tab-name">{passLabelText(t, e.label)}</span>
            {e.cleanup && <span className="badge pass-badge">{t("layers.cleanupPass")}</span>}
            <span className="pass-tab-meta">{settingsSummary(e.settings, speedUnit)}</span>
          </button>
        ))}
      </div>
      {!linked && (canAdd || (sub !== null && (can(C.OPT.layerSubDuplicate) || can(C.OPT.layerSubMove) || can(C.OPT.layerSubDelete)))) && (
        <div className="pass-actions">
          {canAdd && (
            <>
              <button
                className="btn small"
                title={t("pp.addSubTitle")}
                disabled={full}
                onClick={() => void add({}, subs + 1)}
              >
                <Icon name="plus" size={14} /> {t("pp.subLayer")}
              </button>
              <button
                className="btn small"
                title={t("pp.addCleanupTitle")}
                disabled={full}
                onClick={() => void add(cleanupPass(layer.settings, freq.max), subs + 1)}
              >
                <Icon name="plus" size={14} /> {t("pp.cleanupPass")}
              </button>
            </>
          )}
          {(sub === null ? canAdd : can(C.OPT.layerSubDuplicate)) && (
            <button
              className="btn small"
              title={sub === null ? t("pp.dupMain") : t("pp.dupSub")}
              disabled={full}
              onClick={() => void duplicate()}
            >
              {t("pp.duplicate")}
            </button>
          )}
          {sub !== null && can(C.OPT.layerSubMove) && (
            <>
              <button className="icon-btn small" title={t("pp.runEarlier")} aria-label={t("pp.subUp")} disabled={sub === 0} onClick={() => void move(-1)}>
                <Icon name="chevronUp" size={16} />
              </button>
              <button className="icon-btn small" title={t("pp.runLater")} aria-label={t("pp.subDown")} disabled={sub >= subs - 1} onClick={() => void move(1)}>
                <Icon name="chevronDown" size={16} />
              </button>
            </>
          )}
          {sub !== null && can(C.OPT.layerSubDelete) && (
            <button className="btn small btn-warn" title={t("pp.deleteSub")} onClick={() => void remove()}>
              {t("pp.delete")}
            </button>
          )}
        </div>
      )}
      {subs > 0 && !editSubs && <p className="hint">{t("pp.subsReadOnly")}</p>}

      {sub !== null && editSubs && !linked && (
        <>
          <TextField label={t("pp.subLayer")} value={entry.settings.name ?? ""} onCommit={(name) => setSub({ name: name.trim() || null })} />
          <CheckField
            label={t("pp.cleanupPass")}
            checked={entry.cleanup !== null}
            onChange={(on) => setSub({ cleanup: on ? { every_n_passes: Math.max(1, layer.settings.passes) } : null })}
          />
          {entry.cleanup && (
            <>
              <NumberField
                label={t("pp.cleanAfter")}
                unit={t("pp.passesUnit")}
                value={entry.cleanup.every_n_passes}
                min={1}
                max={1000}
                digits={0}
                onCommit={(v) => v !== null && setSub({ cleanup: { every_n_passes: Math.round(v) } })}
              />
              <p className="hint">
                {t(entry.cleanup.every_n_passes === 1 ? "pp.cleanupEvery1" : "pp.cleanupEveryN", { n: entry.cleanup.every_n_passes })}
              </p>
            </>
          )}
        </>
      )}

      {canEdit ? (
        <>
          <CutSettingsForm key={`${layer.id}#${pass}`} settings={entry.settings} machine={machine} onPatch={patch} />
          {/* Beginner Mode hides the advanced cut settings (tabs included), as LightBurn's Cut Settings Editor does. */}
          {!beginnerMode && <CutOptionsSection key={`opts-${layer.id}#${pass}`} layer={layer} sub={sub} settings={entry.settings} run={run} report={report} has={has} gantry={gantryOf(machine)} hasImages={hasImages} />}
        </>
      ) : (
        <dl className="kv">
          <dt>{t("pp.settings")}</dt>
          <dd>{settingsSummary(entry.settings, speedUnit)}</dd>
        </dl>
      )}
    </>
  );
}

/** Full cut-settings form for one pass. Each field sends one partial settings patch. */
function CutSettingsForm({ settings: s, machine, onPatch }: { settings: CutSettings; machine: MachineStatus | null; onPatch: (p: C.CutSettingsPatch) => unknown }) {
  const t = useT();
  const set = (p: C.CutSettingsPatch) => void onPatch(p);
  const num = (f: (v: number) => C.CutSettingsPatch) => (v: number | null) => v !== null && set(f(v));
  const hatching = s.mode !== "line";
  const widths = pulseWidths(machine);
  const freq = frequencyRange(machine);
  const pw = s.pulse_width_ns;
  const pwKnown = pw === null || widths.includes(pw);
  // Only the source settings the active device type takes (a GRBL or Ruida gantry has no frequency or pulse width).
  const fields = sourceFields(machine);
  return (
    <>
      <label className="field">
        <span className="field-label">{t("pp.mode")}</span>
        <span className="field-input">
          <select value={s.mode} onChange={(e) => set({ mode: e.target.value as FillMode })}>
            {MODES.map((m) => (
              <option key={m.value} value={m.value}>
                {t(`pp.mode.${m.value}` as "pp.mode.line")}
              </option>
            ))}
          </select>
        </span>
      </label>
      <NumberField label={t("pp.power")} unit="%" value={s.power_pct} min={0} max={100} digits={1} stepper onCommit={num((power_pct) => ({ power_pct }))} />
      <NumberField label={t("pp.speed")} unit="mm/s" value={s.speed_mm_s} min={0.1} digits={1} stepper onCommit={num((speed_mm_s) => ({ speed_mm_s }))} />
      {fields.frequency && (
        <NumberField
          label={t("pp.frequency")}
          unit="kHz"
          value={s.frequency_khz}
          min={freq.min}
          max={freq.max}
          digits={1}
          stepper
          onCommit={num((frequency_khz) => ({ frequency_khz }))}
        />
      )}
      {fields.pulseWidth && (
      <label className="field">
        <span className="field-label">{t("pp.pulseWidth")}</span>
        <span className="field-input">
          <select
            value={pw === null ? "" : String(pw)}
            onChange={(e) => set({ pulse_width_ns: e.target.value === "" ? null : Number(e.target.value) })}
            title={t("pp.pulseTitle")}
          >
            <option value="">{t("pp.sourceDefault")}</option>
            {!pwKnown && pw !== null && <option value={String(pw)}>{t("pp.pwUnsupported", { pw, nearest: nearestPulseWidth(pw, widths) })}</option>}
            {widths.map((v) => (
              <option key={v} value={String(v)}>
                {v} ns
              </option>
            ))}
          </select>
        </span>
      </label>
      )}
      {fields.pulseWidth && !pwKnown && pw !== null && <p className="warn-text">{t("pp.pwPick", { pw })}</p>}
      <NumberField label={t("pp.passes")} value={s.passes} min={1} max={1000} digits={0} stepper onCommit={num((v) => ({ passes: Math.round(v) }))} />

      <h4 className={hatching ? "" : "muted"}>{hatching ? t("pp.hatch") : t("pp.hatchFillOnly")}</h4>
      <NumberField label={t("pp.interval")} unit="mm" value={s.hatch.interval_mm} min={0.001} digits={4} onCommit={num((interval_mm) => ({ hatch: { interval_mm } }))} />
      {s.mode === "offset_fill" && <p className="hint">{t("pp.offsetHint")}</p>}
      <NumberField label={t("pp.angle")} unit="°" value={s.hatch.angle_deg} digits={2} onCommit={num((angle_deg) => ({ hatch: { angle_deg } }))} />
      <NumberField
        label={t("pp.increment")}
        unit={t("pp.degPerPass")}
        value={s.hatch.angle_increment_deg}
        digits={2}
        onCommit={num((angle_increment_deg) => ({ hatch: { angle_increment_deg } }))}
      />
      <CheckField label={t("pp.crossHatch")} checked={s.hatch.cross_hatch} onChange={(cross_hatch) => set({ hatch: { cross_hatch } })} />
      <CheckField label={t("pp.bidirectional")} checked={s.hatch.bidirectional} onChange={(bidirectional) => set({ hatch: { bidirectional } })} />
    </>
  );
}

function round6(v: number): number {
  return Math.round(v * 1e6) / 1e6;
}

/** `<input type=color>` only accepts #rrggbb. */
function toHex(c: string): string {
  if (/^#[0-9a-f]{6}$/i.test(c)) return c;
  if (/^#[0-9a-f]{3}$/i.test(c)) return `#${c[1]}${c[1]}${c[2]}${c[2]}${c[3]}${c[3]}`;
  return "#000000";
}
