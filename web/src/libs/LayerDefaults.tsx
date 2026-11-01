// Layer presets and defaults in the Properties panel and the Cuts / Layers menu (LightBurn's Cut Settings Editor
// "Make Default", "Make Default for All", "Reset to Default" (Shift: OpenEtch's own defaults), "Load default layer
// settings on New / restart"), plus the link badge of a layer linked to a library preset with Unlink and the Library
// window. Defaults are per device (POST /api/layers/{id}/default, /reset; GET / PUT /api/prefs).

import type { Invocation } from "../api/client.ts";
import type { Layer } from "../api/types.ts";
import type { MenuItem } from "../components/ContextMenu.tsx";
import { useT } from "../workspace/usePrefs.ts";
import type { T } from "../workspace/i18n.ts";
import { libsApi } from "./api.ts";
import { LinkIcon } from "./MaterialLibrary.tsx";
import { layerLink } from "./model.ts";
import { editPrefs, libsToast, openLibsWindow, refreshPrefs, useDevicePrefs } from "./store.ts";
import { DropMenu, errText } from "./ui.tsx";
import "./libs.css";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
type Notify = (msg: string, kind?: "info" | "error") => void;

export function unlinkPreset(layer: string): Invocation {
  return { id: "layer.unlink_preset", params: { layer } };
}

interface Ctx {
  t: T;
  run: Run;
  notify: Notify;
  report: (e: unknown) => void;
}

function makeDefault(c: Ctx, l: Layer, all: boolean) {
  libsApi
    .layerMakeDefault(l.id, all)
    .then(() => {
      c.notify(all ? c.t("ldef.savedAll", { name: l.name }) : c.t("ldef.saved", { name: l.name }));
      void refreshPrefs();
    })
    .catch((e) => c.report(new Error(errText(e))));
}

function reset(c: Ctx, l: Layer, builtin: boolean) {
  libsApi
    .layerReset(l.id, builtin)
    .then(() => c.notify(builtin ? c.t("ldef.resetBuiltin", { name: l.name }) : c.t("ldef.reset", { name: l.name })))
    .catch((e) => c.report(new Error(errText(e))));
}

/** Items for the Cuts / Layers right-click menu. */
export function layerPresetMenuItems(l: Layer, c: Ctx): MenuItem[] {
  const link = layerLink(l);
  const items: MenuItem[] = [
    { sep: true },
    { label: c.t("ldef.makeDefault"), action: () => makeDefault(c, l, false) },
    { label: c.t("ldef.makeDefaultAll"), action: () => makeDefault(c, l, true) },
    { label: c.t("ldef.resetDefault"), action: () => reset(c, l, false) },
    { label: c.t("ldef.resetBuiltinItem"), action: () => reset(c, l, true) },
    { sep: true },
    { label: c.t("ldef.library"), action: () => openLibsWindow("library", { layer: l.id }) },
  ];
  if (link) items.push({ label: c.t("ldef.unlink"), action: () => void c.run(unlinkPreset(l.id)).catch(c.report) });
  return items;
}

/** The link badge (with Unlink) and the Defaults menu, above a layer's cut settings. */
export function LayerPresetBar(p: { layer: Layer; run: Run; notify?: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const { prefs, supported } = useDevicePrefs();
  const link = layerLink(p.layer);
  const notify: Notify = p.notify ?? libsToast;
  const c: Ctx = { t, run: p.run, notify, report: p.report };
  const color = p.layer.color.toLowerCase();
  const hasOwn = prefs.layer_defaults.includes(color);
  const hasAll = prefs.layer_defaults.includes("*");
  return (
    <div className="lx-preset-bar">
      {link ? (
        <div className="lx-linked" role="status">
          <span className="lx-link-badge">
            <LinkIcon /> {t("ldef.linked")}
          </span>
          <span className="lx-linked-path" title={`${link.library}: ${link.path}`}>
            {link.path}
          </span>
          <button className="btn small" onClick={() => void p.run(unlinkPreset(p.layer.id)).catch(p.report)} title={t("ldef.unlinkTitle")}>
            {t("ldef.unlink")}
          </button>
        </div>
      ) : null}
      {supported !== false && (
        <div className="lx-preset-actions">
          <DropMenu
            label={t("ldef.defaults")}
            title={hasOwn ? t("ldef.hasOwn") : hasAll ? t("ldef.hasAll") : t("ldef.noneSaved")}
            items={[
              { label: t("ldef.makeDefault"), action: () => makeDefault(c, p.layer, false) },
              { label: t("ldef.makeDefaultAll"), action: () => makeDefault(c, p.layer, true) },
              { sep: true },
              { label: t("ldef.resetDefault"), hint: hasOwn || hasAll ? undefined : t("ldef.builtinHint"), action: () => reset(c, p.layer, false) },
              { label: t("ldef.resetBuiltinItem"), action: () => reset(c, p.layer, true) },
              { sep: true },
              {
                label: `${prefs.load_defaults_on_new ? "✓ " : ""}${t("ldef.loadOnNew")}`,
                action: () => void editPrefs({ load_defaults_on_new: !prefs.load_defaults_on_new }).catch((e) => p.report(new Error(errText(e)))),
              },
            ]}
          />
          <button className="btn small" onClick={() => openLibsWindow("library", { layer: p.layer.id })} title={t("ldef.libraryTitle")}>
            {t("ldef.library")}
          </button>
        </div>
      )}
      {link && <p className="hint">{t("ldef.linkedHint")}</p>}
    </div>
  );
}
