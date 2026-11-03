// Where the user guide shows (route.ts has the `#/guide...` routes): a side panel beside the canvas on a desktop, a
// MacBook or an iPad in landscape (App.tsx puts it after the right dock, and reserves its width so the docks rail
// before the canvas gets too narrow), a full-screen sheet under the top bar on a phone or an iPad in portrait (STOP
// stays in view above it), or the whole tab for `?guide` (Open in new tab, main.tsx). The viewer and the guide's
// pages are one lazy chunk (GuideViewer.tsx), loaded the first time the guide opens, so start-up does not grow.

import { Component, lazy, Suspense, useLayoutEffect, useMemo, useState, useSyncExternalStore, type CSSProperties, type ReactNode } from "react";
import { useT } from "../workspace/usePrefs.ts";
import { closeGuide, guideHashNow, parseGuideHash, subscribeGuide, type GuideRoute } from "./route.ts";
import "./guide-shell.css";

const loadViewer = () => import("./GuideViewer.tsx");
let Viewer = lazy(loadViewer);

/** The guide route in the address, or null while the guide is closed. */
export function useGuideRoute(): GuideRoute | null {
  const hash = useSyncExternalStore(subscribeGuide, guideHashNow, () => "");
  return useMemo(() => parseGuideHash(hash), [hash]);
}

/** Start fetching the viewer chunk (on hover of Help > User guide, say), without showing it. */
export function prefetchGuide(): void {
  void loadViewer().catch(() => {});
}

class LoadBoundary extends Component<{ fallback: (retry: () => void) => ReactNode; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    if (!this.state.failed) return this.props.children;
    return this.props.fallback(() => {
      // React.lazy keeps a failed import: a new one tries the network again.
      Viewer = lazy(loadViewer);
      this.setState({ failed: false });
    });
  }
}

function Load(p: { route: GuideRoute; mode: "pane" | "sheet" | "page"; newTab: boolean }) {
  const t = useT();
  const [n, setN] = useState(0);
  return (
    <LoadBoundary
      key={n}
      fallback={(retry) => (
        <div className="guide-loading" role="alert">
          <p>{t("guide.loadFailed")}</p>
          <p className="guide-loading-actions">
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                retry();
                setN((k) => k + 1);
              }}
            >
              {t("guide.retry")}
            </button>
            {p.mode !== "page" && (
              <button type="button" className="btn" onClick={closeGuide}>
                {t("guide.close")}
              </button>
            )}
          </p>
        </div>
      )}
    >
      <Suspense fallback={<div className="guide-loading">{t("guide.loading")}</div>}>
        <Viewer route={p.route} mode={p.mode} newTab={p.newTab} />
      </Suspense>
    </LoadBoundary>
  );
}

/** The guide beside the canvas (a flex column of `.app > .main`, after the right dock). */
export function GuidePane(p: { route: GuideRoute; width: number }) {
  const t = useT();
  return (
    <aside className="guide-pane" style={{ width: p.width }} aria-label={t("guide.title")}>
      <Load route={p.route} mode="pane" newTab />
    </aside>
  );
}

/** The bottom of the production cockpit's own bar (its STOP and Exit), or null when the cockpit is not open. */
function useCockpitBarBottom(): number | null {
  const [bottom, setBottom] = useState<number | null>(null);
  useLayoutEffect(() => {
    const bar = document.querySelector<HTMLElement>(".cockpit-bar");
    if (!bar) return;
    const read = () => setBottom(Math.round(bar.getBoundingClientRect().bottom));
    read();
    const ro = new ResizeObserver(read);
    ro.observe(bar);
    window.addEventListener("resize", read);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", read);
    };
  }, []);
  return bottom;
}

/** While the sheet lies over a dialog, the dialog is inert: one modal at a time for screen readers and the keyboard
 * (Close the guide gives it back). */
function useInertDialogs(): void {
  useLayoutEffect(() => {
    const under = [...document.querySelectorAll<HTMLElement>(".modal-backdrop")].filter((e) => !e.inert);
    for (const e of under) e.inert = true;
    return () => {
      for (const e of under) e.inert = false;
    };
  }, []);
}

/** The guide as a full-screen sheet under the top bar (phones, narrow tablets), or under the production cockpit's
 * bar while the cockpit is open (its bar is taller than the app's). */
export function GuideSheet(p: { route: GuideRoute; newTab: boolean }) {
  const t = useT();
  const cockpitBottom = useCockpitBarBottom();
  useInertDialogs();
  const style: CSSProperties | undefined = cockpitBottom === null ? undefined : { top: cockpitBottom };
  return (
    <div className="guide-sheet" style={style} role="dialog" aria-modal="true" aria-label={t("guide.title")}>
      <Load route={p.route} mode="sheet" newTab={p.newTab} />
    </div>
  );
}

/** The guide alone in its tab (`?guide`): no engine, no canvas. */
export function GuideStandalone() {
  const route = useGuideRoute() ?? { slug: "index", anchor: null };
  return (
    <main className="guide-standalone">
      <Load route={route} mode="page" newTab={false} />
    </main>
  );
}
