// Right "Properties" panel: the selected object(s), the stock, and the selected layer's cut settings. Every field
// commits one command (`object.transform`, `object.set_layer`, `stock.move`, `layer.update`) on Enter or when focus
// leaves it; X and Y commit together as one step. Numbers accept simple maths (`20/3`).

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { boxOutside, frequencyRange, nearestPulseWidth, pulseWidths } from "../api/laser.ts";
import type { CutSettings, FillMode, GDocument, Layer, MachineStatus, Vec2 } from "../api/types.ts";
import { roundMm } from "../editing.ts";
import { normDeg, objectBounds, transformParts, unionBoxes, type Box } from "../geometry.ts";
import { topGroup, unitsOf, type Selection } from "../selection.ts";
import type { ArrangeAction } from "./Toolbar.tsx";
import { CheckField, ColorField, Icon, NumberField, PairField, TextField, fmt } from "./ui.tsx";

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
  const { doc, selection } = props;
  if (!doc) return <div className="panel-empty">No document.</div>;
  const layer = selection.layer ? doc.layers.find((l) => l.id === selection.layer) : undefined;
  const nothing = selection.objects.length === 0 && !layer && !selection.stock;
  return (
    <div className="props">
      {selection.objects.length > 0 && <ObjectSection {...props} doc={doc} />}
      {selection.stock && doc.stock && <StockSection {...props} doc={doc} />}
      {layer && <LayerSection layer={layer} machine={props.machine} run={props.run} report={props.report} doc={doc} has={props.has} />}
      {nothing && (
        <div className="panel-empty">
          Select an object, layer or the stock. Drag on empty canvas to box-select (left to right encloses, right to left
          crosses); Shift adds, Ctrl toggles. Drop files on the window to import them.
        </div>
      )}
    </div>
  );
}

/** Reference point for X/Y (LightBurn's 9-point anchor). */
type Anchor = "nw" | "n" | "ne" | "w" | "c" | "e" | "sw" | "s" | "se";
const ANCHORS: Anchor[] = ["nw", "n", "ne", "w", "c", "e", "sw", "s", "se"];

function anchorPoint(b: Box, a: Anchor): Vec2 {
  const x = a.includes("w") ? b.x0 : a.includes("e") ? b.x1 : (b.x0 + b.x1) / 2;
  const y = a.startsWith("s") ? b.y0 : a.startsWith("n") ? b.y1 : (b.y0 + b.y1) / 2;
  return [x, y];
}

function ObjectSection({ doc, selection, run, report, onArrange, workspaceMm }: PropertiesPanelProps & { doc: GDocument }) {
  const [lockAspect, setLockAspect] = useState(true);
  const [anchor, setAnchor] = useState<Anchor>("c");
  const objs = doc.objects.filter((o) => selection.objects.includes(o.id));
  const bounds = unionBoxes(objs.map(objectBounds));
  if (objs.length === 0 || !bounds) return null;
  const single = objs.length === 1 ? objs[0] : undefined;
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
    if (lockAspect) {
      if (nw !== null) sy = sx;
      else sx = sy;
    }
    if (!(sx > 0 && sy > 0)) return;
    // Scale about the anchor, so the reference point stays where it is.
    exec(C.transform(ids, { scale: [round6(sx), round6(sy)], about: [roundMm(ax), roundMm(ay)] }));
  };

  return (
    <section className="prop-section">
      <h3>{single ? "Object" : group ? `Group "${group.name}"` : `${units.length > 1 ? `${units.length} items` : "Group"} · ${objs.length} objects`}</h3>
      {outside && <p className="warn-text">Part of the selection is outside the galvo field and cannot be marked.</p>}
      {single && <TextField label="Name" value={single.name} onCommit={(name) => exec(C.rename(single.id, name))} />}
      <label className="field">
        <span className="field-label">Layer</span>
        <span className="field-input">
          <select
            value={layers.size === 1 ? objs[0].layer : ""}
            onChange={(e) => e.target.value && exec(C.setLayer(ids, e.target.value))}
          >
            {layers.size > 1 && <option value="">Several layers</option>}
            {doc.layers.map((l) => (
              <option key={l.id} value={l.id}>
                {l.name}
              </option>
            ))}
          </select>
        </span>
      </label>
      <div className="pos-row">
        <span className="pos-label">Reference</span>
        <div className="anchor-grid small" role="radiogroup" aria-label="Reference point">
          {ANCHORS.map((a) => (
            <button key={a} role="radio" aria-checked={a === anchor} className={a === anchor ? "active" : ""} title={`X/Y and scaling refer to this point (${a})`} onClick={() => setAnchor(a)}>
              <span className="anchor-dot" />
            </button>
          ))}
        </div>
      </div>
      <PairField labels={["X", "Y"]} unit="mm" value={[ax, ay]} onCommit={(v) => setPos(v[0], v[1])} />
      <div className="field-grid">
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
          if (Math.abs(delta) > 1e-9) exec(C.transform(ids, { rotate_deg: round6(delta), about: [roundMm(ax), roundMm(ay)] }));
        }}
      />
      {parts && (
        <dl className="kv">
          <dt>Flip</dt>
          <dd>{parts.flip === "horizontal" ? "Horizontal" : parts.flip === "vertical" ? "Vertical" : "None"}</dd>
        </dl>
      )}
      <div className="btn-row compact">
        <button className="icon-btn small" title="Flip horizontal (Ctrl+Shift+H)" aria-label="Flip horizontal" onClick={() => onArrange("flipH")}>
          <Icon name="flipH" size={18} />
        </button>
        <button className="icon-btn small" title="Flip vertical (Ctrl+Shift+V)" aria-label="Flip vertical" onClick={() => onArrange("flipV")}>
          <Icon name="flipV" size={18} />
        </button>
        <button className="icon-btn small" title="Centre on stock" aria-label="Centre on stock" disabled={!doc.stock} onClick={() => onArrange("stock")}>
          <Icon name="centerStock" size={18} />
        </button>
        <button className="icon-btn small" title="Centre on field (P)" aria-label="Centre on field" onClick={() => onArrange("field")}>
          <Icon name="centerField" size={18} />
        </button>
        {units.length >= 2 && (
          <>
            <button className="icon-btn small" title="Align left (L)" aria-label="Align left" onClick={() => onArrange("left")}>
              <Icon name="alignLeft" size={18} />
            </button>
            <button className="icon-btn small" title="Align horizontal centres (C)" aria-label="Align horizontal centres" onClick={() => onArrange("hcenter")}>
              <Icon name="alignHCenter" size={18} />
            </button>
            <button className="icon-btn small" title="Align vertical centres (E)" aria-label="Align vertical centres" onClick={() => onArrange("vcenter")}>
              <Icon name="alignVCenter" size={18} />
            </button>
          </>
        )}
      </div>
      <p className="hint">
        X/Y are the chosen reference point of the bounding box in the field frame. Arrows nudge 5 mm (Ctrl 1, Shift 20,
        Ctrl+Shift 0.1); <kbd>.</kbd> and <kbd>,</kbd> rotate 90°.
      </p>
    </section>
  );
}

function StockSection({ doc, has, run, report, onEditStock }: PropertiesPanelProps & { doc: GDocument }) {
  const s = doc.stock;
  if (!s) return null;
  const exec = (inv: Invocation) => run(inv).catch(report);
  const move = (p: { center?: Vec2; rotation_deg?: number }) => {
    if (has(C.OPT.stockMove)) exec(C.stockMove(p));
    else exec(C.stockDefine({ ...s, center: p.center ?? s.center, rotation_deg: p.rotation_deg ?? s.rotation_deg }));
  };
  const dims = s.shape.kind === "rect" ? `${fmt(s.shape.width_mm)} × ${fmt(s.shape.height_mm)} mm` : `Ø ${fmt(s.shape.diameter_mm)} mm`;
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
      </dl>
      <PairField labels={["Centre X", "Centre Y"]} unit="mm" value={s.center} onCommit={(c) => move({ center: [roundMm(c[0]), roundMm(c[1])] })} />
      <NumberField label="Rotation" unit="°" value={s.rotation_deg} digits={2} onCommit={(v) => v !== null && move({ rotation_deg: round6(normDeg(v)) })} />
      <p className="hint">While the stock is selected, drag it on the canvas or nudge it with the arrow keys.</p>
      <div className="btn-row">
        <button className="btn" onClick={onEditStock}>
          Edit stock…
        </button>
        {has(C.OPT.stockClear) && (
          <button className="btn" onClick={() => exec(C.stockClear())}>
            Remove stock
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
];

/** Full cut-settings form for one layer. Each field sends a partial `layer.update`. */
export function LayerSection({
  layer,
  machine,
  run,
  report,
  doc,
  has,
}: {
  layer: Layer;
  machine: MachineStatus | null;
  run: Run;
  report: (e: unknown) => void;
  doc?: GDocument;
  has?: (cmd: string) => boolean;
}) {
  const s: CutSettings = layer.settings;
  const set = (patch: C.CutSettingsPatch) => run(C.layerUpdate(layer.id, { settings: patch })).catch(report);
  const setL = (patch: { name?: string; color?: string; visible?: boolean; output?: boolean }) => run(C.layerUpdate(layer.id, patch)).catch(report);
  const num = (f: (v: number) => C.CutSettingsPatch) => (v: number | null) => v !== null && set(f(v));
  const hatching = s.mode !== "line";
  const widths = pulseWidths(machine);
  const freq = frequencyRange(machine);
  const pw = s.pulse_width_ns;
  const pwKnown = pw === null || widths.includes(pw);
  const others = doc ? doc.layers.filter((l) => l.id !== layer.id) : [];
  const count = doc ? doc.objects.filter((o) => o.layer === layer.id).length : 0;
  const [confirmDelete, setConfirmDelete] = useState(false);
  return (
    <section className="prop-section">
      <h3>
        <span className="swatch" style={{ background: layer.color }} /> Layer
      </h3>
      <TextField label="Name" value={layer.name} onCommit={(name) => setL({ name })} />
      <ColorField label="Colour" value={toHex(layer.color)} onCommit={(color) => setL({ color })} />
      <CheckField label="Show" checked={layer.visible} onChange={(visible) => setL({ visible })} />
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
      <NumberField
        label="Frequency"
        unit="kHz"
        value={s.frequency_khz}
        min={freq.min}
        max={freq.max}
        digits={1}
        onCommit={num((frequency_khz) => ({ frequency_khz }))}
      />
      <label className="field">
        <span className="field-label">Pulse width</span>
        <span className="field-input">
          <select
            value={pw === null ? "" : String(pw)}
            onChange={(e) => set({ pulse_width_ns: e.target.value === "" ? null : Number(e.target.value) })}
            title="The MOPA source only accepts these pulse widths"
          >
            <option value="">Source default</option>
            {!pwKnown && pw !== null && <option value={String(pw)}>{pw} ns (not supported; nearest {nearestPulseWidth(pw, widths)} ns)</option>}
            {widths.map((v) => (
              <option key={v} value={String(v)}>
                {v} ns
              </option>
            ))}
          </select>
        </span>
      </label>
      {!pwKnown && pw !== null && <p className="warn-text">The source does not take {pw} ns; pick one from the list.</p>}
      <NumberField label="Passes" value={s.passes} min={1} max={1000} digits={0} onCommit={num((v) => ({ passes: Math.round(v) }))} />

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
      {doc && has?.(C.OPT.layerDelete) && others.length > 0 && (
        <div className="btn-row">
          {!confirmDelete ? (
            <button className="btn" onClick={() => setConfirmDelete(true)}>
              Delete layer…
            </button>
          ) : (
            <>
              <span className="hint">{count ? `Its ${count} object${count === 1 ? "" : "s"} move to "${others[0].name}".` : "The layer is empty."}</span>
              <button className="btn" onClick={() => setConfirmDelete(false)}>
                Cancel
              </button>
              <button
                className="btn btn-warn"
                onClick={() => {
                  setConfirmDelete(false);
                  run(C.layerDelete(layer.id, count ? others[0].id : undefined)).catch(report);
                }}
              >
                Delete
              </button>
            </>
          )}
        </div>
      )}
    </section>
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
