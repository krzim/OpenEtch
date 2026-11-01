// LightBurn's Optimization Settings "Set as Defaults" and reset, per device: the document's optimization settings
// saved as the defaults new documents start with (POST / DELETE /api/optimization/default), and the saved defaults
// (or OpenEtch's) applied to this document (POST /api/optimization/reset, one undo step).

import { useState } from "react";
import { useT } from "../workspace/usePrefs.ts";
import { libsApi } from "./api.ts";
import { refreshPrefs, useDevicePrefs } from "./store.ts";
import { DropMenu, errText } from "./ui.tsx";
import "./libs.css";

export function OptimizationDefaults(p: { dirty: boolean; onApplied: () => void; report: (e: unknown) => void }) {
  const t = useT();
  const { prefs, supported } = useDevicePrefs();
  const [done, setDone] = useState<string | null>(null);
  if (supported === false) return null;
  const fail = (e: unknown) => p.report(new Error(errText(e)));
  const saved = (msg: string) => () => {
    setDone(msg);
    return refreshPrefs();
  };
  return (
    <>
      <DropMenu
        label={t("odef.set")}
        up
        title={p.dirty ? t("odef.saveFirst") : t("odef.setTitle")}
        items={[
          {
            label: t("odef.set"),
            hint: p.dirty ? t("odef.saveFirst") : undefined,
            disabled: p.dirty,
            action: () => void libsApi.optimizationMakeDefault().then(saved(t("odef.saved"))).catch(fail),
          },
          {
            label: t("odef.reset"),
            disabled: !prefs.has_optimization_defaults,
            action: () => void libsApi.optimizationReset(false).then(p.onApplied).catch(fail),
          },
          { label: t("ldef.resetBuiltinItem"), action: () => void libsApi.optimizationReset(true).then(p.onApplied).catch(fail) },
          { sep: true },
          {
            label: t("odef.forget"),
            disabled: !prefs.has_optimization_defaults,
            action: () => void libsApi.optimizationForgetDefault().then(saved(t("odef.forgotten"))).catch(fail),
          },
        ]}
      />
      {done && (
        <span className="hint lx-done" role="status">
          {done}
        </span>
      )}
    </>
  );
}
