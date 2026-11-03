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
import { Modal, UnitTextField } from "./ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { errorText } from "../api/errorText.ts";

type Run = (inv: Invocation) => Promise<unknown>;

/** Number input for dialogs: keeps the text (mm), parses on submit; lengths show in the display unit. */
const Num = UnitTextField;

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
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, submit };
}

export function StockDialog({ doc, run, onClose }: { doc: GDocument | null; run: Run; onClose: () => void }) {
  const t = useT();
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
    const thickMm = parse(thick);
    const center: Vec2 = [parse(cx) ?? NaN, parse(cy) ?? NaN];
    const rotation = parse(rot);
    if (thickMm === null || thickMm < 0) return t("dlg.thicknessMustBeANumber0");
    if (!Number.isFinite(center[0]) || !Number.isFinite(center[1]) || rotation === null) return t("dlg.centreAndRotationMustBeNumbers");
    let shape: StockShape;
    if (kind === "rect") {
      const W = parse(w);
      const H = parse(h);
      const R = parse(r) ?? 0;
      if (W === null || H === null || W <= 0 || H <= 0) return t("dlg.widthAndHeightMustBe0");
      shape = { kind: "rect", width_mm: W, height_mm: H, corner_radius_mm: Math.max(0, R) };
    } else {
      const D = parse(dia);
      if (D === null || D <= 0) return t("dlg.diameterMustBe0");
      shape = { kind: "circle", diameter_mm: D };
    }
    return C.stockDefine({ name: name.trim() || "Stock", shape, thickness_mm: thickMm, material: material.trim() || null, center, rotation_deg: rotation });
  };

  return (
    <Modal
      title={s ? t("dlg.editStock") : "Stock"}
      onClose={onClose}
      onSubmit={() => submit(build())}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={onClose}>
            {t("dlg.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => submit(build())}>
            OK
          </button>
        </>
      }
    >
      <div className="segmented">
        <button className={kind === "rect" ? "active" : ""} onClick={() => setKind("rect")}>
          {t("dlg.rectangle")}
        </button>
        <button className={kind === "circle" ? "active" : ""} onClick={() => setKind("circle")}>
          {t("dlg.circle")}
        </button>
      </div>
      <label className="field">
        <span className="field-label">{t("dlg.name")}</span>
        <span className="field-input">
          <input type="text" value={name} onChange={(e) => setName(e.target.value)} />
        </span>
      </label>
      {kind === "rect" ? (
        <div className="field-grid">
          <Num label={t("dlg.width")} unit="mm" value={w} onChange={setW} />
          <Num label={t("dlg.height")} unit="mm" value={h} onChange={setH} />
          <Num label={t("dlg.cornerR")} unit="mm" value={r} onChange={setR} />
        </div>
      ) : (
        <Num label={t("dlg.diameter")} unit="mm" value={dia} onChange={setDia} />
      )}
      <Num label={t("dlg.thickness")} unit="mm" value={thick} onChange={setThick} />
      <label className="field">
        <span className="field-label">{t("dlg.material")}</span>
        <span className="field-input">
          <input type="text" value={material} placeholder={t("dlg.eGStainless304")} onChange={(e) => setMaterial(e.target.value)} />
        </span>
      </label>
      <div className="field-grid">
        <Num label={t("dlg.centreX")} unit="mm" value={cx} onChange={setCx} />
        <Num label={t("dlg.centreY")} unit="mm" value={cy} onChange={setCy} />
        <Num label={t("dlg.rotation")} unit="°" value={rot} onChange={setRot} />
      </div>
      <p className="hint">{t("dlg.theB6SZHeightIs")}</p>
    </Modal>
  );
}

const ANCHORS: C.Anchor[] = ["top_left", "top", "top_right", "left", "center", "right", "bottom_left", "bottom", "bottom_right"];

type FitMode = C.Fit["mode"];

export function PlaceDialog({ ids, run, onClose }: { ids: string[]; run: Run; onClose: () => void }) {
  const t = useT();
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
    if (!Number.isFinite(offset[0]) || !Number.isFinite(offset[1])) return t("dlg.offsetMustBeNumbers");
    const pos = (v: string, what: string): number | string => {
      const n = parse(v);
      return n === null || n <= 0 ? t("dlg.v1MustBe0", { v1: what }) : n;
    };
    let fit: C.Fit;
    switch (mode) {
      case "none":
        fit = { mode: "none" };
        break;
      case "contain": {
        const m = parse(margin);
        if (m === null || m < 0) return t("dlg.marginMustBeANumber0");
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
            {t("dlg.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => submit(build())}>
            {t("dlg.place")}
          </button>
        </>
      }
    >
      <div className="field">
        <span className="field-label">{t("dlg.anchor")}</span>
        <div className="anchor-grid">
          {ANCHORS.map((a) => (
            <button key={a} className={a === anchor ? "active" : ""} title={a.replace("_", " ")} aria-label={a.replace("_", " ")} onClick={() => setAnchor(a)}>
              <span className="anchor-dot" />
            </button>
          ))}
        </div>
      </div>
      <label className="field">
        <span className="field-label">{t("dlg.fit")}</span>
        <span className="field-input">
          <select value={mode} onChange={(e) => setMode(e.target.value as FitMode)}>
            <option value="none">{t("dlg.keepSize")}</option>
            <option value="contain">{t("dlg.scaleToFitInsideStock")}</option>
            <option value="width">{t("dlg.scaleToWidth")}</option>
            <option value="height">{t("dlg.scaleToHeight")}</option>
            <option value="exact">{t("dlg.exactSize")}</option>
          </select>
        </span>
      </label>
      {mode === "contain" && <Num label={t("dlg.margin")} unit="mm" value={margin} onChange={setMargin} />}
      {(mode === "width" || mode === "exact") && <Num label={t("dlg.width")} unit="mm" value={width} onChange={setWidth} />}
      {(mode === "height" || mode === "exact") && <Num label={t("dlg.height")} unit="mm" value={height} onChange={setHeight} />}
      {mode === "exact" && (
        <label className="field field-check">
          <span className="field-label">{t("dlg.keepAspect")}</span>
          <input type="checkbox" checked={keepAspect} onChange={(e) => setKeepAspect(e.target.checked)} />
        </label>
      )}
      <div className="field-grid">
        <Num label={t("dlg.offsetX")} unit="mm" value={ox} onChange={setOx} />
        <Num label={t("dlg.offsetY")} unit="mm" value={oy} onChange={setOy} />
      </div>
      <p className="hint">{t("dlg.anchorAndOffsetAreInThe")}</p>
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
  const t = useT();
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
    if (c === null || r === null || c < 1 || r < 1 || !Number.isInteger(c) || !Number.isInteger(r)) return t("dlg.columnsAndRowsMustBeWhole");
    if (c * r > 2500) return t("dlg.thatIsMoreThan2500Copies");
    if (x === null || y === null) return t("dlg.spacingMustBeNumbers");
    return C.arrayGrid(ids, c, r, [x, y]);
  };
  return (
    <Modal
      title={t("dlg.rectangularArray")}
      onClose={onClose}
      onSubmit={() => submit(build())}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={onClose}>
            {t("dlg.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => submit(build())}>
            {t("dlg.create")}
          </button>
        </>
      }
    >
      <div className="field-grid">
        <Num label={t("dlg.columns")} value={cols} onChange={setCols} />
        <Num label={t("dlg.rows")} value={rows} onChange={setRows} />
        <Num label={t("dlg.pitchX")} unit="mm" value={dx} onChange={setDx} />
        <Num label={t("dlg.pitchY")} unit="mm" value={dy} onChange={setDy} />
      </div>
      {outside > 0 && (
        <p className="warn-text" role="status">
          {outside} of the {(nc ?? 0) * (nr ?? 0)} cells would be outside the {workspaceText(workspaceMm)}{" "}
          {isSquare(workspaceMm) ? "galvo field" : "workspace"}. Use a smaller pitch or
          negative pitch, or move the original first.
        </p>
      )}
      <p className="hint">{t("dlg.countsIncludeTheOriginalPitchIs")}</p>
    </Modal>
  );
}

export function CommandPalette({ commands, run, onClose }: { commands: CommandSpec[] | null; run: Run; onClose: () => void }) {
  const t = useT();
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
      setResult(t("dlg.parametersMustBeAJsonObject"));
      return;
    }
    setBusy(true);
    try {
      const r = await run({ id: picked.id, params: p as Record<string, unknown> });
      setResult(JSON.stringify(r, null, 2) ?? "ok");
    } catch (e) {
      // The command runner shows the engine's own text: the command id there is the one the user picked.
      setResult(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal title={t("dlg.commands")} onClose={onClose} wide>
      <div className="palette">
        <div className="palette-list">
          <input type="search" placeholder={t("dlg.filterCommands")} value={q} onChange={(e) => setQ(e.target.value)} />
          {commands === null && <p className="muted">{t("dlg.loading")}</p>}
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
            <p className="muted">{t("dlg.pickACommandThisRunsExactly")}</p>
          ) : (
            <>
              <h3>
                {picked.title} <span className={`badge risk-${picked.risk}`}>{picked.risk}</span>
              </h3>
              <p className="desc">{picked.description}</p>
              <details>
                <summary>{t("dlg.inputSchema")}</summary>
                <pre>{JSON.stringify(picked.input_schema, null, 2)}</pre>
              </details>
              <textarea spellCheck={false} value={params} onChange={(e) => setParams(e.target.value)} rows={8} />
              <div className="btn-row">
                <button className="btn btn-primary" disabled={busy} onClick={go}>
                  {t("dlg.run")}
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
