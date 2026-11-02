// Document tabs (LightBurn's File > New Window, here as tabs over one engine: ARCHITECTURE.md "Open documents"). One
// tab per open document with its name and an unsaved-changes dot; clicking a tab makes that document the active one
// (every edit, save, preview and run then goes to it), "+" opens a new empty document or a saved project in a new
// tab, and × closes a tab, asking first when it has unsaved changes. Shown only on engines with GET /api/documents.
// The list follows the engine's `documents_changed` and `project_saved` events, so a switch made by an agent or another
// browser shows here too. The tab row shows only with two or more documents; with one, its "+" sits in the top bar
// beside the document name.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError } from "../api/client.ts";
import { onEngineEvent } from "../api/events.ts";
import { projectList } from "../api/project.ts";
import type { ProjectEntry } from "../api/types.ts";
import { Icon } from "../components/ui.tsx";
import { useT } from "./usePrefs.ts";
import { WsModal } from "./WsModal.tsx";
import { neighbourAfterClose, wsApi, type DocumentList, type OpenDocument } from "./wsApi.ts";
import "./workspace.css";

export interface DocTabsProps {
  /** The active document's unsaved state as the File menu knows it (fresher than the list for the active tab). */
  dirty: boolean | null;
  /** Refetch the document, history and project after the active document changed. */
  refresh: () => Promise<void> | void;
  /** The active document changed (here or elsewhere): drop the selection and anything tied to the old document. */
  onSwitched: () => void;
  notify: (msg: string, kind?: "info" | "error") => void;
  report: (e: unknown) => void;
}

/** File menu entries for the tabs: New tab, Open project in new tab. */
export interface DocTabsApi {
  newTab: () => void;
  openInTab: (project: string) => void;
  /** The engine has several documents (GET /api/documents answers). */
  supported: boolean;
}

export interface DocTabsUi {
  api: DocTabsApi;
  /** The tab row; null without the engine's documents, or with one document open (the row shows only with two or
   * more: docs/ux/ui-plan-2026-11.md 1.1). */
  bar: JSX.Element | null;
  /** With one document open: its "+" (new tab, open a project in a tab) for the top bar, beside the name. */
  addButton: JSX.Element | null;
}

export function useDocTabs(p: DocTabsProps): DocTabsUi {
  const t = useT();
  const [list, setList] = useState<DocumentList | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const [closing, setClosing] = useState<OpenDocument | null>(null);
  const [adding, setAdding] = useState<{ x: number; y: number } | null>(null);
  const lastActive = useRef<string | null>(null);
  const live = useRef(p);
  live.current = p;

  const load = useCallback(async () => {
    try {
      const l = await wsApi.documents();
      if (!l) {
        setUnsupported(true);
        return;
      }
      setList(l);
      if (lastActive.current !== null && l.active !== lastActive.current) live.current.onSwitched();
      lastActive.current = l.active;
    } catch (e) {
      if (e instanceof ApiError && (e.status === 404 || e.status === 405)) setUnsupported(true);
    }
  }, []);

  useEffect(() => {
    void load();
    return onEngineEvent((ev) => {
      if (ev.type === "documents_changed" || ev.type === "project_saved") void load();
    });
  }, [load]);

  const after = useCallback(async () => {
    await load();
    await live.current.refresh();
  }, [load]);

  const activate = (id: string) => {
    if (id === list?.active) return;
    wsApi
      .activate(id)
      .then(after)
      .catch(live.current.report);
  };
  const newTab = useCallback(() => {
    wsApi
      .openDocument()
      .then(after)
      .catch(live.current.report);
  }, [after]);
  const openInTab = useCallback(
    (project: string) => {
      wsApi
        .openDocument(project)
        .then((r) => {
          if (r?.existing) live.current.notify(t("tabs.alreadyOpen", { name: project }));
          return after();
        })
        .catch(live.current.report);
    },
    [after, t],
  );
  const close = (d: OpenDocument, discard: boolean) => {
    setClosing(null);
    wsApi
      .close(d.id, discard)
      .then(after)
      .catch((e) => {
        // Unsaved changes the list did not know about yet: ask.
        if (e instanceof ApiError && e.status === 409 && !discard && (list?.documents.length ?? 0) > 1) setClosing({ ...d, dirty: true });
        else live.current.report(e);
      });
  };
  const askClose = (d: OpenDocument) => {
    const dirty = d.active ? (p.dirty ?? d.dirty) : d.dirty;
    if (dirty) setClosing(d);
    else close(d, false);
  };

  const supported = !unsupported && !!list;
  const api_ = { newTab, openInTab, supported };
  if (!supported || !list) return { api: api_, bar: null, addButton: null };
  const many = list.documents.length > 1;
  const addMenu = adding && <AddMenu x={adding.x} y={adding.y} onNew={newTab} onOpen={openInTab} onClose={() => setAdding(null)} />;
  if (!many && !closing) {
    const addButton = (
      <>
        <button className="doc-tab-add topbar-add" title={t("tabs.add")} aria-label={t("tabs.add")} onClick={(e) => setAdding({ x: e.clientX, y: e.clientY })}>
          <Icon name="plus" size={14} />
        </button>
        {addMenu}
      </>
    );
    return { api: api_, bar: null, addButton };
  }
  const bar = (
    <div className="doc-tabs" role="tablist" aria-label={t("tabs.label")}>
      {list.documents.map((d) => {
        const dirty = d.active ? (p.dirty ?? d.dirty) : d.dirty;
        return (
          <div key={d.id} className={`doc-tab${d.active ? " active" : ""}`}>
            <button
              role="tab"
              aria-selected={d.active}
              className="doc-tab-btn"
              title={d.project ? t("tabs.projectTitle", { name: d.project }) : t("tabs.unsavedTitle")}
              onClick={() => activate(d.id)}
              onAuxClick={(e) => {
                // Middle-click closes, as browser tabs do.
                if (e.button === 1 && many) askClose(d);
              }}
            >
              <span className="doc-tab-name">{d.title}</span>
              {dirty && <span className="doc-tab-dirty" aria-label={t("tabs.unsaved")} title={t("tabs.unsaved")} />}
            </button>
            {many && (
              <button className="doc-tab-close" title={t("tabs.close", { name: d.title })} aria-label={t("tabs.close", { name: d.title })} onClick={() => askClose(d)}>
                <Icon name="close" size={12} />
              </button>
            )}
          </div>
        );
      })}
      <button className="doc-tab-add" title={t("tabs.add")} aria-label={t("tabs.add")} onClick={(e) => setAdding({ x: e.clientX, y: e.clientY })}>
        <Icon name="plus" size={14} />
      </button>
      {addMenu}
      {closing && (
        <WsModal
          title={t("tabs.closeTitle", { name: closing.title })}
          onClose={() => setClosing(null)}
          footer={
            <>
              <button className="btn" onClick={() => setClosing(null)}>
                {t("common.cancel")}
              </button>
              {!closing.active && (
                <button
                  className="btn"
                  onClick={() => {
                    const d = closing;
                    setClosing(null);
                    activate(d.id);
                  }}
                >
                  {t("tabs.showIt")}
                </button>
              )}
              <button className="btn btn-warn" onClick={() => close(closing, true)}>
                {t("tabs.discardClose")}
              </button>
            </>
          }
        >
          <p>{t("tabs.closeText", { name: closing.title })}</p>
          {neighbourAfterClose(list, closing.id) && closing.active && <p className="hint">{t("tabs.closeNext")}</p>}
        </WsModal>
      )}
    </div>
  );
  return { api: api_, bar, addButton: null };
}

/** "+": a new empty document, or a saved project opened in a new tab. */
function AddMenu(p: { x: number; y: number; onNew: () => void; onOpen: (name: string) => void; onClose: () => void }) {
  const t = useT();
  const [projects, setProjects] = useState<ProjectEntry[] | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const { onClose } = p;
  useEffect(() => {
    let alive = true;
    api
      .projects()
      .then((r) => alive && setProjects(projectList(r)))
      .catch(() => alive && setProjects([]));
    return () => {
      alive = false;
    };
  }, []);
  useEffect(() => {
    const down = (e: PointerEvent) => ref.current && !ref.current.contains(e.target as Node) && onClose();
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("pointerdown", down, true);
    window.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", down, true);
      window.removeEventListener("keydown", key);
    };
  }, [onClose]);
  const left = Math.min(p.x, (typeof window === "undefined" ? 1200 : window.innerWidth) - 260);
  return (
    <div ref={ref} className="menu ctx-menu doc-tab-menu" role="menu" style={{ left: Math.max(4, left), top: p.y + 8 }}>
      <button
        role="menuitem"
        className="menu-item"
        onClick={() => {
          onClose();
          p.onNew();
        }}
      >
        <span>{t("tabs.new")}</span>
      </button>
      <div className="menu-sep" role="separator" />
      <div className="menu-label">{t("tabs.openProject")}</div>
      {projects === null && <div className="menu-item muted">{t("common.loading")}</div>}
      {projects?.length === 0 && <div className="menu-item muted">{t("tabs.noProjects")}</div>}
      {projects?.slice(0, 20).map((pr) => (
        <button
          key={pr.name}
          role="menuitem"
          className="menu-item"
          onClick={() => {
            onClose();
            p.onOpen(pr.name);
          }}
        >
          <span>{pr.name}</span>
        </button>
      ))}
    </div>
  );
}
