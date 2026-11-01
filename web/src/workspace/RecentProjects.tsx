// File menu > Recent projects (LightBurn's File > Recent Projects): the engine's saved projects, most recently
// modified first, fetched when the menu opens.

import { useEffect, useState } from "react";
import { api } from "../api/client.ts";
import { formatModified, projectList } from "../api/project.ts";
import type { ProjectEntry } from "../api/types.ts";
import { useT } from "./usePrefs.ts";

export const RECENT_COUNT = 8;

/** The `n` most recently modified projects (projectList sorts newest first). */
export function recentProjects(res: unknown, n = RECENT_COUNT): ProjectEntry[] {
  return projectList(res).slice(0, n);
}

export function RecentProjects(p: { current: string | null; disabled?: boolean; onOpen: (name: string) => void }) {
  const t = useT();
  const [list, setList] = useState<ProjectEntry[] | null>(null);
  useEffect(() => {
    let live = true;
    api
      .projects()
      .then((r) => live && setList(recentProjects(r)))
      .catch(() => live && setList([]));
    return () => {
      live = false;
    };
  }, []);
  return (
    <div role="group" aria-label={t("file.recent")}>
      <div className="menu-label">{t("file.recent")}</div>
      {list === null && <div className="menu-item muted">{t("common.loading")}</div>}
      {list?.length === 0 && <div className="menu-item muted">{t("file.noRecent")}</div>}
      {list?.map((pr) => (
        <button
          key={pr.name}
          role="menuitem"
          className={`menu-item recent-item${pr.name === p.current ? " current" : ""}`}
          disabled={p.disabled}
          title={t("file.openRecent", { name: pr.name })}
          onClick={() => p.onOpen(pr.name)}
        >
          <span className="recent-name">{pr.name}</span>
          <span className="menu-hint">{formatModified(pr.modified_unix_s)}</span>
        </button>
      ))}
    </div>
  );
}
