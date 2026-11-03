// New from example (File menu, the empty-canvas card, the phone's More > File): the example projects that ship with
// OpenEtch (`example.list`, crates/commands/src/builtin/examples.rs). Opening one is `doc.new {example}`: a new,
// unsaved document laid out for the active device, with its walkthrough as the project notes (shown on open).

import { useEffect, useState } from "react";
import { execute } from "../api/client.ts";
import { errorText } from "../api/errorText.ts";
import { Dialog } from "../components/Dialog.tsx";
import { Icon } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { TPL, parseExamples, type ExampleInfo } from "./templateModel.ts";

export function ExamplesDialog(p: { onOpen: (id: string, title: string) => void; onClose: () => void }) {
  const t = useT();
  const [list, setList] = useState<ExampleInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    execute({ id: TPL.examples, params: {} })
      .then((r) => alive && setList(parseExamples(r.result)))
      .catch((e) => alive && (setError(errorText(e)), setList([])));
    return () => {
      alive = false;
    };
  }, []);
  return (
    <Dialog
      title={t("tpl.examplesTitle")}
      size="lg"
      onClose={p.onClose}
      footer={
        <button className="btn" onClick={p.onClose}>
          {t("common.cancel")}
        </button>
      }
    >
      <p className="hint">{t("tpl.examplesHint")}</p>
      {error && <p className="form-error">{error}</p>}
      {list === null && <p className="muted">{t("common.loading")}</p>}
      <ul className="tpl-examples">
        {list?.map((e) => (
          <li key={e.id} className="tpl-example">
            <div className="tpl-example-head">
              <h4>{e.title}</h4>
              <button className="btn btn-primary" onClick={() => p.onOpen(e.id, e.title)}>
                <Icon name="book" size={16} /> {t("tpl.openExample")}
              </button>
            </div>
            <p>{e.summary}</p>
            {e.tags.length > 0 && (
              <p className="tpl-tags">
                {e.tags.map((g) => (
                  <span key={g} className="files-badge">
                    {g}
                  </span>
                ))}
              </p>
            )}
            {e.steps.length > 0 && (
              <details>
                <summary>{t("tpl.steps", { n: e.steps.length })}</summary>
                <ol className="tpl-steps">
                  {e.steps.map((s, i) => (
                    <li key={i}>{s}</li>
                  ))}
                </ol>
              </details>
            )}
          </li>
        ))}
      </ul>
    </Dialog>
  );
}
