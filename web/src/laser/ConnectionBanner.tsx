// The lost-connection banner (owner: P5, docs/ux/ui-plan-2026-11.md P5): "Lost connection to the laser engine –
// reconnecting… STOP may not reach the laser; use the hardware E-stop." It shows at once when the event socket closes
// (or the self poll fails), after a short grace while it keeps reconnecting, and goes away when the engine answers.

import { useEffect, useRef, useState } from "react";
import { useT } from "../workspace/usePrefs.ts";
import { onlineOf } from "./laserContext.ts";
import { BANNER_GRACE_MS, bannerVisible } from "./statusChip.ts";
import { useLaserLive } from "./useLaserLive.ts";
import "./laser.css";

export function ConnectionBanner() {
  const t = useT();
  const { facts } = useLaserLive();
  const online = onlineOf(facts);
  const lostAt = useRef<number | null>(null);
  const everOnline = useRef(false);
  const [, tick] = useState(0);
  if (online) {
    lostAt.current = null;
    everOnline.current = true;
  } else lostAt.current ??= Date.now();
  // Re-render once the grace period is over (the link may sit in "connecting" without changing).
  useEffect(() => {
    if (online) return;
    const id = window.setTimeout(() => tick((n) => n + 1), BANNER_GRACE_MS + 50);
    return () => window.clearTimeout(id);
  }, [online, facts.link]);
  const show = bannerVisible(online, facts.link, lostAt.current === null ? 0 : Date.now() - lostAt.current);
  if (!show) return null;
  return (
    <div className="conn-banner" role="alert">
      <span className="conn-banner-dot" aria-hidden="true" />
      <span className="conn-banner-text">
        <strong>{everOnline.current ? t("lz.lost") : t("lz.lostNever")}</strong> <span>{t("lz.lostStop")}</span>
      </span>
    </div>
  );
}
