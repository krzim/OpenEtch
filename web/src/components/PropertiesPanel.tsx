// Right "Properties" panel: the selected object(s), the stock, and the selected layer's cut settings. Every field
// commits one command (`object.transform`, `object.update`, `layer.update`) on Enter or blur.

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import type { CutSettings, FillMode, GDocument, Layer } from "../api/types.ts";
import { boxCenter, boxSize, normDeg, objectBounds, rotationDeg, unionBoxes } from "../geometry.ts";
import type { Selection } from "../selection.ts";
import { CheckField, ColorField, NumberField, TextField, fmt } from "./ui.tsx";

export interface PropertiesPanelProps {
  doc: GDocument | null;
  selection: Selection;
  run: (inv: Invocation) => Promise<unknown>;
  report: (e: unknown) => void;
  onEditStock: () => void;
}

export function PropertiesPanel(props: PropertiesPanelProps) {
  const { doc, selection } = props;
  if (!doc) return <div className="panel-empty">No document.</div>;
  const layer = selection.layer ? doc.layers.find((l) => l.id === selection.layer) : undefined;
  const nothing = selection.objects.length === 0 && !layer && !selection.stock;
  return (
    <div className="props">
      {selection.objects.length > 0 && <ObjectSection {...props} doc={doc} />}
      {selection.stock && doc.stock && <StockSection doc={doc} onEdit={props.onEditStock} />}
      {layer && <LayerSection layer={layer} run={props.run} report={props.report} />}
      {nothing && <div className="panel-empty">Select an object, layer or the stock.</div>}
    </div>
  );
}

function ObjectSection({ doc, selection, run, report }: PropertiesPanelProps & { doc: GDocument }) {
  const [lockAspect, setLockAspect] = useState(true);
  const objs = doc.objects.filter((o) => selection.objects.includes(o.id));
  const bounds = unionBoxes(objs.map(objectBounds));
  if (objs.length === 0 || !bounds) return null;
  const single = objs.length === 1 ? objs[0] : undefined;
  const ids = objs.map((o) => o.id);
  const [cx, cy] = boxCenter(bounds);
  const [w, h] = boxSize(bounds);
  const rot = single ? normDeg(rotationDeg(single.transform)) : 0;
  const exec = (inv: Invocation) => run(inv).catch(report);

  const setPos = (nx: number | null, ny: number | null) =>
    exec(C.transform(ids, { translate: [nx === null ? 0 : nx - cx, ny === null ? 0 : ny - cy] }));

  const setSize = (nw: number | null, nh: number | null) => {
    let sx = nw !== null && w > 1e-9 ? nw / w : 1;
    let sy = nh !== null && h > 1e-9 ? nh / h : 1;
    if (lockAspect) {
      if (nw !== null) sy = sx;
      else sx = sy;
    }
    if (!(sx > 0 && sy > 0)) return;
    exec(C.transform(ids, { scale: [sx, sy], about: [cx, cy] }));
  };

  return (
    <section className="prop-section">
      <h3>{single ? "Object" : `${objs.length} objects`}</h3>
      {single && <TextField label="Name" value={single.name} onCommit={(name) => exec(C.rename(single.id, name))} />}
      {single && (
        <label className="field">
          <span className="field-label">Layer</span>
          <span className="field-input">
            <select value={single.layer} onChange={(e) => exec(C.setLayer([single.id], e.target.value))}>
              {doc.layers.map((l) => (
                <option key={l.id} value={l.id}>
                  {l.name}
                </option>
              ))}
            </select>
          </span>
        </label>
      )}
      <div className="field-grid">
        <NumberField label="X" unit="mm" value={cx} onCommit={(v) => setPos(v, null)} />
        <NumberField label="Y" unit="mm" value={cy} onCommit={(v) => setPos(null, v)} />
        <NumberField label="W" unit="mm" value={w} min={0.01} onCommit={(v) => setSize(v, null)} />
        <NumberField label="H" unit="mm" value={h} min={0.01} onCommit={(v) => setSize(null, v)} />
      </div>
      <CheckField label="Lock aspect" checked={lockAspect} onChange={setLockAspect} />
      <NumberField
        label={single ? "Rotation" : "Rotate by"}
        unit="°"
        value={rot}
        digits={2}
        onCommit={(v) => {
          if (v === null) return;
          const delta = single ? normDeg(v - rot) : v;
          if (Math.abs(delta) > 1e-9) exec(C.transform(ids, { rotate_deg: delta, about: [cx, cy] }));
        }}
      />
      <p className="hint">X/Y are the centre of the bounding box in the field frame.</p>
    </section>
  );
}

function StockSection({ doc, onEdit }: { doc: GDocument; onEdit: () => void }) {
  const s = doc.stock;
  if (!s) return null;
  const dims =
    s.shape.kind === "rect"
      ? `${fmt(s.shape.width_mm)} × ${fmt(s.shape.height_mm)} mm`
      : `Ø ${fmt(s.shape.diameter_mm)} mm`;
  return (
    <section className="prop-section">
      <h3>Stock</h3>
      <dl className="kv">
        <dt>Name</dt>
        <dd>{s.name}</dd>
        <dt>Shape</dt>
        <dd>{dims}</dd>
        <dt>Thickness</dt>
        <dd>{fmt(s.thickness_mm)} mm</dd>
        <dt>Material</dt>
        <dd>{s.material ?? "—"}</dd>
        <dt>Centre</dt>
        <dd>
          {fmt(s.center[0])}, {fmt(s.center[1])} mm
        </dd>
        <dt>Rotation</dt>
        <dd>{fmt(s.rotation_deg, 2)}°</dd>
      </dl>
      <button className="btn" onClick={onEdit}>
        Edit stock…
      </button>
    </section>
  );
}

const MODES: { value: FillMode; label: string }[] = [
  { value: "line", label: "Line" },
  { value: "fill", label: "Fill" },
  { value: "fill_then_line", label: "Fill + line" },
];

/** Full cut-settings form for one layer. Each field sends a partial `layer.update`. */
export function LayerSection({ layer, run, report }: { layer: Layer; run: (inv: Invocation) => Promise<unknown>; report: (e: unknown) => void }) {
  const s: CutSettings = layer.settings;
  const set = (patch: C.CutSettingsPatch) => run(C.layerUpdate(layer.id, { settings: patch })).catch(report);
  const setL = (patch: { name?: string; color?: string; visible?: boolean; output?: boolean }) =>
    run(C.layerUpdate(layer.id, patch)).catch(report);
  const num = (f: (v: number) => C.CutSettingsPatch) => (v: number | null) => v !== null && set(f(v));
  const hatching = s.mode !== "line";
  return (
    <section className="prop-section">
      <h3>
        <span className="swatch" style={{ background: layer.color }} /> Layer
      </h3>
      <TextField label="Name" value={layer.name} onCommit={(name) => setL({ name })} />
      <ColorField label="Colour" value={toHex(layer.color)} onCommit={(color) => setL({ color })} />
      <CheckField label="Visible" checked={layer.visible} onChange={(visible) => setL({ visible })} />
      <CheckField label="Output" checked={layer.output} onChange={(output) => setL({ output })} />

      <h4>Cut settings</h4>
      <label className="field">
        <span className="field-label">Mode</span>
        <span className="field-input">
          <select value={s.mode} onChange={(e) => set({ mode: e.target.value as FillMode })}>
            {MODES.map((m) => (
              <option key={m.value} value={m.value}>
                {m.label}
              </option>
            ))}
          </select>
        </span>
      </label>
      <NumberField label="Power" unit="%" value={s.power_pct} min={0} max={100} digits={1} onCommit={num((power_pct) => ({ power_pct }))} />
      <NumberField label="Speed" unit="mm/s" value={s.speed_mm_s} min={0.1} digits={1} onCommit={num((speed_mm_s) => ({ speed_mm_s }))} />
      <NumberField label="Frequency" unit="kHz" value={s.frequency_khz} min={1} digits={1} onCommit={num((frequency_khz) => ({ frequency_khz }))} />
      <NumberField
        label="Pulse width"
        unit="ns"
        value={s.pulse_width_ns}
        min={1}
        digits={0}
        allowEmpty
        placeholder="source default"
        onCommit={(pulse_width_ns) => set({ pulse_width_ns })}
      />
      <NumberField label="Passes" value={s.passes} min={1} digits={0} onCommit={num((v) => ({ passes: Math.round(v) }))} />

      <h4 className={hatching ? "" : "muted"}>Hatch{hatching ? "" : " (fill modes only)"}</h4>
      <NumberField label="Interval" unit="mm" value={s.hatch.interval_mm} min={0.001} digits={4} onCommit={num((interval_mm) => ({ hatch: { interval_mm } }))} />
      <NumberField label="Angle" unit="°" value={s.hatch.angle_deg} digits={2} onCommit={num((angle_deg) => ({ hatch: { angle_deg } }))} />
      <NumberField
        label="Increment"
        unit="°/pass"
        value={s.hatch.angle_increment_deg}
        digits={2}
        onCommit={num((angle_increment_deg) => ({ hatch: { angle_increment_deg } }))}
      />
      <CheckField label="Cross hatch" checked={s.hatch.cross_hatch} onChange={(cross_hatch) => set({ hatch: { cross_hatch } })} />
      <CheckField label="Bidirectional" checked={s.hatch.bidirectional} onChange={(bidirectional) => set({ hatch: { bidirectional } })} />
      {layer.sub_layers.length > 0 && <p className="hint">{layer.sub_layers.length} sub-layer pass(es) not shown.</p>}
    </section>
  );
}

/** `<input type=color>` only accepts #rrggbb. */
function toHex(c: string): string {
  if (/^#[0-9a-f]{6}$/i.test(c)) return c;
  if (/^#[0-9a-f]{3}$/i.test(c)) return `#${c[1]}${c[1]}${c[2]}${c[2]}${c[3]}${c[3]}`;
  return "#000000";
}
