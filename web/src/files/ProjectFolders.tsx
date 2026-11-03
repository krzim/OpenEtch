// Folders in the Save / Open dialogs and the Project locations setting (UX-013). LightBurn saves anywhere through the
// OS dialogs; OpenEtch is a server, so projects live in folders under the projects directory and under locations
// (further directories mounted as top folders). The tree comes from GET /api/projects/folders plus the folders of the
// listed projects; New folder sends POST /api/projects/folders; the Project locations setting edits the saved
// locations with PUT /api/projects/locations (UI only; the command line's --projects-mount ones stay).

import { useCallback, useEffect, useMemo, useState } from "react";
import { useT } from "../workspace/usePrefs.ts";
import { filesApi } from "./filesApi.ts";
import {
  folderRows,
  hasChildren,
  joinProject,
  locationsProblem,
  newFolderProblem,
  parseFolders,
  parseLocations,
  visibleRows,
  type LocationDraft,
  type ProjectLocation,
} from "./folders.ts";
import { errorText } from "../api/errorText.ts";
import { HelpLink } from "../guide/HelpLink.tsx";

function errText(e: unknown): string {
  if (e && typeof e === "object" && "detail" in e && typeof (e as { detail?: unknown }).detail === "string") return (e as { detail: string }).detail;
  return errorText(e);
}

/** The engine's folders and locations; `null` while loading (and for an engine without the routes: empty). */
export function useProjectFolders(): { folders: string[] | null; locations: ProjectLocation[]; reload: () => void } {
  const [folders, setFolders] = useState<string[] | null>(null);
  const [locations, setLocations] = useState<ProjectLocation[]>([]);
  const [n, setN] = useState(0);
  useEffect(() => {
    let alive = true;
    filesApi
      .projectFolders()
      .then((r) => {
        if (!alive) return;
        setFolders(parseFolders(r));
        setLocations(parseLocations(r));
      })
      .catch(() => alive && setFolders([]));
    return () => {
      alive = false;
    };
  }, [n]);
  return { folders, locations, reload: useCallback(() => setN((x) => x + 1), []) };
}

/**
 * The folder tree: the projects directory, its folders and the locations, expandable; a click picks a folder. New
 * folder makes one under the picked folder.
 */
export function FolderTree(p: {
  folders: string[];
  locations: ProjectLocation[];
  projectNames: string[];
  selected: string;
  onSelect: (folder: string) => void;
  onCreated: (folder: string) => void;
  disabled?: boolean;
}) {
  const t = useT();
  const rows = useMemo(() => folderRows(p.folders, p.projectNames, p.locations), [p.folders, p.projectNames, p.locations]);
  // The picked folder's parents start expanded.
  const [open, setOpen] = useState<Set<string>>(() => {
    const segs = p.selected.split("/").filter(Boolean);
    return new Set(segs.map((_, i) => segs.slice(0, i + 1).join("/")));
  });
  const [naming, setNaming] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const shown = visibleRows(rows, open);
  const toggle = (path: string) =>
    setOpen((o) => {
      const n = new Set(o);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });
  const problem = naming !== null ? newFolderProblem(p.selected, naming) : null;
  const create = async () => {
    if (naming === null || problem) return;
    const path = joinProject(p.selected, naming.trim());
    setBusy(true);
    setError(null);
    try {
      await filesApi.createProjectFolder(path);
      setNaming(null);
      setOpen((o) => new Set([...o, p.selected]));
      p.onCreated(path);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="folder-tree-wrap">
      <div className="folder-tree" role="tree" aria-label={t("fold.tree")}>
        {shown.map((r) => {
          const kids = hasChildren(rows, r);
          const isOpen = r.depth === 0 || open.has(r.path);
          return (
            <div key={r.path || "/"} className="folder-row" style={{ paddingLeft: `${r.depth * 14 + 2}px` }}>
              {r.depth > 0 && kids ? (
                <button className="folder-twisty" aria-label={isOpen ? t("fold.collapse") : t("fold.expand")} onClick={() => toggle(r.path)}>
                  {isOpen ? "▾" : "▸"}
                </button>
              ) : (
                <span className="folder-twisty" />
              )}
              <button
                role="treeitem"
                aria-selected={p.selected.toLowerCase() === r.path.toLowerCase()}
                aria-expanded={kids ? isOpen : undefined}
                className={`folder-name${p.selected.toLowerCase() === r.path.toLowerCase() ? " active" : ""}${r.unreachable ? " unreachable" : ""}`}
                disabled={p.disabled}
                title={r.location ? (r.unreachable ? t("fold.locationDown") : t("fold.location")) : undefined}
                onClick={() => p.onSelect(r.path)}
                onDoubleClick={() => kids && toggle(r.path)}
              >
                {r.depth === 0 ? t("fold.root") : r.location ? `${r.name} ⛁` : r.name}
              </button>
            </div>
          );
        })}
      </div>
      {naming === null ? (
        <button className="btn small" disabled={p.disabled || busy} onClick={() => (setNaming(""), setError(null))} title={t("fold.newTitle")}>
          {t("fold.new")}
        </button>
      ) : (
        <div className="folder-new">
          <input
            type="text"
            autoFocus
            value={naming}
            maxLength={100}
            placeholder={t("fold.newPlaceholder")}
            aria-label={t("fold.newName")}
            onChange={(e) => setNaming(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                e.stopPropagation();
                void create();
              } else if (e.key === "Escape") {
                e.stopPropagation();
                setNaming(null);
              }
            }}
          />
          <button className="btn small btn-primary" disabled={busy || !!problem} onClick={() => void create()}>
            {t("fold.create")}
          </button>
          <button className="btn small" onClick={() => setNaming(null)}>
            {t("common.cancel")}
          </button>
        </div>
      )}
      {naming !== null && naming.trim() !== "" && problem && <p className="form-error">{problem}</p>}
      {error && <p className="form-error">{error}</p>}
    </div>
  );
}

/**
 * Preferences > Project locations: further directories (a NAS, a synced cloud folder) mounted as top folders of the
 * Save / Open dialogs. The command line's locations are listed and stay; the saved ones can be added, edited and
 * removed, then saved at once (the engine checks that each path is an existing directory).
 */
export function ProjectLocationsSection() {
  const t = useT();
  const [list, setList] = useState<ProjectLocation[] | null>(null);
  const [drafts, setDrafts] = useState<LocationDraft[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [unsupported, setUnsupported] = useState(false);
  const take = (raw: unknown) => {
    const l = parseLocations(raw);
    setList(l);
    setDrafts(l.filter((x) => x.source !== "cli").map((x) => ({ label: x.label, path: x.path })));
  };
  useEffect(() => {
    filesApi
      .projectLocations()
      .then(take)
      .catch(() => setUnsupported(true));
  }, []);
  if (unsupported) return null;
  const fixed = (list ?? []).filter((l) => l.source === "cli");
  const problem = locationsProblem(drafts, fixed);
  const before = JSON.stringify((list ?? []).filter((x) => x.source !== "cli").map((x) => ({ label: x.label, path: x.path })));
  const changed = JSON.stringify(drafts.map((d) => ({ label: d.label.trim(), path: d.path.trim() }))) !== before;
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      take(await filesApi.setProjectLocations(drafts.map((d) => ({ label: d.label.trim(), path: d.path.trim() }))));
      setSaved(true);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const reach = (label: string) => list?.find((l) => l.label === label);
  return (
    <section className="prefs-section prefs-wide">
      <h4>{t("fold.locations")} <HelpLink to="settings#project-locations" topic={t("fold.locations")} /></h4>
      <p className="hint">{t("fold.locationsHint")}</p>
      {list === null && <p className="muted">{t("common.loading")}</p>}
      {fixed.map((l) => (
        <div key={l.label} className="loc-row">
          <input type="text" value={l.label} disabled aria-label={t("fold.label")} />
          <input type="text" value={l.path} disabled aria-label={t("fold.path")} />
          <span className="muted">{l.reachable ? t("fold.fromCli") : t("fold.unreachable")}</span>
        </div>
      ))}
      {drafts.map((d, i) => {
        const r = reach(d.label);
        return (
          <div key={i} className="loc-row">
            <input
              type="text"
              value={d.label}
              placeholder={t("fold.labelPlaceholder")}
              aria-label={t("fold.label")}
              onChange={(e) => (setSaved(false), setDrafts((ds) => ds.map((x, j) => (j === i ? { ...x, label: e.target.value } : x))))}
            />
            <input
              type="text"
              value={d.path}
              placeholder={t("fold.pathPlaceholder")}
              aria-label={t("fold.path")}
              onChange={(e) => (setSaved(false), setDrafts((ds) => ds.map((x, j) => (j === i ? { ...x, path: e.target.value } : x))))}
            />
            {r && !r.reachable && <span className="warn-text">{t("fold.unreachable")}</span>}
            <button className="btn small" onClick={() => (setSaved(false), setDrafts((ds) => ds.filter((_, j) => j !== i)))}>
              {t("fold.remove")}
            </button>
          </div>
        );
      })}
      <div className="loc-actions">
        <button className="btn small" disabled={list === null} onClick={() => (setSaved(false), setDrafts((ds) => [...ds, { label: "", path: "" }]))}>
          {t("fold.add")}
        </button>
        <button className="btn small btn-primary" disabled={busy || !changed || !!problem} onClick={() => void save()}>
          {t("fold.save")}
        </button>
        {saved && !changed && <span className="muted">{t("fold.saved")}</span>}
      </div>
      {changed && problem && <p className="form-error">{problem}</p>}
      {error && <p className="form-error">{error}</p>}
    </section>
  );
}
