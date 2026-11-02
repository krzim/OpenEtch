// The phone layout (docs/ux/ui-plan-2026-11.md 1.1, "Phone"): a separate shell that reuses the app's components
// through slots. Top bar (menu, document name, status dot, STOP), a full-bleed canvas with a few buttons over it, a
// bottom sheet, and four tabs above the home indicator:
// - Canvas: the tool strip and, with a selection, the selection sheet (it peeks open when something is selected, so
//   the shape stays visible above it);
// - Layers: Cuts / Layers with the palette;
// - Job: the Laser panel (App shows the Laser workspace, so the canvas previews the job);
// - More: File, Import, Objects, History, Preferences, "Use as an app", Help and the desktop-only features.
// A tap on the active tab opens or closes its sheet.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { OPEN_LASER_EVENT } from "../laser/laserContext.ts";
import { BottomSheet } from "./BottomSheet.tsx";
import type { SheetSnap } from "./phone.ts";
import { ShellIcon, type ShellIconName } from "./shellIcons.tsx";
import { useT } from "./usePrefs.ts";
import "./phone.css";

export type PhoneTab = "canvas" | "layers" | "job" | "more";
export const PHONE_TABS: readonly PhoneTab[] = ["canvas", "layers", "job", "more"];

/** Each tab's sheet height when it opens. */
const OPEN_AT: Record<PhoneTab, SheetSnap> = { canvas: "peek", layers: "half", job: "half", more: "full" };
const ICON: Record<PhoneTab, ShellIconName> = { canvas: "pointer", layers: "layers", job: "zap", more: "ellipsis" };

export interface PhoneShellProps {
  /** The top bar; `openMenu` opens the More sheet (the bar's menu button). */
  topBar: (openMenu: () => void) => ReactNode;
  /** Rows under the top bar (the connection banner). */
  banner?: ReactNode;
  /** The canvas (`.center-stage`). */
  canvas: ReactNode;
  /** Buttons over the canvas, bottom right (undo, redo). */
  canvasHud?: ReactNode;
  /** The drawing tools (the ribbon as a strip). */
  toolStrip: ReactNode;
  /** The selection's fields and quick actions; null without a selection. */
  selectionSheet: ReactNode | null;
  layersSheet: ReactNode;
  jobSheet: ReactNode;
  moreItems: ReactNode;
  /** The tab changed (App shows the Laser workspace while Job is the tab). */
  onTab?: (tab: PhoneTab) => void;
  /** Dialogs, menus, toasts: rendered after the shell. */
  overlays?: ReactNode;
  /** Extra classes for the root (the workspace). */
  className?: string;
}

export function PhoneShell(p: PhoneShellProps) {
  const t = useT();
  const [tab, setTab] = useState<PhoneTab>("canvas");
  const [open, setOpen] = useState(false);
  const [snap, setSnap] = useState<SheetSnap>("peek");
  /** The Canvas sheet was opened by a selection (and closes again when the selection goes). */
  const bySelection = useRef(false);
  const hasSelection = p.selectionSheet !== null;
  const { onTab } = p;

  const show = (next: PhoneTab, snapTo: SheetSnap = OPEN_AT[next]) => {
    if (next !== tab) onTab?.(next);
    setTab(next);
    setSnap(snapTo);
    setOpen(true);
    bySelection.current = false;
  };

  // The Start flow asks for the Laser window (checklist, Start from Design): on the phone that is the Job sheet.
  const showRef = useRef(show);
  showRef.current = show;
  useEffect(() => {
    const open = () => showRef.current("job");
    window.addEventListener(OPEN_LASER_EVENT, open);
    return () => window.removeEventListener(OPEN_LASER_EVENT, open);
  }, []);

  // Selecting a shape on the Canvas tab peeks the selection sheet open; clearing the selection closes it again.
  useEffect(() => {
    if (tab !== "canvas") return;
    if (hasSelection && !open) {
      setSnap("peek");
      setOpen(true);
      bySelection.current = true;
    } else if (!hasSelection && open && bySelection.current) {
      setOpen(false);
      bySelection.current = false;
    }
    // Only a change of selection decides; opening or closing by hand is left alone.
  }, [hasSelection]);

  const titles: Record<PhoneTab, string> = {
    canvas: hasSelection ? t("shell.phoneSelection") : t("shell.phoneTools"),
    layers: t("shell.phoneLayers"),
    job: t("shell.phoneJob"),
    more: t("shell.more"),
  };
  const content: Record<PhoneTab, ReactNode> = {
    canvas: (
      <>
        {p.selectionSheet}
        <div className="phone-tools">{p.toolStrip}</div>
      </>
    ),
    layers: p.layersSheet,
    job: p.jobSheet,
    more: <div className="phone-more">{p.moreItems}</div>,
  };
  return (
    <div className={`app phone-shell${p.className ? ` ${p.className}` : ""}`}>
      {p.topBar(() => show("more"))}
      {p.banner}
      <div className="phone-main">
        {p.canvas}
        <div className="phone-hud">
          {p.canvasHud}
          <button type="button" className="icon-btn phone-hud-btn" title={t("shell.phoneTools")} aria-label={t("shell.phoneTools")} onClick={() => show("canvas")}>
            <ShellIcon name="pencil" />
          </button>
        </div>
        <BottomSheet
          open={open}
          snap={snap}
          onSnap={setSnap}
          title={titles[tab]}
          onClose={() => {
            setOpen(false);
            bySelection.current = false;
          }}
        >
          {content[tab]}
        </BottomSheet>
      </div>
      <nav className="phone-tabs" role="tablist" aria-label={t("shell.phoneTabs")}>
        {PHONE_TABS.map((x) => (
          <button
            key={x}
            type="button"
            role="tab"
            aria-selected={tab === x}
            className={`phone-tab${tab === x ? " active" : ""}`}
            onClick={() => {
              if (tab === x) {
                setOpen((o) => !o);
                bySelection.current = false;
              } else show(x);
            }}
          >
            <ShellIcon name={ICON[x]} size={22} />
            <span>{x === "canvas" ? t("shell.phoneCanvas") : x === "more" ? t("shell.more") : titles[x]}</span>
          </button>
        ))}
      </nav>
      {p.overlays}
    </div>
  );
}
