// LightBurn's 3D Sliced image mode settings in a layer's Image section (galvo): number of slices (the layer's main
// passes; pass k of N engraves where the depth map is darker than (N - k) / N, deepest first), "Enable cleanup pass"
// (a cleanup sub-layer, `layer.sub_add` with LightBurn-like cleanup settings), "Clean after # of passes" and "Clean
// whole image" (`settings.image.clean_whole_image`). The depth reached on the B6 is not verified on hardware.

import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { imageUpdate } from "../api/image.ts";
import { cleanupPass } from "../api/passes.ts";
import type { Layer } from "../api/types.ts";
import { CheckField, NumberField } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { isSlice3d, slice3dState } from "./fileFormats.ts";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export function Slice3dSettings(p: { layer: Layer; run: Run; report: (e: unknown) => void; has?: (id: string) => boolean }) {
  const t = useT();
  const img = (p.layer.settings as { image?: { dither?: unknown } }).image;
  if (!isSlice3d(img?.dither)) return null;
  const s = slice3dState(p.layer);
  const go = (inv: Invocation) => void p.run(inv).catch(p.report);
  const canSub = p.has?.(C.OPT.layerSubAdd) ?? false;
  const setCleanup = (on: boolean) => {
    if (on && s.cleanup === null) {
      const c = cleanupPass(p.layer.settings);
      go(C.layerSubAdd(p.layer.id, { ...c, cleanup: { every_n_passes: Math.max(1, s.slices) } }));
    } else if (!on && s.cleanup !== null) go(C.layerSubDelete(p.layer.id, s.cleanup));
  };
  return (
    <div className="slice3d">
      <div className="section-label">{t("files.slice3d")}</div>
      <NumberField
        label={t("files.slices")}
        value={s.slices}
        min={1}
        max={1000}
        digits={0}
        onCommit={(v) => v !== null && go(C.layerUpdate(p.layer.id, { settings: { passes: Math.round(v) } }))}
      />
      <CheckField label={t("files.cleanupPass")} checked={s.cleanup !== null} onChange={(v) => canSub && setCleanup(v)} />
      {s.cleanup !== null && (
        <NumberField
          label={t("files.cleanAfter")}
          value={s.everyN}
          min={1}
          max={1000}
          digits={0}
          onCommit={(v) => v !== null && s.cleanup !== null && go(C.layerSubUpdate(p.layer.id, s.cleanup, { cleanup: { every_n_passes: Math.round(v) } }))}
        />
      )}
      <CheckField
        label={t("files.cleanWhole")}
        checked={s.cleanWholeImage}
        onChange={(clean_whole_image) => go(imageUpdate(p.layer.id, { clean_whole_image }))}
      />
      <p className="hint">{t("files.slice3dHint", { n: s.slices })}</p>
    </div>
  );
}
