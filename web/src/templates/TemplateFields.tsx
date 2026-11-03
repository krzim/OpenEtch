// Template fields (the designer's side; Cuttle's "Extract as project parameter"): mark texts, a date, a font, a
// layer's finish and the first serial number as the template's fill-in fields, and lock everything else. Every change
// is one engine command (template.set_field / remove_field / set_lock), so it is undoable and an agent sees the same
// fields (template.fields). The fill-in form (FillIn.tsx) is what the person using the template sees.

import { useEffect, useMemo, useState } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument } from "../api/types.ts";
import { Dialog } from "../components/Dialog.tsx";
import { Icon } from "../components/ui.tsx";
import { FontPicker } from "../text/FontPicker.tsx";
import { isText, shownText, type FontInfo } from "../text/textModel.ts";
import { useT } from "../workspace/usePrefs.ts";
import {
  FIELD_KINDS,
  choiceFromLayer,
  newField,
  removeFieldInvocation,
  setFieldInvocation,
  setLockInvocation,
  specOf,
  type FieldKind,
  type TemplateField,
} from "./templateModel.ts";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

const KIND_KEY = {
  text: "tpl.kind.text",
  date: "tpl.kind.date",
  font: "tpl.kind.font",
  preset: "tpl.kind.preset",
  serial: "tpl.kind.serial",
} as const satisfies Record<FieldKind, string>;

export function TemplateFieldsDialog(p: {
  doc: GDocument;
  selection: string[];
  fonts: FontInfo[] | null;
  run: Run;
  report: (e: unknown) => void;
  onClose: () => void;
  onFillIn: () => void;
  /** Start with a new field of this kind (the context menu's "Make a template field"). */
  start?: FieldKind;
}) {
  const t = useT();
  const spec = specOf(p.doc);
  const [edit, setEdit] = useState<TemplateField | null>(null);
  const [isNew, setIsNew] = useState(false);
  const texts = p.doc.objects.filter(isText);
  const selTexts = texts.filter((o) => p.selection.includes(o.id)).map((o) => o.id);
  const taken = spec.fields.map((f) => f.id);

  const add = (kind: FieldKind) => {
    const targets = kind === "preset" ? (p.doc.layers[0] ? [p.doc.layers[0].id] : []) : selTexts.length ? selTexts : texts[0] ? [texts[0].id] : [];
    setEdit(newField(kind, t(KIND_KEY[kind]), targets, taken));
    setIsNew(true);
  };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => void (p.start && add(p.start)), []);

  const save = (f: TemplateField) =>
    p
      .run(setFieldInvocation(f))
      .then(() => setEdit(null))
      .catch(p.report);
  const move = (i: number, d: number) => {
    const f = spec.fields[i];
    const j = i + d;
    if (!f || j < 0 || j >= spec.fields.length) return;
    void p.run(setFieldInvocation(f, j)).catch(p.report);
  };
  const nameOf = (id: string, kind: FieldKind) => {
    if (kind === "preset") return p.doc.layers.find((l) => l.id === id)?.name ?? id;
    return p.doc.objects.find((o) => o.id === id)?.name ?? id;
  };

  return (
    <Dialog
      title={t("tpl.fieldsTitle")}
      size="lg"
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" disabled={!spec.fields.length} onClick={p.onFillIn}>
            <Icon name="fillIn" size={16} /> {t("tpl.fillIn")}
          </button>
          <button className="btn btn-primary" onClick={p.onClose}>
            {t("common.close")}
          </button>
        </>
      }
    >
      <p className="hint">{t("tpl.fieldsHint")}</p>
      <label className="tpl-lock">
        <input type="checkbox" checked={spec.lock_others} disabled={!spec.fields.length && !spec.lock_others} onChange={(e) => void p.run(setLockInvocation(e.target.checked)).catch(p.report)} />
        <span>{t("tpl.lockOthers")}</span>
      </label>
      {spec.fields.length === 0 && <p className="muted">{t("tpl.noFieldsYet")}</p>}
      {spec.fields.length > 0 && (
        <ul className="tpl-list">
          {spec.fields.map((f, i) => (
            <li key={f.id} className="tpl-list-row">
              <span className="tpl-list-main">
                <b>{f.label}</b> <span className="files-badge">{t(KIND_KEY[f.kind])}</span>
                <span className="tpl-list-targets">{f.kind === "serial" ? t("tpl.serialTarget") : f.targets.map((id) => nameOf(id, f.kind)).join(", ")}</span>
              </span>
              <span className="tpl-list-btns">
                <button className="icon-btn small" aria-label={t("tpl.moveUp")} title={t("tpl.moveUp")} disabled={i === 0} onClick={() => move(i, -1)}>
                  <Icon name="chevronUp" size={16} />
                </button>
                <button className="icon-btn small" aria-label={t("tpl.moveDown")} title={t("tpl.moveDown")} disabled={i === spec.fields.length - 1} onClick={() => move(i, 1)}>
                  <Icon name="chevronDown" size={16} />
                </button>
                <button
                  className="icon-btn small"
                  aria-label={t("tpl.editField", { name: f.label })}
                  title={t("tpl.editField", { name: f.label })}
                  onClick={() => {
                    setEdit(f);
                    setIsNew(false);
                  }}
                >
                  <Icon name="pencil" size={16} />
                </button>
                <button className="icon-btn small" aria-label={t("tpl.removeField", { name: f.label })} title={t("tpl.removeField", { name: f.label })} onClick={() => void p.run(removeFieldInvocation(f.id)).catch(p.report)}>
                  <Icon name="delete" size={16} />
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
      {!edit && (
        <div className="tpl-add" role="group" aria-label={t("tpl.addField")}>
          <span className="field-label">{t("tpl.addField")}</span>
          {FIELD_KINDS.map((k) => (
            <button key={k} className="btn" disabled={k !== "serial" && k !== "preset" && !texts.length} onClick={() => add(k)}>
              {t(KIND_KEY[k])}
            </button>
          ))}
        </div>
      )}
      {edit && <FieldEditor key={edit.id + String(isNew)} doc={p.doc} fonts={p.fonts} field={edit} isNew={isNew} taken={taken} onSave={save} onCancel={() => setEdit(null)} />}
    </Dialog>
  );
}

function FieldEditor(p: { doc: GDocument; fonts: FontInfo[] | null; field: TemplateField; isNew: boolean; taken: string[]; onSave: (f: TemplateField) => void; onCancel: () => void }) {
  const t = useT();
  const [f, setF] = useState<TemplateField>(p.field);
  const [choiceLabel, setChoiceLabel] = useState("");
  const [choiceLayer, setChoiceLayer] = useState(p.field.targets[0] ?? p.doc.layers[0]?.id ?? "");
  const up = (c: Partial<TemplateField>) => setF((x) => ({ ...x, ...c }));
  const texts = useMemo(() => p.doc.objects.filter(isText).filter((o) => f.kind === "font" || !o.geometry.variable), [p.doc, f.kind]);
  const toggleTarget = (id: string, on: boolean) => up({ targets: on ? [...f.targets, id] : f.targets.filter((x) => x !== id) });
  const ok = f.label.trim() && (f.kind === "serial" || f.targets.length > 0) && (f.kind !== "preset" || f.choices.length > 0);
  return (
    <section className="tpl-editor" aria-label={t(p.isNew ? "tpl.newField" : "tpl.editField", { name: f.label })}>
      <h4>{t(p.isNew ? "tpl.newField" : "tpl.editField", { name: f.label })}</h4>
      <label className="field">
        <span className="field-label">{t("tpl.label")}</span>
        <span className="field-input">
          <input type="text" value={f.label} maxLength={200} onChange={(e) => up({ label: e.target.value })} />
        </span>
      </label>
      {f.kind === "preset" ? (
        <label className="field">
          <span className="field-label">{t("tpl.layers")}</span>
          <span className="tpl-targets">
            {p.doc.layers.map((l) => (
              <label key={l.id} className="tpl-target">
                <input type="checkbox" checked={f.targets.includes(l.id)} onChange={(e) => toggleTarget(l.id, e.target.checked)} />
                <span className="tpl-swatch" style={{ background: l.color }} aria-hidden="true" />
                {l.name}
              </label>
            ))}
          </span>
        </label>
      ) : f.kind !== "serial" ? (
        <div className="field">
          <span className="field-label">{t("tpl.texts")}</span>
          <span className="tpl-targets">
            {texts.length === 0 && <span className="muted">{t("tpl.noTexts")}</span>}
            {texts.map((o) => (
              <label key={o.id} className="tpl-target">
                <input type="checkbox" checked={f.targets.includes(o.id)} onChange={(e) => toggleTarget(o.id, e.target.checked)} />
                {o.name} <span className="muted">“{shownText(o.geometry).slice(0, 40)}”</span>
              </label>
            ))}
          </span>
        </div>
      ) : (
        <p className="hint">{t("tpl.serialHint")}</p>
      )}
      {f.kind === "text" && (
        <div className="tpl-opts">
          <label className="tpl-target">
            <input type="checkbox" checked={f.required} onChange={(e) => up({ required: e.target.checked })} /> {t("tpl.required.opt")}
          </label>
          <label className="tpl-target">
            <input type="checkbox" checked={f.multiline} onChange={(e) => up({ multiline: e.target.checked })} /> {t("tpl.multiline")}
          </label>
          <label className="tpl-target">
            {t("tpl.maxChars")}{" "}
            <input type="number" className="tpl-small" min={0} max={1000} value={f.max_chars} onChange={(e) => up({ max_chars: Math.max(0, Math.min(1000, Math.round(Number(e.target.value) || 0))) })} />
          </label>
        </div>
      )}
      {f.kind === "date" && (
        <label className="field">
          <span className="field-label">{t("tpl.dateFormat")}</span>
          <span className="field-input">
            <input type="text" value={f.format ?? ""} placeholder="yyyy-MM-dd" spellCheck={false} onChange={(e) => up({ format: e.target.value })} />
          </span>
        </label>
      )}
      {f.kind === "font" && (
        <div className="field">
          <span className="field-label">{t("tpl.fontChoices")}</span>
          <span className="tpl-targets">
            {f.choices.map((c) => (
              <span key={c.label} className="tpl-chip">
                {c.label}
                <button className="icon-btn small" aria-label={t("tpl.removeChoice", { name: c.label })} onClick={() => up({ choices: f.choices.filter((x) => x !== c) })}>
                  <Icon name="close" size={14} />
                </button>
              </span>
            ))}
            <FontPicker
              value={null}
              fonts={p.fonts}
              placeholder={t("tpl.addFont")}
              onPick={(font) => !f.choices.some((c) => c.label.toLowerCase() === font.family.toLowerCase()) && up({ choices: [...f.choices, { label: font.family, font }] })}
            />
            <span className="hint">{t("tpl.fontChoicesHint")}</span>
          </span>
        </div>
      )}
      {f.kind === "preset" && (
        <div className="field">
          <span className="field-label">{t("tpl.finishes")}</span>
          <span className="tpl-targets">
            {f.choices.map((c) => (
              <span key={c.label} className="tpl-chip">
                <span className="tpl-swatch" style={{ background: c.swatch ?? "transparent" }} aria-hidden="true" />
                {c.label}
                <button className="icon-btn small" aria-label={t("tpl.removeChoice", { name: c.label })} onClick={() => up({ choices: f.choices.filter((x) => x !== c) })}>
                  <Icon name="close" size={14} />
                </button>
              </span>
            ))}
            <span className="tpl-row">
              <input type="text" className="tpl-input" placeholder={t("tpl.finishName")} value={choiceLabel} onChange={(e) => setChoiceLabel(e.target.value)} />
              <select value={choiceLayer} onChange={(e) => setChoiceLayer(e.target.value)} aria-label={t("tpl.finishFrom")}>
                {p.doc.layers.map((l) => (
                  <option key={l.id} value={l.id}>
                    {l.name}
                  </option>
                ))}
              </select>
              <button
                className="btn"
                disabled={!choiceLabel.trim() || f.choices.some((c) => c.label.toLowerCase() === choiceLabel.trim().toLowerCase())}
                onClick={() => {
                  const l = p.doc.layers.find((x) => x.id === choiceLayer);
                  if (!l) return;
                  up({ choices: [...f.choices, choiceFromLayer(choiceLabel, l)] });
                  setChoiceLabel("");
                }}
              >
                {t("tpl.addFinish")}
              </button>
            </span>
            <span className="hint">{t("tpl.finishHint")}</span>
          </span>
        </div>
      )}
      <label className="field">
        <span className="field-label">{t("tpl.hint")}</span>
        <span className="field-input">
          <input type="text" value={f.hint ?? ""} maxLength={200} onChange={(e) => up({ hint: e.target.value })} />
        </span>
      </label>
      <div className="tx-row-btns">
        <button className="btn" onClick={p.onCancel}>
          {t("common.cancel")}
        </button>
        <button className="btn btn-primary" disabled={!ok} onClick={() => p.onSave({ ...f, label: f.label.trim() })}>
          {t(p.isNew ? "tpl.addThisField" : "tpl.saveField")}
        </button>
      </div>
    </section>
  );
}
