// "Saved in this browser": where OpenEtch Design keeps projects, and how that storage can be lost (docs/design-only/
// plan.md "The Projects dialog"): this browser on this device only, Safari's 7-day eviction, shared github.io storage,
// and a downloaded copy as the safe keeping. Shown in the Open dialog under the project list.

import { useT } from "../workspace/usePrefs.ts";
import "./designmode.css";

export function StorageNotices() {
  const t = useT();
  return (
    <ul className="hint dm-storage" aria-label={t("dm.projectsTitle")}>
      <li>{t("dm.storageLocal")}</li>
      <li>{t("dm.storageSafari")}</li>
      <li>{t("dm.storageShared")}</li>
      <li>{t("dm.storageCopy")}</li>
    </ul>
  );
}
