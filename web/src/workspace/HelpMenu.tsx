// Top bar extras: the Help menu (keyboard shortcuts, documentation links, About with versions and the MCP address),
// the Preferences button and a quick theme switch (Light / Dark / Match system). In the compact layouts and below they
// fold, with the Window menu, into one ⋯ overflow menu (docs/ux/ui-plan-2026-11.md 1.1).

import { useEffect, useRef, useState, type ReactNode } from "react";
import type { MachineStatus } from "../api/types.ts";
import { Icon } from "../components/ui.tsx";
import { hintFor } from "./hotkeys.ts";
import { updatePrefs, type ThemePref } from "./prefs.ts";
import { usePrefs, useT } from "./usePrefs.ts";
import { ShellIcon } from "./shellIcons.tsx";
import { WsModal } from "./WsModal.tsx";
import "./workspace.css";

export const REPO_URL = "https://github.com/krzim/OpenEtch";
export const DOC_LINKS = {
  readme: `${REPO_URL}#readme`,
  architecture: `${REPO_URL}/blob/main/ARCHITECTURE.md`,
  webReadme: `${REPO_URL}/blob/main/web/README.md`,
  issues: `${REPO_URL}/issues`,
  lightburn: "https://docs.lightburnsoftware.com/",
};

/** The web UI's version (package.json, injected by vite.config.ts; "dev" under the Node test runner). */
export const WEB_VERSION: string = typeof __OPENETCH_VERSION__ === "string" ? __OPENETCH_VERSION__ : "dev";

/** The engine's version if GET /api/machine reports one. */
export function engineVersion(m: MachineStatus | null): string | null {
  const v = m?.version ?? m?.engine_version;
  return typeof v === "string" && v ? v : null;
}

function useMenu() {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => root.current && !root.current.contains(e.target as Node) && setOpen(false);
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", down);
    window.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", down);
      window.removeEventListener("keydown", key);
    };
  }, [open]);
  return { open, setOpen, root };
}

export interface TopActionsProps {
  machine: MachineStatus | null;
  onHotkeys: () => void;
  onPreferences: () => void;
  /** More Help entries (src/libs: debug log, Report a Bug, Check for Updates), above About. */
  helpExtra?: (close: () => void) => ReactNode;
  /** Compact layouts and below: Window, Help, theme and Preferences fold into one ⋯ menu. */
  overflow?: boolean;
  /** The Window menu's entries (DockHost.tsx `WindowMenuEntries`), shown in the ⋯ menu. */
  windowItems?: (close: () => void) => ReactNode;
}

/** The Help menu's entries (also listed in the ⋯ overflow menu and the phone's More sheet). */
export function HelpItems(p: { close: () => void; onHotkeys: () => void; onAbout: () => void; helpExtra?: (close: () => void) => ReactNode }) {
  const t = useT();
  const prefs = usePrefs();
  const link = (href: string, label: string) => (
    <a role="menuitem" className="menu-item" href={href} target="_blank" rel="noopener noreferrer" onClick={p.close}>
      <span>{label}</span>
      <span className="menu-hint">↗</span>
    </a>
  );
  return (
    <>
      <button
        role="menuitem"
        className="menu-item"
        onClick={() => {
          p.close();
          p.onHotkeys();
        }}
      >
        <span>{t("help.hotkeys")}</span>
        <span className="menu-hint">{hintFor("help.hotkeys", prefs.hotkeys)}</span>
      </button>
      <div className="menu-sep" role="separator" />
      {link(DOC_LINKS.readme, t("help.readme"))}
      {link(DOC_LINKS.architecture, t("help.architecture"))}
      {link(DOC_LINKS.webReadme, t("help.webReadme"))}
      {link(DOC_LINKS.lightburn, t("help.lightburn"))}
      {link(DOC_LINKS.issues, t("help.issues"))}
      <div className="menu-sep" role="separator" />
      {p.helpExtra && (
        <>
          {p.helpExtra(p.close)}
          <div className="menu-sep" role="separator" />
        </>
      )}
      <button
        role="menuitem"
        className="menu-item"
        onClick={() => {
          p.close();
          p.onAbout();
        }}
      >
        <span>{t("help.about")}</span>
      </button>
    </>
  );
}

const NEXT_THEME: Record<ThemePref, ThemePref> = { light: "dark", dark: "system", system: "light" };

export function TopActions(p: TopActionsProps) {
  const t = useT();
  const prefs = usePrefs();
  const help = useMenu();
  const [about, setAbout] = useState(false);
  const themeName = { light: t("theme.light"), dark: t("theme.dark"), system: t("theme.system") }[prefs.theme];
  const cycleTheme = () => updatePrefs({ theme: NEXT_THEME[prefs.theme] });
  const close = () => help.setOpen(false);
  if (p.overflow) {
    return (
      <>
        <div className="help-menu file-menu top-overflow" ref={help.root}>
          <button
            className={`icon-btn${help.open ? " open" : ""}`}
            title={t("shell.more")}
            aria-label={t("shell.more")}
            aria-haspopup="menu"
            aria-expanded={help.open}
            onClick={() => help.setOpen((v) => !v)}
          >
            <ShellIcon name="ellipsis" />
          </button>
          {help.open && (
            <div className="menu menu-right" role="menu" aria-label={t("shell.more")}>
              {p.windowItems && (
                <>
                  <div className="menu-label">{t("win.menu")}</div>
                  {p.windowItems(close)}
                  <div className="menu-sep" role="separator" />
                </>
              )}
              <button role="menuitem" className="menu-item" onClick={cycleTheme}>
                <span>{t("theme.switchTitle", { theme: themeName })}</span>
                <ThemeIcon pref={prefs.theme} />
              </button>
              <button
                role="menuitem"
                className="menu-item"
                onClick={() => {
                  close();
                  p.onPreferences();
                }}
              >
                <span>{t("prefs.title")}</span>
                <span className="menu-hint">{hintFor("app.preferences", prefs.hotkeys)}</span>
              </button>
              <div className="menu-sep" role="separator" />
              <div className="menu-label">{t("help.menu")}</div>
              <HelpItems close={close} onHotkeys={p.onHotkeys} onAbout={() => setAbout(true)} helpExtra={p.helpExtra} />
            </div>
          )}
        </div>
        {about && <AboutDialog machine={p.machine} onClose={() => setAbout(false)} />}
      </>
    );
  }
  return (
    <>
      <button className="icon-btn theme-btn" title={t("theme.switchTitle", { theme: themeName })} aria-label={t("theme.switchTitle", { theme: themeName })} onClick={cycleTheme}>
        <ThemeIcon pref={prefs.theme} />
      </button>
      <button className="icon-btn" title={t("prefs.title")} aria-label={t("prefs.title")} onClick={p.onPreferences}>
        <GearIcon />
      </button>
      <div className="help-menu file-menu" ref={help.root}>
        <button className={`file-btn${help.open ? " open" : ""}`} aria-haspopup="menu" aria-expanded={help.open} onClick={() => help.setOpen((v) => !v)}>
          {t("help.menu")}
          <Icon name="chevronDown" size={16} />
        </button>
        {help.open && (
          <div className="menu menu-right" role="menu" aria-label={t("help.menu")}>
            <HelpItems close={close} onHotkeys={p.onHotkeys} onAbout={() => setAbout(true)} helpExtra={p.helpExtra} />
          </div>
        )}
      </div>
      {about && <AboutDialog machine={p.machine} onClose={() => setAbout(false)} />}
    </>
  );
}

/** The About dialog, for callers outside the Help menu (the phone's More sheet). */
export function AboutDialog(p: { machine: MachineStatus | null; onClose: () => void }) {
  return <AboutDialogBody {...p} />;
}

function AboutDialogBody(p: { machine: MachineStatus | null; onClose: () => void }) {
  const t = useT();
  const ev = engineVersion(p.machine);
  const origin = typeof location === "undefined" ? "" : location.origin;
  return (
    <WsModal
      title={t("help.aboutTitle")}
      onClose={p.onClose}
      footer={
        <button className="btn btn-primary" autoFocus onClick={p.onClose}>
          {t("common.close")}
        </button>
      }
    >
      <div className="about">
        <div className="about-brand">
          <span className="brand-mark" aria-hidden="true" />
          <span className="about-name">OpenEtch</span>
        </div>
        <p>{t("help.aboutText")}</p>
        <dl className="kv">
          <dt>{t("help.webVersion")}</dt>
          <dd>{WEB_VERSION}</dd>
          <dt>{t("help.engineVersion")}</dt>
          <dd>{ev ?? t("help.notReported")}</dd>
          <dt>{t("help.mcp")}</dt>
          <dd>
            <code>{`${origin}/mcp`}</code>
          </dd>
          <dt>{t("help.source")}</dt>
          <dd>
            <a href={REPO_URL} target="_blank" rel="noopener noreferrer">
              {REPO_URL.replace("https://", "")}
            </a>
          </dd>
        </dl>
        <p className="hint">{t("help.aboutNote")}</p>
      </div>
    </WsModal>
  );
}

export function ThemeIcon({ pref }: { pref: ThemePref }) {
  const d =
    pref === "light"
      ? "M12 8a4 4 0 100 8 4 4 0 000-8z M12 2v2 M12 20v2 M4.9 4.9l1.4 1.4 M17.7 17.7l1.4 1.4 M2 12h2 M20 12h2 M4.9 19.1l1.4-1.4 M17.7 6.3l1.4-1.4"
      : pref === "dark"
        ? "M20 14.5A8 8 0 019.5 4a8 8 0 1010.5 10.5z"
        : "M4 5h16v11H4z M9 20h6 M12 16v4 M12 7.5a3 3 0 000 6z";
  return (
    <svg className="icon" width={20} height={20} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={d} />
    </svg>
  );
}

function GearIcon() {
  return (
    <svg className="icon" width={20} height={20} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 9a3 3 0 100 6 3 3 0 000-6z M19.4 15a1.7 1.7 0 00.3 1.8l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.8-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.8.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.8 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.8l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.8.3H9a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.8-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.8V9a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z" />
    </svg>
  );
}
