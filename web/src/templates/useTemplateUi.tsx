// Templates with fill-in fields, the example projects and the contact sheet, as one hook for App.tsx:
// - File menu: New from example… (under New), Project ▸ Fill in template… and Template fields…, Laser ▸ Check every
//   row… (command search finds them through the File menu).
// - The canvas HUD: a "Fill in" button while the document has fields (the phone's and the iPad's way in).
// - The canvas context menu: "Make a template field" on a selected text.
// - The dialogs: FillIn.tsx, TemplateFields.tsx, Examples.tsx and the contact sheet (text/ContactSheet.tsx).
// Opening a template that has fields shows its fill-in form (files/useFilesUi.tsx opens it, `fillInOnOpen`).

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { execute } from "../api/client.ts";
import type { Engine } from "../api/useEngine.ts";
import type { MenuItem } from "../components/ContextMenu.tsx";
import type { FileActions } from "../components/FileMenu.tsx";
import { FileMenuPart } from "../components/FileMenuPart.tsx";
import { Icon } from "../components/ui.tsx";
import { HudItem } from "../hud/CanvasHud.tsx";
import { ContactSheetDialog } from "../text/ContactSheet.tsx";
import { TXT, isText, parseFontList, type FontInfo } from "../text/textModel.ts";
import { useT } from "../workspace/usePrefs.ts";
import { ExamplesDialog } from "./Examples.tsx";
import { FillInDialog } from "./FillIn.tsx";
import { TemplateFieldsDialog } from "./TemplateFields.tsx";
import { TPL, hasFields, type FieldKind } from "./templateModel.ts";
import "./templates.css";

type Modal = { kind: "fill" } | { kind: "fields"; start?: FieldKind } | { kind: "examples" } | { kind: "sheet" };

/** Ask for the fill-in form from elsewhere (the Variable Text window opens the contact sheet the same way). */
export const OPEN_FILL_IN = "openetch:fill-in";
export const OPEN_CONTACT_SHEET = "openetch:contact-sheet";

export interface TemplateUi {
  fileItems: (close: () => void) => ReactNode;
  menuItems: MenuItem[];
  hud: ReactNode;
  dialogs: ReactNode;
  modalOpen: boolean;
  openExamples: () => void;
}

export function useTemplateUi(p: {
  engine: Engine;
  file: FileActions;
  selection: string[];
  design: boolean;
  notify: (m: string, k?: "info" | "error") => void;
  report: (e: unknown) => void;
}): TemplateUi {
  const t = useT();
  const { engine } = p;
  const { doc, has, run } = engine;
  const [modal, setModal] = useState<Modal | null>(null);
  const [fonts, setFonts] = useState<FontInfo[] | null>(null);
  const close = useCallback(() => setModal(null), []);
  const can = { fill: has(TPL.fill), fields: has(TPL.setField), examples: has(TPL.examples), sheet: has(TXT.contactSheet) };
  const fields = hasFields(doc);
  const variable = !!doc?.objects.some((o) => (isText(o) && (o.geometry.variable?.mode === "serial" || o.geometry.variable?.mode === "csv")) || !!o.extras?.["openetch.barcode"]?.includes('"variable"'));

  // Other windows ask for the form or the sheet by event (no prop threading through the Variable Text window).
  useEffect(() => {
    const fill = () => setModal({ kind: "fill" });
    const sheet = () => setModal({ kind: "sheet" });
    window.addEventListener(OPEN_FILL_IN, fill);
    window.addEventListener(OPEN_CONTACT_SHEET, sheet);
    return () => {
      window.removeEventListener(OPEN_FILL_IN, fill);
      window.removeEventListener(OPEN_CONTACT_SHEET, sheet);
    };
  }, []);

  // The designer's dialog needs the fonts for font choices.
  useEffect(() => {
    if (modal?.kind !== "fields" || fonts) return;
    execute({ id: TXT.fontList, params: {} })
      .then((r) => setFonts(parseFontList(r.result)))
      .catch(() => setFonts([]));
  }, [modal, fonts]);

  // A template (or project) with fields that was just opened shows its form: an open starts a fresh timeline whose
  // one step loads it (files/fileFormats.ts openedRevision). Examples (doc.new) show their notes instead, and the
  // page's first document opens nothing.
  const seen = useRef<string | null>(null);
  useEffect(() => {
    if (!doc) return;
    const steps = engine.timeline.steps;
    const s0 = steps.length === 1 ? steps[0] : null;
    // Wait until the timeline and the document agree.
    if (s0 && s0.revision !== doc.revision) return;
    const key = s0 ? `${s0.commands}@${s0.revision}` : "";
    const first = seen.current === null;
    if (key === seen.current) return;
    seen.current = key;
    if (first || !s0) return;
    if (/(^|[^\w.])doc\.(load|import_lbrn2)($|[^\w])/.test(s0.commands) && hasFields(doc)) setModal((m) => m ?? { kind: "fill" });
  }, [doc, engine.timeline]);

  const fileItems = (closeMenu: () => void) => (
    <>
      {can.examples && <FileMenuPart section="new" close={closeMenu} items={[{ label: t("tpl.newFromExample"), icon: "book", disabled: p.file.busy, action: () => setModal({ kind: "examples" }) }]} />}
      {(can.fill || can.fields) && (
        <FileMenuPart
          section="project"
          close={closeMenu}
          items={[
            ...(can.fill ? [{ label: t("tpl.fillInMenu"), disabled: !fields, title: fields ? undefined : t("tpl.noFieldsTitle"), action: () => setModal({ kind: "fill" }) }] : []),
            ...(can.fields ? [{ label: t("tpl.fieldsMenu"), action: () => setModal({ kind: "fields" }) }] : []),
          ]}
        />
      )}
      {can.sheet && (
        <FileMenuPart
          section="laser"
          close={closeMenu}
          items={[{ label: t("sheet.menu"), disabled: !variable, title: variable ? undefined : t("sheet.noVariables"), action: () => setModal({ kind: "sheet" }) }]}
        />
      )}
    </>
  );

  const selText = p.selection.length === 1 && doc ? doc.objects.find((o) => o.id === p.selection[0] && isText(o)) : undefined;
  const menuItems: MenuItem[] =
    p.design && can.fields && selText && !(isText(selText) && selText.geometry.variable)
      ? [{ label: t("tpl.makeField"), action: () => setModal({ kind: "fields", start: "text" }) }]
      : [];

  const hud =
    fields && can.fill ? (
      <HudItem id="fill-in" slot="tr" rank={40} optional>
        <div className="vp-overlay tpl-hud-wrap">
          <button className="btn tpl-hud" onClick={() => setModal({ kind: "fill" })} title={t("tpl.fillInTitle")}>
            <Icon name="fillIn" size={18} /> {t("tpl.fillIn")}
          </button>
        </div>
      </HudItem>
    ) : null;

  const m = modal;
  const dialogs = (
    <>
      {m?.kind === "fill" && <FillInDialog run={run} onClose={close} notify={p.notify} onEditFields={can.fields ? () => setModal({ kind: "fields" }) : undefined} />}
      {m?.kind === "fields" && doc && (
        <TemplateFieldsDialog doc={doc} selection={p.selection} fonts={fonts} run={run} report={p.report} onClose={close} onFillIn={() => setModal({ kind: "fill" })} start={m.start} />
      )}
      {m?.kind === "examples" && (
        <ExamplesDialog
          onClose={close}
          onOpen={(id, title) => {
            setModal(null);
            p.file.newFromExample(id, title);
          }}
        />
      )}
      {m?.kind === "sheet" && (
        <ContactSheetDialog
          onClose={close}
          onGoto={has(TXT.setSerial) ? (value) => void run({ id: TXT.setSerial, params: { current: value } }).then(() => p.notify(t("sheet.shown", { n: value }))).catch(p.report) : undefined}
        />
      )}
    </>
  );

  return { fileItems, menuItems, hud, dialogs, modalOpen: modal !== null, openExamples: () => setModal({ kind: "examples" }) };
}
