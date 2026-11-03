// The MOPA materials UI for App.tsx in one hook (ARCHITECTURE.md "MOPA materials"): its dialogs (finish of a
// preset, bracket test, colour separation, hue step model), the bracket test's canvas HUD bar, a Materials dropdown in
// the ribbon's Laser Tools group and a Result colours toggle in INSPECT, the canvas context menu's Colour separation
// for an image, the hotkey / command search handlers, and `viewDoc`, the document as the canvas draws it in Result
// colours view (each layer in the colour it should come out). App renders `dialogs`, `hud` and `toolbar`.

import { useMemo, type ReactNode } from "react";
import type { GDocument } from "../api/types.ts";
import { Dropdown } from "../components/EditMenus.tsx";
import type { MenuItem } from "../components/Menu.tsx";
import { RibbonSlot } from "../components/Ribbon.tsx";
import { Icon } from "../components/ui.tsx";
import { openLibsWindow, useLibrary } from "../libs/store.ts";
import { useT } from "../workspace/usePrefs.ts";
import { BracketDialog, BracketHud } from "./Bracket.tsx";
import { ColorSeparateDialog } from "./ColorSeparate.tsx";
import { FinishDialog } from "./FinishDialog.tsx";
import { HueModelDialog } from "./HueModelDialog.tsx";
import { linkedColorLookup, withResultColors } from "./model.ts";
import { closeMaterials, materialsWindow, openMaterials, pickedEntry, resultColors, setResultColors } from "./store.ts";
import "./materials.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

export interface MaterialsUiProps {
  doc: GDocument | null;
  selection: readonly string[];
  design: boolean;
  notify: Notify;
  report: (e: unknown) => void;
  selectIds: (ids: string[]) => void;
}

export function useMaterialsUi(p: MaterialsUiProps) {
  const t = useT();
  const win = materialsWindow.use();
  const result = resultColors.use();
  const picked = pickedEntry.use();
  const lib = useLibrary();

  // ---- Result colours view ----
  const lookup = useMemo(() => linkedColorLookup(lib.view?.selected ?? null, lib.view?.tree ?? []), [lib.view]);
  const shown = useMemo(() => (result && p.doc ? withResultColors(p.doc, lookup) : p.doc), [result, p.doc, lookup]);
  const toggleResult = () => {
    const on = !resultColors.get();
    setResultColors(on);
    p.notify(t(on ? "mat.resultColorsOn" : "mat.resultColorsOff"));
  };

  // ---- actions ----
  const images = (p.doc?.objects ?? []).filter((o) => p.selection.includes(o.id) && o.geometry.kind === "image");
  const image = images.length === 1 ? images[0] : null;
  const separate = () => {
    if (!image) return void p.notify(t("mat.sepSelectImage"), "error");
    openMaterials({ kind: "separate", image: image.id });
  };
  const bracket = () => {
    if (!picked) {
      p.notify(t("mat.bracketPickFirst"), "error");
      openLibsWindow("library");
      return;
    }
    openMaterials({ kind: "bracket", library: picked.library, id: picked.id });
  };

  const items: MenuItem[] = [
    { label: t("mat.bracketMenu"), actionId: "laser.bracketTest", action: bracket },
    { label: t("mat.separate"), actionId: "image.colorSeparate", disabled: !image, action: separate },
    { sep: true },
    { label: t("mat.resultColors"), actionId: "view.resultColors", checked: result, action: toggleResult },
    { label: t("mat.modelEdit"), actionId: "laser.hueModel", action: () => openMaterials({ kind: "hueModel" }) },
  ];

  const toolbar: ReactNode = p.design ? (
    <>
      <RibbonSlot group="laserTools" order={10}>
        <Dropdown icon="palette" label={t("mat.materials")} title={t("mat.materialsTitle")} items={items} badge={result} />
      </RibbonSlot>
      <RibbonSlot group="inspect" order={60}>
        <span className="rb-toggles" role="group" aria-label={t("mat.resultColors")}>
          <button
            type="button"
            className={`mini-btn${result ? " active" : ""}`}
            aria-pressed={result}
            aria-label={t("mat.resultColors")}
            title={t("mat.resultColorsTitle", { state: result ? t("tb.on") : t("tb.off") })}
            onClick={toggleResult}
          >
            <Icon name="palette" size={18} />
          </button>
        </span>
      </RibbonSlot>
    </>
  ) : null;

  const menuItems: MenuItem[] = image && p.design ? [{ label: t("mat.separate"), actionId: "image.colorSeparate", action: separate, section: "modify" } as MenuItem] : [];

  const handlers: Record<string, () => boolean | void> = {
    "view.resultColors": () => toggleResult(),
    "laser.bracketTest": () => (p.design ? bracket() : false),
    "image.colorSeparate": () => (p.design && image ? separate() : false),
    "laser.hueModel": () => openMaterials({ kind: "hueModel" }),
  };

  let dialogs: ReactNode = null;
  switch (win?.kind) {
    case "finish":
      dialogs = <FinishDialog key={`${win.library}/${win.id}`} library={win.library} entryId={win.id} photo={win.photo} notify={p.notify} report={p.report} onClose={closeMaterials} />;
      break;
    case "bracket":
      dialogs = <BracketDialog library={win.library} entryId={win.id} notify={p.notify} report={p.report} onClose={closeMaterials} />;
      break;
    case "separate":
      dialogs = <ColorSeparateDialog image={win.image} notify={p.notify} report={p.report} onClose={closeMaterials} onCreated={(ids) => ids.length && p.selectIds(ids)} />;
      break;
    case "hueModel":
      dialogs = <HueModelDialog notify={p.notify} report={p.report} onClose={closeMaterials} />;
      break;
  }

  const hud = <BracketHud doc={p.doc} selection={p.selection} design={p.design} notify={p.notify} report={p.report} />;

  return { viewDoc: shown, dialogs, hud, toolbar, menuItems, handlers, items, resultColors: result, modalOpen: !!win };
}
