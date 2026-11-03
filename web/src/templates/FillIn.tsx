// The fill-in form of a template (Cuttle's simplified view, Cricut's ready-to-make projects): one short field per
// template parameter, sized for a finger on the phone and the iPad (the Dialog primitive is a full-screen sheet there,
// with the footer above the keyboard). Apply sends one `template.fill` (one undo step) with the values that changed;
// a name that now runs off its shape is reported here before anyone frames it. It never starts anything.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { execute, type Invocation } from "../api/client.ts";
import { errorText } from "../api/errorText.ts";
import { Dialog } from "../components/Dialog.tsx";
import { Icon } from "../components/ui.tsx";
import { FontPicker } from "../text/FontPicker.tsx";
import { TXT, parseFontList, type FontInfo } from "../text/textModel.ts";
import { useT } from "../workspace/usePrefs.ts";
import { TPL, draftFont, draftOf, draftProblems, fillInvocation, fillReport, fontDraft, parseSpec, today, type Draft, type TemplateField, type TemplateSpec } from "./templateModel.ts";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export function FillInDialog(p: { run: Run; onClose: () => void; notify: (m: string, k?: "info" | "error") => void; onEditFields?: () => void }) {
  const t = useT();
  const [spec, setSpec] = useState<TemplateSpec | null>(null);
  const [before, setBefore] = useState<Draft>({});
  const [draft, setDraft] = useState<Draft>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<{ overflow: { id: string; message: string }[]; warnings: string[] } | null>(null);
  const [fonts, setFonts] = useState<FontInfo[] | null>(null);
  const [tried, setTried] = useState(false);

  useEffect(() => {
    let alive = true;
    execute({ id: TPL.fields, params: {} })
      .then((r) => {
        if (!alive) return;
        const s = parseSpec(r.result);
        const d = draftOf(s);
        setSpec(s);
        setBefore(d);
        setDraft(d);
      })
      .catch((e) => alive && setError(errorText(e)));
    return () => {
      alive = false;
    };
  }, []);
  const needsFonts = !!spec?.fields.some((f) => f.kind === "font" && f.choices.length === 0);
  useEffect(() => {
    if (!needsFonts) return;
    execute({ id: TXT.fontList, params: {} })
      .then((r) => setFonts(parseFontList(r.result)))
      .catch(() => setFonts([]));
  }, [needsFonts]);

  const problems = useMemo(() => (spec ? draftProblems(spec, draft) : {}), [spec, draft]);
  const changed = !!spec && fillInvocation(spec, draft, before) !== null;
  const set = (id: string, v: string) => {
    setDraft((d) => ({ ...d, [id]: v }));
    setReport(null);
  };

  const apply = () => {
    if (!spec) return;
    setTried(true);
    if (Object.keys(problems).length) return;
    const inv = fillInvocation(spec, draft, before);
    if (!inv) return p.onClose();
    setBusy(true);
    setError(null);
    p.run(inv)
      .then((r) => {
        const rep = fillReport(r);
        setBefore(draft);
        if (rep.overflow.length || rep.warnings.length) setReport(rep);
        else {
          p.notify(t("tpl.filled"));
          p.onClose();
        }
      })
      .catch((e) => setError(errorText(e)))
      .finally(() => setBusy(false));
  };

  const fieldsOk = spec && spec.fields.length > 0;
  return (
    <Dialog
      title={t("tpl.fillTitle")}
      size="md"
      onClose={p.onClose}
      onSubmit={apply}
      dirty={changed && !report}
      footer={
        <>
          {p.onEditFields && (
            <button className="btn tpl-edit-fields" onClick={p.onEditFields}>
              {t("tpl.editFields")}
            </button>
          )}
          <button className="btn" onClick={p.onClose}>
            {report ? t("common.close") : t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!fieldsOk || busy || (tried && Object.keys(problems).length > 0)} onClick={apply}>
            {t("tpl.apply")}
          </button>
        </>
      }
    >
      {error && <p className="form-error">{error}</p>}
      {!spec && !error && <p className="muted">{t("common.loading")}</p>}
      {spec && spec.fields.length === 0 && <p className="muted">{t("tpl.noFields")}</p>}
      {spec && spec.fields.length > 0 && (
        <div className="tpl-form">
          {spec.fields.map((f) => (
            <FieldInput key={f.id} f={f} value={draft[f.id] ?? ""} problem={tried ? problems[f.id] : undefined} fonts={fonts} onChange={(v) => set(f.id, v)} />
          ))}
          {spec.lock_others && (
            <p className="hint tpl-locked">
              <Icon name="lock" size={16} /> {t("tpl.lockedHint")}
            </p>
          )}
        </div>
      )}
      {report && (
        <div className="tpl-report" role="status">
          {report.overflow.map((o, i) => (
            <p key={`o${i}`} className="tpl-warn">
              <Icon name="warning" size={16} /> {t("tpl.overflow", { message: o.message })}
            </p>
          ))}
          {report.warnings.map((w, i) => (
            <p key={`w${i}`} className="tpl-warn">
              <Icon name="warning" size={16} /> {w}
            </p>
          ))}
          <p className="hint">{t("tpl.reportHint")}</p>
        </div>
      )}
    </Dialog>
  );
}

function FieldInput(p: { f: TemplateField; value: string; problem?: { key: string; n?: number }; fonts: FontInfo[] | null; onChange: (v: string) => void }) {
  const t = useT();
  const { f } = p;
  const id = `tpl-${f.id}`;
  const err = p.problem ? t(p.problem.key as "tpl.required", { n: p.problem.n ?? 0 }) : null;
  const gone = (f.problems?.length ?? 0) > 0;
  let input: ReactNode;
  switch (f.kind) {
    case "text":
      input = f.multiline ? (
        <textarea id={id} className="tpl-input" rows={3} spellCheck value={p.value} onChange={(e) => p.onChange(e.target.value)} />
      ) : (
        <input
          id={id}
          className="tpl-input"
          type="text"
          spellCheck
          autoComplete="off"
          enterKeyHint="done"
          value={p.value}
          maxLength={f.max_chars > 0 ? f.max_chars : undefined}
          onChange={(e) => p.onChange(e.target.value)}
        />
      );
      break;
    case "date":
      input = (
        <span className="tpl-row">
          <input id={id} className="tpl-input" type="date" value={p.value} onChange={(e) => p.onChange(e.target.value)} />
          <button type="button" className="btn" onClick={() => p.onChange(today())}>
            {t("tpl.today")}
          </button>
        </span>
      );
      break;
    case "serial":
      input = (
        <span className="tpl-row">
          <button type="button" className="btn tpl-step" aria-label={t("tpl.less")} onClick={() => p.onChange(String((Number.parseInt(p.value, 10) || 0) - 1))}>
            −
          </button>
          <input id={id} className="tpl-input tpl-num" type="text" inputMode="numeric" value={p.value} onChange={(e) => p.onChange(e.target.value)} />
          <button type="button" className="btn tpl-step" aria-label={t("tpl.more")} onClick={() => p.onChange(String((Number.parseInt(p.value, 10) || 0) + 1))}>
            +
          </button>
        </span>
      );
      break;
    case "font":
      input = f.choices.length ? (
        <span className="tpl-choices" role="radiogroup" aria-labelledby={`${id}-l`}>
          {f.choices.map((c) => (
            <button key={c.label} type="button" role="radio" aria-checked={p.value === c.label} className={`tpl-choice${p.value === c.label ? " on" : ""}`} onClick={() => p.onChange(c.label)}>
              {c.label}
            </button>
          ))}
        </span>
      ) : (
        <FontPicker value={draftFont(p.value)} fonts={p.fonts} onPick={(font) => p.onChange(fontDraft(font))} />
      );
      break;
    case "preset":
      input = (
        <span className="tpl-choices" role="radiogroup" aria-labelledby={`${id}-l`}>
          {f.choices.map((c) => (
            <button key={c.label} type="button" role="radio" aria-checked={p.value === c.label} className={`tpl-choice${p.value === c.label ? " on" : ""}`} onClick={() => p.onChange(c.label)}>
              <span className="tpl-swatch" style={{ background: c.swatch ?? "transparent" }} aria-hidden="true" />
              {c.label}
            </button>
          ))}
        </span>
      );
      break;
  }
  const count = f.kind === "text" && f.max_chars > 0 ? `${[...p.value].length} / ${f.max_chars}` : null;
  return (
    <div className={`tpl-field${err ? " bad" : ""}`}>
      <label id={`${id}-l`} className="tpl-label" htmlFor={f.kind === "preset" || (f.kind === "font" && f.choices.length) ? undefined : id}>
        {f.label}
        {count && <span className="tpl-count">{count}</span>}
      </label>
      {input}
      {f.hint && <p className="hint">{f.hint}</p>}
      {gone && <p className="tpl-warn">{f.problems!.join("; ")}</p>}
      {err && <p className="form-error">{err}</p>}
    </div>
  );
}
