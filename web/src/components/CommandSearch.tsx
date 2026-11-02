// Command search (S, or the toolbar's search button): type to find any command, Enter runs it. Lists every
// hotkeys.ts action (run exactly as its key would run it, commands.ts) and the menu items other parts of the app
// publish (File, Edit, Tools, Modify, Arrange, Laser Tools, the ribbon's own buttons), each with its shortcut and its
// action id. Request run / Start is deliberately not listed: a burn starts only from Start or the foot switch.

import { useEffect, useId, useMemo, useRef, useState } from "react";
import { ACTIONS, hintFor } from "../workspace/hotkeys.ts";
import { usePrefs, useT, useTD } from "../workspace/usePrefs.ts";
import { canRunAction, runHotkeyAction } from "./commands.ts";
import { mergeCommands, pushRecent, searchCommands, ALIASES, type Command } from "./commandSearch.ts";
import { Dialog } from "./Dialog.tsx";
import { formatChord } from "./formatChord.ts";
import type { MenuItem } from "./Menu.tsx";
import { usePublished } from "./Ribbon.tsx";
import "./ribbon.css";

/** Published menus searched (Ribbon.tsx usePublish): key -> where it lives. */
export const SEARCHED_MENUS = ["menu.file", "menu.edit", "menu.tools", "menu.modify", "ribbon.arrange", "menu.laserTools", "menu.webTools", "menu.ribbon"] as const;

/** Actions never offered here. */
const HIDDEN = new Set(["laser.requestRun", "app.commandSearch"]);

const RECENTS_KEY = "openetch.commandSearch.recents";

function loadRecents(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(RECENTS_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, 8) : [];
  } catch {
    return [];
  }
}

function saveRecents(r: string[]): void {
  try {
    localStorage.setItem(RECENTS_KEY, JSON.stringify(r));
  } catch {
    // Storage blocked: recents last for this page only.
  }
}

/** A published menu as commands: submenus flattened into paths ("File › Export"). */
export function menuCommands(items: readonly MenuItem[] | undefined, path: string, hotkeys: Readonly<Record<string, string[]>>): Command[] {
  const out: Command[] = [];
  for (const it of items ?? []) {
    if (it.sep || it.content) continue;
    if (it.submenu?.length) {
      out.push(...menuCommands(it.submenu, `${path} › ${it.label}`, hotkeys));
      continue;
    }
    if (!it.action) continue;
    const action = it.action;
    out.push({
      key: `menu:${path}/${it.label}`,
      label: it.label,
      path,
      actionId: it.actionId,
      hint: it.hint !== undefined ? formatChord(it.hint) : it.actionId ? formatChord(hintFor(it.actionId, hotkeys)) : undefined,
      aliases: it.actionId ? ALIASES[it.actionId] : undefined,
      disabled: it.disabled,
      run: action,
    });
  }
  return out;
}

export function CommandSearch(p: { onClose: () => void; onEngineConsole: () => void }) {
  const t = useT();
  const td = useTD();
  const { hotkeys } = usePrefs();
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const [recents, setRecents] = useState(loadRecents);
  const listId = useId();
  const list = useRef<HTMLUListElement>(null);
  const menus = {
    "menu.file": usePublished<{ label: string; items: MenuItem[] }>("menu.file"),
    "menu.edit": usePublished<{ label: string; items: MenuItem[] }>("menu.edit"),
    "menu.tools": usePublished<{ label: string; items: MenuItem[] }>("menu.tools"),
    "menu.modify": usePublished<{ label: string; items: MenuItem[] }>("menu.modify"),
    "ribbon.arrange": usePublished<{ label: string; items: MenuItem[] }>("ribbon.arrangeMenu"),
    "menu.laserTools": usePublished<{ label: string; items: MenuItem[] }>("menu.laserTools"),
    "menu.webTools": usePublished<{ label: string; items: MenuItem[] }>("menu.webTools"),
    "menu.ribbon": usePublished<{ label: string; items: MenuItem[] }>("menu.ribbon"),
  };

  const all = useMemo(() => {
    const actions: Command[] = ACTIONS.filter((a) => !a.fixed && !HIDDEN.has(a.id)).map((a) => ({
      key: a.id,
      label: td(`hk.${a.id}`, a.label),
      path: td(`hk.group.${a.group}`, a.group[0].toUpperCase() + a.group.slice(1)),
      actionId: a.id,
      hint: formatChord(hintFor(a.id, hotkeys)) || undefined,
      aliases: ALIASES[a.id],
      disabled: !canRunAction(a.id),
      run: () => runHotkeyAction(a.id),
    }));
    const menu: Command[] = [];
    for (const m of Object.values(menus)) if (m) menu.push(...menuCommands(m.items, m.label, hotkeys));
    menu.push({ key: "dev.engineConsole", label: t("rb.engineCommands"), path: t("rb.developer"), aliases: ["json", "mcp", "api", "commands"], run: p.onEngineConsole });
    return mergeCommands(actions, menu);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [td, t, hotkeys, ...Object.values(menus)]);

  const found = useMemo(() => searchCommands(all, q, recents), [all, q, recents]);
  const act = Math.min(active, Math.max(0, found.length - 1));

  useEffect(() => setActive(0), [q]);
  useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-i="${act}"]`)?.scrollIntoView({ block: "nearest" });
  }, [act]);

  const run = (c: Command | undefined) => {
    if (!c || c.disabled) return;
    const next = pushRecent(recents, c.key);
    setRecents(next);
    saveRecents(next);
    p.onClose();
    // After the dialog has closed and given the focus back, so the command sees the app as it was.
    window.setTimeout(c.run, 0);
  };

  return (
    <Dialog title={t("rb.searchTitle")} onClose={p.onClose} size="md" dismissable>
      <div className="cs">
        <input
          type="search"
          className="cs-input"
          autoFocus
          placeholder={t("rb.searchPlaceholder")}
          aria-label={t("rb.searchTitle")}
          role="combobox"
          aria-expanded="true"
          aria-controls={listId}
          aria-activedescendant={found.length ? `${listId}-${act}` : undefined}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") setActive((i) => Math.min(found.length - 1, i + 1));
            else if (e.key === "ArrowUp") setActive((i) => Math.max(0, i - 1));
            else if (e.key === "PageDown") setActive((i) => Math.min(found.length - 1, i + 8));
            else if (e.key === "PageUp") setActive((i) => Math.max(0, i - 8));
            else if (e.key === "Enter") run(found[act]);
            else return;
            e.preventDefault();
          }}
        />
        {!q && recents.length > 0 && <div className="cs-section">{t("rb.recent")}</div>}
        <ul ref={list} id={listId} className="cs-list" role="listbox" aria-label={t("rb.searchTitle")}>
          {found.map((c, i) => (
            <li
              key={c.key}
              id={`${listId}-${i}`}
              data-i={i}
              role="option"
              aria-selected={i === act}
              aria-disabled={c.disabled || undefined}
              className={`cs-item${i === act ? " active" : ""}${c.disabled ? " disabled" : ""}`}
              onPointerMove={() => i !== act && setActive(i)}
              onClick={() => run(c)}
            >
              <span className="cs-label">{c.label}</span>
              {c.path && <span className="cs-path">{c.path}</span>}
              {c.actionId && <span className="cs-id">{c.actionId}</span>}
              {c.hint && <kbd className="cs-hint">{c.hint}</kbd>}
            </li>
          ))}
          {!found.length && <li className="cs-empty">{t("rb.noMatch", { q })}</li>}
        </ul>
        <div className="cs-foot">{t("rb.searchKeys")}</div>
      </div>
    </Dialog>
  );
}
