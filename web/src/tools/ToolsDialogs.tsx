// The Tools menu extras' windows: Dogbone corners (`path.dogbone`), Resize Slots in Selection (`path.resize_slots`,
// Apply keeps the window open as in LightBurn), Multi-File Trace (`image.trace_many` for images in the document,
// `image.trace_files` for image files, downloaded as one zip of SVGs or imported) and Manage User Tools (LightBurn's
// Web Tools > Manage User Tools, saved with `PUT /api/prefs {user_tools}`).

import { useState } from "react";
import { execute, type Invocation } from "../api/client.ts";
import { CheckField, Modal, NumberField } from "../components/ui.tsx";
import { DEFAULT_TRACE, traceProblem, type TraceOptions } from "../files/fileFormats.ts";
import { saveBlob } from "../files/filesApi.ts";
import { errText, fileBase64 } from "../libs/ui.tsx";
import { userMessage } from "../api/errorText.ts";
import { useT } from "../workspace/usePrefs.ts";
import {
  DEFAULT_SLOTS,
  dogboneInvocation,
  dogboneProblem,
  importTracedSvgs,
  parseSteps,
  parseTraceFiles,
  slotsInvocation,
  slotsProblem,
  traceFilesInvocation,
  traceManyInvocation,
  userToolProblem,
  type DogboneStyle,
  type SlotForm,
  type UserTool,
} from "./toolsApi.ts";
import "./tools.css";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
type Notify = (msg: string, kind?: "info" | "error") => void;

interface Common {
  run: Run;
  notify: Notify;
  onClose: () => void;
}

function useBusy() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const go = async <T,>(f: () => Promise<T>): Promise<T | null> => {
    setBusy(true);
    setError(null);
    try {
      return await f();
    } catch (e) {
      setError(errText(e));
      return null;
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, setError, go };
}

const field = <T extends object>(r: unknown, k: string): T | null => {
  const v = r && typeof r === "object" ? (r as Record<string, unknown>)[k] : null;
  return v && typeof v === "object" ? (v as T) : null;
};

export function DogboneDialog(p: Common & { ids: string[] }) {
  const t = useT();
  const [d, setD] = useState<number | null>(3.175);
  const [style, setStyle] = useState<DogboneStyle>("dogbone");
  const { busy, error, setError, go } = useBusy();
  const ok = async () => {
    const problem = dogboneProblem(d);
    if (problem) return setError(t(problem));
    const r = await go(() => p.run(dogboneInvocation(p.ids, d ?? 0, style)));
    if (r === null) return;
    const objects = field<Record<string, { corners?: unknown[]; skipped?: unknown[] }>>(r, "objects") ?? {};
    const done = Object.values(objects).reduce((n, o) => n + (o.corners?.length ?? 0), 0);
    const skipped = Object.values(objects).reduce((n, o) => n + (o.skipped?.length ?? 0), 0);
    p.notify(t("dog.done", { n: done }) + (skipped ? ` ${t("dog.skipped", { n: skipped })}` : ""));
    p.onClose();
  };
  return (
    <Modal
      help="drawing-and-editing#the-tools-menu"
      title={t("dog.title")}
      onClose={p.onClose}
      onSubmit={() => void ok()}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void ok()}>
            {t("files.ok")}
          </button>
        </>
      }
    >
      <NumberField label={t("dog.diameter")} unit="mm" value={d} min={0.001} max={100} digits={3} onCommit={setD} />
      <label className="field">
        <span className="field-label">{t("dog.style")}</span>
        <span className="field-input">
          <select value={style} onChange={(e) => setStyle(e.target.value as DogboneStyle)}>
            <option value="dogbone">{t("dog.dogbone")}</option>
            <option value="tbone_longer">{t("dog.tboneLonger")}</option>
            <option value="tbone_shorter">{t("dog.tboneShorter")}</option>
          </select>
        </span>
      </label>
      <p className="hint">{t("dog.hint")}</p>
    </Modal>
  );
}

export function ResizeSlotsDialog(p: Common & { ids: string[] }) {
  const t = useT();
  const [f, setF] = useState<SlotForm>(DEFAULT_SLOTS);
  const set = (patch: Partial<SlotForm>) => setF((x) => ({ ...x, ...patch }));
  const { busy, error, setError, go } = useBusy();
  const apply = async (close: boolean) => {
    const problem = slotsProblem(f);
    if (problem) return setError(t(problem));
    const r = await go(() => p.run(slotsInvocation(p.ids, f)));
    if (r === null) return;
    const o = r as { steps?: number; widths?: number };
    p.notify(t("slots.done", { steps: o.steps ?? 0, widths: o.widths ?? 0 }));
    // As in LightBurn, Apply keeps the window open (the old thickness is now the new one).
    set({ oldMm: f.newMm });
    if (close) p.onClose();
  };
  return (
    <Modal
      help="drawing-and-editing#the-tools-menu"
      title={t("slots.title")}
      onClose={p.onClose}
      onSubmit={() => void apply(true)}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn" disabled={busy} onClick={() => void apply(false)}>
            {t("slots.apply")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void apply(true)}>
            {t("files.ok")}
          </button>
        </>
      }
    >
      <NumberField label={t("slots.old")} unit="mm" value={f.oldMm} min={0.001} max={1000} digits={3} onCommit={(oldMm) => set({ oldMm })} />
      <NumberField label={t("slots.new")} unit="mm" value={f.newMm} min={0.001} max={1000} digits={3} onCommit={(newMm) => set({ newMm })} />
      <NumberField label={t("slots.tolerance")} unit="mm" value={f.toleranceMm} min={0} max={1000} digits={3} onCommit={(toleranceMm) => set({ toleranceMm })} />
      <CheckField label={t("slots.depth")} checked={f.depth} onChange={(depth) => set({ depth })} />
      <CheckField label={t("slots.width")} checked={f.width} onChange={(width) => set({ width })} />
      <CheckField label={t("slots.tabHeight")} checked={f.tabHeight} onChange={(tabHeight) => set({ tabHeight })} />
      <p className="hint">{t("slots.hint")}</p>
    </Modal>
  );
}

const IMAGE_ACCEPT = "image/*,.tif,.tiff,.tga,.bmp,.webp";

export function MultiTraceDialog(p: Common & { imageIds: string[]; activeLayer: string | null; onCreated: (ids: string[]) => void }) {
  const t = useT();
  const [source, setSource] = useState<"selected" | "files">(p.imageIds.length ? "selected" : "files");
  const [files, setFiles] = useState<File[]>([]);
  const [o, setO] = useState<TraceOptions>(DEFAULT_TRACE);
  const [dpi, setDpi] = useState<number | null>(254);
  const [toDoc, setToDoc] = useState(false);
  const [deleteImages, setDeleteImages] = useState(false);
  const { busy, error, setError, go } = useBusy();
  const set = <K extends keyof TraceOptions>(k: K, v: TraceOptions[K]) => setO((x) => ({ ...x, [k]: v }));
  const created = (r: unknown): string[] => {
    const c = r && typeof r === "object" ? (r as { created?: unknown }).created : null;
    return Array.isArray(c) ? c.filter((x): x is string => typeof x === "string") : [];
  };
  const ok = async () => {
    const problem = traceProblem(o);
    if (problem) return setError(problem);
    if (source === "selected") {
      if (!p.imageIds.length) return setError(t("mtrace.noImages"));
      const r = await go(() => p.run(traceManyInvocation(p.imageIds, o, deleteImages, null)));
      if (r === null) return;
      const empty = (r as { empty?: unknown[] }).empty?.length ?? 0;
      p.notify(t("mtrace.doneDoc", { n: created(r).length }) + (empty ? ` ${t("mtrace.empty", { n: empty })}` : ""));
      p.onCreated(created(r));
      return p.onClose();
    }
    if (!files.length) return setError(t("mtrace.noImages"));
    const res = await go(async () => {
      const payload = await Promise.all(files.map(async (f) => ({ name: f.name, data_base64: await fileBase64(f) })));
      const r = await execute(traceFilesInvocation(payload, o, dpi, !toDoc));
      return parseTraceFiles(r.result);
    });
    if (!res) return;
    const traced = res.files.filter((f) => f.svg);
    for (const f of res.files.filter((x) => x.error)) p.notify(t("mtrace.failed", { name: f.name, error: userMessage(f.error ?? "") }), "error");
    if (!traced.length) return setError(t("mtrace.doneFiles", { n: 0, total: res.files.length }));
    if (toDoc) {
      const r = await go(() => p.run(importTracedSvgs(traced, p.activeLayer)));
      if (r === null) return;
      // A batch answers {results: [result, ...]}.
      const results = r && typeof r === "object" && Array.isArray((r as { results?: unknown }).results) ? (r as { results: unknown[] }).results : [r];
      p.onCreated(results.flatMap(created));
    } else if (res.zip) {
      const bytes = Uint8Array.from(atob(res.zip), (c) => c.charCodeAt(0));
      saveBlob(new Blob([bytes], { type: "application/zip" }), res.zipName);
    }
    p.notify(t("mtrace.doneFiles", { n: traced.length, total: res.files.length }));
    p.onClose();
  };
  const names = files.map((f) => f.name);
  return (
    <Modal
      help="images#multi-file-trace"
      title={t("mtrace.title")}
      onClose={p.onClose}
      wide
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void ok()}>
            {t("mtrace.run")}
          </button>
        </>
      }
    >
      <div className="tools-cols">
        <div>
          <fieldset className="tools-set">
            <legend>{t("mtrace.source")}</legend>
            <label className="tools-radio">
              <input type="radio" checked={source === "selected"} disabled={!p.imageIds.length} onChange={() => setSource("selected")} />
              {t("mtrace.selected", { n: p.imageIds.length })}
            </label>
            <label className="tools-radio">
              <input type="radio" checked={source === "files"} onChange={() => setSource("files")} />
              {t("mtrace.files")}
            </label>
            {source === "files" ? (
              <>
                <label className="btn tools-pick">
                  {t("mtrace.pick")}
                  <input
                    type="file"
                    accept={IMAGE_ACCEPT}
                    multiple
                    hidden
                    onChange={(e) => {
                      setFiles([...(e.target.files ?? [])]);
                      e.target.value = "";
                    }}
                  />
                </label>
                {names.length > 0 && <p className="hint">{t("mtrace.picked", { n: names.length, names: names.slice(0, 5).join(", ") + (names.length > 5 ? ", …" : "") })}</p>}
                <NumberField label={t("mtrace.dpi")} unit="dpi" value={dpi} min={1} max={100000} digits={0} onCommit={setDpi} />
                <fieldset className="tools-set">
                  <legend>{t("mtrace.output")}</legend>
                  <label className="tools-radio">
                    <input type="radio" checked={!toDoc} onChange={() => setToDoc(false)} />
                    {t("mtrace.download")}
                  </label>
                  <label className="tools-radio">
                    <input type="radio" checked={toDoc} onChange={() => setToDoc(true)} />
                    {t("mtrace.addToDoc")}
                  </label>
                </fieldset>
              </>
            ) : (
              <CheckField label={t("mtrace.deleteImages")} checked={deleteImages} onChange={setDeleteImages} />
            )}
          </fieldset>
          <p className="hint">{t("mtrace.hint")}</p>
        </div>
        <div>
          <NumberField label={t("files.cutoff")} value={o.cutoff} min={0} max={255} digits={0} onCommit={(v) => v !== null && set("cutoff", Math.round(v))} />
          <NumberField label={t("files.threshold")} value={o.threshold} min={0} max={255} digits={0} onCommit={(v) => v !== null && set("threshold", Math.round(v))} />
          <NumberField label={t("files.ignoreLess")} value={o.ignoreLessThan} min={0} max={1000} digits={0} onCommit={(v) => v !== null && set("ignoreLessThan", Math.round(v))} />
          <NumberField label={t("files.smoothness")} value={o.smoothness} min={0} max={1.33} digits={2} onCommit={(v) => v !== null && set("smoothness", v)} />
          <NumberField label={t("files.optimize")} value={o.optimize} min={0} max={5} digits={2} onCommit={(v) => v !== null && set("optimize", v)} />
          <CheckField label={t("files.traceTransparency")} checked={o.transparency} onChange={(v) => set("transparency", v)} />
          <CheckField label={t("files.centerLine")} checked={o.centerline} onChange={(v) => set("centerline", v)} />
          <CheckField label={t("files.sketchTrace")} checked={o.sketch} onChange={(v) => set("sketch", v)} />
        </div>
      </div>
    </Modal>
  );
}

/** One tool being edited: the form's text fields (steps as JSON text). */
interface Draft {
  name: string;
  category: string;
  kind: "url" | "commands";
  url: string;
  steps: string;
}

const EMPTY_DRAFT: Draft = { name: "", category: "", kind: "url", url: "https://", steps: '[\n  {"id": "shape.rect", "params": {"width_mm": 50, "height_mm": 20}}\n]' };

function draftOf(tool: UserTool): Draft {
  return tool.kind === "url"
    ? { ...EMPTY_DRAFT, name: tool.name, category: tool.category, kind: "url", url: tool.url }
    : { ...EMPTY_DRAFT, name: tool.name, category: tool.category, kind: "commands", steps: JSON.stringify(tool.steps, null, 2) };
}

function toolOf(d: Draft): { tool: UserTool } | { error: string } {
  if (d.kind === "url") return { tool: { name: d.name, category: d.category, kind: "url", url: d.url } };
  const s = parseSteps(d.steps);
  return "error" in s ? s : { tool: { name: d.name, category: d.category, kind: "commands", steps: s.steps } };
}

export function UserToolsDialog(p: { tools: UserTool[]; save: (tools: UserTool[]) => Promise<void>; notify: Notify; onClose: () => void }) {
  const t = useT();
  const [list, setList] = useState<UserTool[]>(p.tools);
  const [editing, setEditing] = useState<{ index: number | null; draft: Draft } | null>(null);
  const { busy, error, setError, go } = useBusy();
  const commit = () => {
    if (!editing) return;
    const r = toolOf(editing.draft);
    if ("error" in r) return setError(r.error);
    const problem = userToolProblem(r.tool);
    if (problem) return setError(problem);
    setError(null);
    setList((l) => (editing.index === null ? [...l, r.tool] : l.map((x, i) => (i === editing.index ? r.tool : x))));
    setEditing(null);
  };
  const save = async () => {
    const ok = await go(() => p.save(list));
    if (ok === null) return;
    p.notify(t("ut.saved"));
    p.onClose();
  };
  const d = editing?.draft;
  const setD = (patch: Partial<Draft>) => setEditing((e) => (e ? { ...e, draft: { ...e.draft, ...patch } } : e));
  return (
    <Modal
      help="drawing-and-editing#the-tools-menu"
      title={t("ut.title")}
      onClose={p.onClose}
      wide
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || !!editing} onClick={() => void save()}>
            {t("ut.save")}
          </button>
        </>
      }
    >
      <p className="hint">{t("ut.notice")}</p>
      <ul className="tools-list" aria-label={t("ut.title")}>
        {list.length === 0 && <li className="hint">{t("ut.none")}</li>}
        {list.map((tool, i) => (
          <li key={i}>
            <span className="tools-name">
              {tool.category ? `${tool.category} › ` : ""}
              {tool.name}
            </span>
            <span className="tools-what">{tool.kind === "url" ? tool.url : `${tool.steps.length} × ${tool.steps.map((s) => s.id).join(", ")}`}</span>
            <span className="tools-actions">
              <button className="btn small" disabled={i === 0} title={t("ut.up")} onClick={() => setList((l) => l.map((x, k) => (k === i - 1 ? l[i] : k === i ? l[i - 1] : x)))}>
                ↑
              </button>
              <button className="btn small" onClick={() => setEditing({ index: i, draft: draftOf(tool) })}>
                {t("ut.edit")}
              </button>
              <button className="btn small" onClick={() => setList((l) => l.filter((_, k) => k !== i))}>
                {t("ut.remove")}
              </button>
            </span>
          </li>
        ))}
      </ul>
      {!editing && (
        <button className="btn" onClick={() => setEditing({ index: null, draft: EMPTY_DRAFT })}>
          {t("ut.add")}
        </button>
      )}
      {d && (
        <fieldset className="tools-set">
          <legend>{editing?.index === null ? t("ut.add") : t("ut.edit")}</legend>
          <label className="field">
            <span className="field-label">{t("ut.name")}</span>
            <span className="field-input">
              <input type="text" value={d.name} maxLength={40} onChange={(e) => setD({ name: e.target.value })} />
            </span>
          </label>
          <label className="field">
            <span className="field-label">{t("ut.category")}</span>
            <span className="field-input">
              <input type="text" value={d.category} maxLength={40} onChange={(e) => setD({ category: e.target.value })} />
            </span>
          </label>
          <label className="field">
            <span className="field-label">{t("ut.kind")}</span>
            <span className="field-input">
              <select value={d.kind} onChange={(e) => setD({ kind: e.target.value === "commands" ? "commands" : "url" })}>
                <option value="url">{t("ut.kindUrl")}</option>
                <option value="commands">{t("ut.kindCommands")}</option>
              </select>
            </span>
          </label>
          {d.kind === "url" ? (
            <label className="field">
              <span className="field-label">{t("ut.url")}</span>
              <span className="field-input">
                <input type="url" value={d.url} onChange={(e) => setD({ url: e.target.value })} />
              </span>
            </label>
          ) : (
            <>
              <label className="field tools-steps">
                <span className="field-label">{t("ut.steps")}</span>
                <textarea rows={6} spellCheck={false} value={d.steps} onChange={(e) => setD({ steps: e.target.value })} />
              </label>
              <p className="hint">{t("ut.stepsHint")}</p>
            </>
          )}
          <div className="btn-row compact">
            <button className="btn" onClick={() => setEditing(null)}>
              {t("common.cancel")}
            </button>
            <button className="btn btn-primary" onClick={commit}>
              {t("files.ok")}
            </button>
          </div>
        </fieldset>
      )}
    </Modal>
  );
}
