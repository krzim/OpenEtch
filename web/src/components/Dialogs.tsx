// Modal dialogs for the Design toolbar (Stock, Place on stock, Array) and a command palette that can run any
// registered command with raw JSON parameters, the same catalogue an AI agent sees over MCP.

import { useMemo, useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { isSquare, workspaceText } from "../api/devices.ts";
import { createdIds } from "../api/drop.ts";
import { paramsTemplate } from "../api/schema.ts";
import type { CommandSpec, GDocument, StockShape, Vec2 } from "../api/types.ts";
import { evalNumber } from "../editing.ts";
import { boxSize, objectBounds, unionBoxes } from "../geometry.ts";
import { Modal } from "./ui.tsx";

type Run = (inv: Invocation) => Promise<unknown>;

/** Plain number input for dialogs: keeps the text, parses on submit. */
function Num(props: { label: string; value: string; onChange: (v: string) => void; unit?: string }) {
  return (
    <label className="field">
      <span className="field-label">{props.label}</span>
      <span className="field-input">
        <input type="text" inputMode="decimal" value={props.value} onChange={(e) => props.onChange(e.target.value)} />
        {props.unit && <span className="unit">{props.unit}</span>}
      </span>
    </label>
  );
}

/** A typed number (simple maths allowed), or null. */
function parse(v: string): number | null {
  return evalNumber(v);
}

/** Footer with Cancel and a primary action that runs `submit` and closes on success. */
function useSubmit(run: Run, onClose: () => void, onDone?: (result: unknown) => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async (inv: Invocation | string) => {
    if (busy) return;
    if (typeof inv === "string") {
      setError(inv);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const r = await run(inv);
      onDone?.(r);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, submit };
}

export function StockDialog({ doc, run, onClose }: { doc: GDocument | null; run: Run; onClose: () => void }) {
  const s = doc?.stock;
  const [kind, setKind] = useState<StockShape["kind"]>(s?.shape.kind ?? "rect");
  const [name, setName] = useState(s?.name ?? "Stock");
  const [w, setW] = useState(String(s?.shape.kind === "rect" ? s.shape.width_mm : 50));
  const [h, setH] = useState(String(s?.shape.kind === "rect" ? s.shape.height_mm : 30));
  const [r, setR] = useState(String(s?.shape.kind === "rect" ? s.shape.corner_radius_mm : 0));
  const [dia, setDia] = useState(String(s?.shape.kind === "circle" ? s.shape.diameter_mm : 30));
  const [thick, setThick] = useState(String(s?.thickness_mm ?? 1));
  const [material, setMaterial] = useState(s?.material ?? "");
  const [cx, setCx] = useState(String(s?.center[0] ?? 0));
  const [cy, setCy] = useState(String(s?.center[1] ?? 0));
  const [rot, setRot] = useState(String(s?.rotation_deg ?? 0));
  const { busy, error, submit } = useSubmit(run, onClose);

  const build = (): Invocation | string => {
    const t = parse(thick);
    const center: Vec2 = [parse(cx) ?? NaN, parse(cy) ?? NaN];
    const rotation = parse(rot);
    if (t === null || t < 0) return "Thickness must be a number ≥ 0.";
    if (!Number.isFinite(center[0]) || !Number.isFinite(center[1]) || rotation === null) return "Centre and rotation must be numbers.";
    let shape: StockShape;
    if (kind === "rect") {
      const W = parse(w);
      const H = parse(h);
      const R = parse(r) ?? 0;
      if (W === null || H === null || W <= 0 || H <= 0) return "Width and height must be > 0.";
      shape = { kind: "rect", width_mm: W, height_mm: H, corner_radius_mm: Math.max(0, R) };
    } else {
      const D = parse(dia);
      if (D === null || D <= 0) return "Diameter must be > 0.";
      shape = { kind: "circle", diameter_mm: D };
    }
    return C.stockDefine({ name: name.trim() || "Stock", shape, thickness_mm: t, material: material.trim() || null, center, rotation_deg: rotation });
  };

  return (
    <Modal
      title={s ? "Edit stock" : "Stock"}
      onClose={onClose}
      onSubmit={() => submit(build())}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => submit(build())}>
            OK
          </button>
        </>
      }
    >
      <div className="segmented">
        <button className={kind === "rect" ? "active" : ""} onClick={() => setKind("rect")}>
          Rectangle
        </button>
        <button className={kind === "circle" ? "active" : ""} onClick={() => setKind("circle")}>
          Circle
        </button>
      </div>
      <label className="field">
        <span className="field-label">Name</span>
        <span className="field-input">
          <input type="text" value={name} onChange={(e) => setName(e.target.value)} />
        </span>
      </label>
      {kind === "rect" ? (
        <div className="field-grid">
          <Num label="Width" unit="mm" value={w} onChange={setW} />
          <Num label="Height" unit="mm" value={h} onChange={setH} />
          <Num label="Corner r" unit="mm" value={r} onChange={setR} />
        </div>
      ) : (
        <Num label="Diameter" unit="mm" value={dia} onChange={setDia} />
      )}
      <Num label="Thickness" unit="mm" value={thick} onChange={setThick} />
      <label className="field">
        <span className="field-label">Material</span>
        <span className="field-input">
          <input type="text" value={material} placeholder="e.g. stainless 304" onChange={(e) => setMaterial(e.target.value)} />
        </span>
      </label>
      <div className="field-grid">
        <Num label="Centre X" unit="mm" value={cx} onChange={setCx} />
        <Num label="Centre Y" unit="mm" value={cy} onChange={setCy} />
        <Num label="Rotation" unit="°" value={rot} onChange={setRot} />
      </div>
      <p className="hint">The B6's Z height is not readable over USB; thickness is used for focus notes only.</p>
    </Modal>
  );
}

const ANCHORS: C.Anchor[] = ["top_left", "top", "top_right", "left", "center", "right", "bottom_left", "bottom", "bottom_right"];

type FitMode = C.Fit["mode"];

export function PlaceDialog({ ids, run, onClose }: { ids: string[]; run: Run; onClose: () => void }) {
  const [anchor, setAnchor] = useState<C.Anchor>("center");
  const [mode, setMode] = useState<FitMode>("none");
  const [margin, setMargin] = useState("2");
  const [width, setWidth] = useState("20");
  const [height, setHeight] = useState("20");
  const [keepAspect, setKeepAspect] = useState(true);
  const [ox, setOx] = useState("0");
  const [oy, setOy] = useState("0");
  const { busy, error, submit } = useSubmit(run, onClose);
  const build = (): Invocation | string => {
    const offset: Vec2 = [parse(ox) ?? NaN, parse(oy) ?? NaN];
    if (!Number.isFinite(offset[0]) || !Number.isFinite(offset[1])) return "Offset must be numbers.";
    const pos = (v: string, what: string): number | string => {
      const n = parse(v);
      return n === null || n <= 0 ? `${what} must be > 0.` : n;
    };
    let fit: C.Fit;
    switch (mode) {
      case "none":
        fit = { mode: "none" };
        break;
      case "contain": {
        const m = parse(margin);
        if (m === null || m < 0) return "Margin must be a number ≥ 0.";
        fit = { mode: "contain", margin_mm: m };
        break;
      }
      case "width": {
        const w = pos(width, "Width");
        if (typeof w === "string") return w;
        fit = { mode: "width", width_mm: w };
        break;
      }
      case "height": {
        const h = pos(height, "Height");
        if (typeof h === "string") return h;
        fit = { mode: "height", height_mm: h };
        break;
      }
      case "exact": {
        const w = pos(width, "Width");
        const h = pos(height, "Height");
        if (typeof w === "string") return w;
        if (typeof h === "string") return h;
        fit = { mode: "exact", width_mm: w, height_mm: h, keep_aspect: keepAspect };
        break;
      }
    }
    return C.place(ids, anchor, fit, offset);
  };
  return (
    <Modal
      title={`Place ${ids.length} object${ids.length === 1 ? "" : "s"} on stock`}
      onClose={onClose}
      onSubmit={() => submit(build())}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => submit(build())}>
            Place
          </button>
        </>
      }
    >
      <div className="field">
        <span className="field-label">Anchor</span>
        <div className="anchor-grid">
          {ANCHORS.map((a) => (
            <button key={a} className={a === anchor ? "active" : ""} title={a.replace("_", " ")} aria-label={a.replace("_", " ")} onClick={() => setAnchor(a)}>
              <span className="anchor-dot" />
            </button>
          ))}
        </div>
      </div>
      <label className="field">
        <span className="field-label">Fit</span>
        <span className="field-input">
          <select value={mode} onChange={(e) => setMode(e.target.value as FitMode)}>
            <option value="none">Keep size</option>
            <option value="contain">Scale to fit inside stock</option>
            <option value="width">Scale to width</option>
            <option value="height">Scale to height</option>
            <option value="exact">Exact size</option>
          </select>
        </span>
      </label>
      {mode === "contain" && <Num label="Margin" unit="mm" value={margin} onChange={setMargin} />}
      {(mode === "width" || mode === "exact") && <Num label="Width" unit="mm" value={width} onChange={setWidth} />}
      {(mode === "height" || mode === "exact") && <Num label="Height" unit="mm" value={height} onChange={setHeight} />}
      {mode === "exact" && (
        <label className="field field-check">
          <span className="field-label">Keep aspect</span>
          <input type="checkbox" checked={keepAspect} onChange={(e) => setKeepAspect(e.target.checked)} />
        </label>
      )}
      <div className="field-grid">
        <Num label="Offset X" unit="mm" value={ox} onChange={setOx} />
        <Num label="Offset Y" unit="mm" value={oy} onChange={setOy} />
      </div>
      <p className="hint">Anchor and offset are in the stock's own frame; "top" is the stock's +Y side.</p>
    </Modal>
  );
}

export function ArrayDialog({
  doc,
  ids,
  workspaceMm,
  run,
  onClose,
  onCreated,
}: {
  doc: GDocument;
  ids: string[];
  /** The active device's workspace (W x H mm, centred; a galvo field is square). */
  workspaceMm: Vec2;
  run: Run;
  onClose: () => void;
  /** The copies' ids, to select them with the originals (LightBurn selects the result). */
  onCreated?: (ids: string[]) => void;
}) {
  const b = unionBoxes(doc.objects.filter((o) => ids.includes(o.id)).map(objectBounds));
  const [bw, bh] = b ? boxSize(b) : [10, 10];
  const [cols, setCols] = useState("3");
  const [rows, setRows] = useState("1");
  const [dx, setDx] = useState(String(Number((bw + 2).toFixed(2))));
  const [dy, setDy] = useState(String(Number((bh + 2).toFixed(2))));
  const { busy, error, submit } = useSubmit(run, onClose, (r) => onCreated?.(createdIds(r)));
  // Warn before creating copies the galvo cannot reach.
  const nc = parse(cols);
  const nr = parse(rows);
  const px = parse(dx);
  const py = parse(dy);
  let outside = 0;
  if (b && nc !== null && nr !== null && px !== null && py !== null && nc >= 1 && nr >= 1 && nc * nr <= 2500) {
    const [hx, h] = [workspaceMm[0] / 2, workspaceMm[1] / 2];
    for (let i = 0; i < nc; i++)
      for (let j = 0; j < nr; j++) {
        const ox = i * px;
        const oy = j * py;
        if (b.x0 + ox <= -hx || b.x1 + ox >= hx || b.y0 + oy <= -h || b.y1 + oy >= h) outside++;
      }
  }
  const build = (): Invocation | string => {
    const c = parse(cols);
    const r = parse(rows);
    const x = parse(dx);
    const y = parse(dy);
    if (c === null || r === null || c < 1 || r < 1 || !Number.isInteger(c) || !Number.isInteger(r)) return "Columns and rows must be whole numbers ≥ 1.";
    if (c * r > 2500) return "That is more than 2500 copies.";
    if (x === null || y === null) return "Spacing must be numbers.";
    return C.arrayGrid(ids, c, r, [x, y]);
  };
  return (
    <Modal
      title="Rectangular array"
      onClose={onClose}
      onSubmit={() => submit(build())}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => submit(build())}>
            Create
          </button>
        </>
      }
    >
      <div className="field-grid">
        <Num label="Columns" value={cols} onChange={setCols} />
        <Num label="Rows" value={rows} onChange={setRows} />
        <Num label="Pitch X" unit="mm" value={dx} onChange={setDx} />
        <Num label="Pitch Y" unit="mm" value={dy} onChange={setDy} />
      </div>
      {outside > 0 && (
        <p className="warn-text" role="status">
          {outside} of the {(nc ?? 0) * (nr ?? 0)} cells would be outside the {workspaceText(workspaceMm)}{" "}
          {isSquare(workspaceMm) ? "galvo field" : "workspace"}. Use a smaller pitch or
          negative pitch, or move the original first.
        </p>
      )}
      <p className="hint">Counts include the original. Pitch is centre-to-centre; negative pitch goes left/down.</p>
    </Modal>
  );
}

export function CommandPalette({ commands, run, onClose }: { commands: CommandSpec[] | null; run: Run; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [picked, setPicked] = useState<CommandSpec | null>(null);
  const [params, setParams] = useState("{}");
  const [result, setResult] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const list = useMemo(() => {
    const t = q.trim().toLowerCase();
    // Machine actions are not document commands; the toolbar handles them.
    return (commands ?? []).filter(
      (c) => (c.risk === "query" || c.risk === "edit") && (!t || c.id.toLowerCase().includes(t) || c.title.toLowerCase().includes(t)),
    );
  }, [commands, q]);
  const go = async () => {
    if (!picked) return;
    let p: unknown;
    try {
      p = JSON.parse(params || "{}");
    } catch (e) {
      setResult(`Invalid JSON: ${e instanceof Error ? e.message : e}`);
      return;
    }
    if (typeof p !== "object" || p === null || Array.isArray(p)) {
      setResult("Parameters must be a JSON object.");
      return;
    }
    setBusy(true);
    try {
      const r = await run({ id: picked.id, params: p as Record<string, unknown> });
      setResult(JSON.stringify(r, null, 2) ?? "ok");
    } catch (e) {
      setResult(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal title="Commands" onClose={onClose} wide>
      <div className="palette">
        <div className="palette-list">
          <input type="search" placeholder="Filter commands" value={q} onChange={(e) => setQ(e.target.value)} />
          {commands === null && <p className="muted">Loading…</p>}
          {list.map((c) => (
            <button
              key={c.id}
              className={`palette-item${picked?.id === c.id ? " active" : ""}`}
              onClick={() => {
                setPicked(c);
                setResult(null);
                setParams(JSON.stringify(paramsTemplate(c.input_schema), null, 2));
              }}
            >
              <span className="palette-id">{c.id}</span>
              <span className="palette-title">{c.title}</span>
            </button>
          ))}
        </div>
        <div className="palette-detail">
          {!picked ? (
            <p className="muted">Pick a command. This runs exactly what an AI agent can run over MCP, tagged as your edit.</p>
          ) : (
            <>
              <h3>
                {picked.title} <span className={`badge risk-${picked.risk}`}>{picked.risk}</span>
              </h3>
              <p className="desc">{picked.description}</p>
              <details>
                <summary>Input schema</summary>
                <pre>{JSON.stringify(picked.input_schema, null, 2)}</pre>
              </details>
              <textarea spellCheck={false} value={params} onChange={(e) => setParams(e.target.value)} rows={8} />
              <div className="btn-row">
                <button className="btn btn-primary" disabled={busy} onClick={go}>
                  Run
                </button>
              </div>
              {result && <pre className="palette-result">{result}</pre>}
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}
