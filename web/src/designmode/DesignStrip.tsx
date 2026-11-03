// OpenEtch Design's header strip, under the top bar on every layout (docs/design-only/plan.md "Show instead"): what
// this site is ("runs in your browser, nothing is uploaded") and the way to the full app with laser control. One slim
// row; on a phone the short texts, and on touch screens the link is a full 44 px target.

import { FULL_APP_URL } from "../mode.ts";
import { useT } from "../workspace/usePrefs.ts";
import "./designmode.css";

export function DesignStrip() {
  const t = useT();
  return (
    <div className="dm-strip" role="note" aria-label={t("dm.strip")}>
      <span className="dm-strip-text" title={t("dm.stripTitle")}>
        <span className="dm-long">{t("dm.strip")}</span>
        <span className="dm-short">{t("dm.stripShort")}</span>
      </span>
      <a className="dm-strip-link" href={FULL_APP_URL} target="_blank" rel="noopener noreferrer" title={t("dm.fullAppTitle")}>
        <span className="dm-long">{t("dm.fullApp")}</span>
        <span className="dm-short">{t("dm.fullAppShort")}</span>
        <span aria-hidden="true"> ↗</span>
      </a>
    </div>
  );
}
