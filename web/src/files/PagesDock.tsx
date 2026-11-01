// LightBurn 2.2's Pages window as a dock: one row per page (name and object count; the active page highlighted),
// click to switch (`page.activate`), double-click or the context menu to rename, Add / Duplicate / Delete buttons,
// move up / down and drag to reorder (`page.move`). Layers and cut settings are shared by every page; jobs, previews
// and every shape command work on the active page. "Move selection to page" is in the canvas context menu.

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument } from "../api/types.ts";
import { ContextMenu, type MenuItem } from "../components/ContextMenu.tsx";
import { Icon, Modal } from "../components/ui.tsx";
import { Dock } from "../workspace/Dock.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { nextPageName, pageMoveTarget, pagesOf, type PageRow } from "./fileFormats.ts";

export const PAGE_CMD = {
  add: "page.add",
  duplicate: "page.duplicate",
  remove: "page.delete",
  rename: "page.rename",
  activate: "page.activate",
  move: "page.move",
  send: "page.send_objects",
} as const;

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

/** A page reference for the commands: its id (a document without pages has none, and the commands create one). */
function ref(r: PageRow): Record<string, unknown> {
  return r.id ? { page: r.id } : {};
}

export function PagesDock(p: { doc: GDocument | null; has: (id: string) => boolean; run: Run; report: (e: unknown) => void }) {
  const t = useT();
  const rows = pagesOf(p.doc);
  const [menu, setMenu] = useState<{ x: number; y: number; row: number } | null>(null);
  const [rename, setRename] = useState<{ row: PageRow; text: string } | null>(null);
  const [drag, setDrag] = useState<number | null>(null);
  if (!p.has(PAGE_CMD.add) || !p.doc) return null;
  const go = (inv: Invocation) => void p.run(inv).catch(p.report);
  const add = () => go({ id: PAGE_CMD.add, params: { name: nextPageName(rows) } });
  const dup = (r: PageRow) => go({ id: PAGE_CMD.duplicate, params: ref(r) });
  const del = (r: PageRow) => go({ id: PAGE_CMD.remove, params: ref(r) });
  const move = (i: number, to: number) => go({ id: PAGE_CMD.move, params: { ...ref(rows[i]), to_index: to } });
  const activate = (r: PageRow) => !r.active && r.id && go({ id: PAGE_CMD.activate, params: { page: r.id } });
  const active = rows.find((r) => r.active) ?? rows[0];
  const menuItems = (i: number): MenuItem[] => {
    const r = rows[i];
    const up = pageMoveTarget(rows.length, i, -1);
    const down = pageMoveTarget(rows.length, i, 1);
    return [
      { label: t("files.pageAdd"), action: add },
      { label: t("files.pageDuplicate"), action: () => dup(r) },
      { label: t("files.pageDelete"), disabled: rows.length < 2, action: () => del(r) },
      { label: t("files.pageRename"), action: () => setRename({ row: r, text: r.name }) },
      { sep: true },
      { label: t("files.pageMoveUp"), disabled: up === null, action: () => up !== null && move(i, up) },
      { label: t("files.pageMoveDown"), disabled: down === null, action: () => down !== null && move(i, down) },
    ];
  };
  const submitRename = () => {
    if (!rename) return;
    const name = rename.text.trim();
    if (name && name !== rename.row.name) go({ id: PAGE_CMD.rename, params: { ...ref(rename.row), name } });
    setRename(null);
  };
  return (
    <Dock
      id="pages"
      title={rows.length > 1 ? t("files.pagesN", { n: rows.length }) : t("files.pages")}
      defaultCollapsed={rows.length < 2}
      actions={
        <>
          <button className="icon-btn small" title={t("files.pageAdd")} aria-label={t("files.pageAdd")} onClick={add}>
            <Icon name="plus" size={16} />
          </button>
          <button className="icon-btn small" title={t("files.pageDuplicate")} aria-label={t("files.pageDuplicate")} onClick={() => dup(active)}>
            <Icon name="array" size={16} />
          </button>
          <button className="icon-btn small" title={t("files.pageDelete")} aria-label={t("files.pageDelete")} disabled={rows.length < 2} onClick={() => del(active)}>
            <Icon name="delete" size={16} />
          </button>
        </>
      }
    >
      <ul className="pages-list" role="listbox" aria-label={t("files.pages")}>
        {rows.map((r, i) => (
          <li
            key={r.id ?? "single"}
            role="option"
            aria-selected={r.active}
            className={`pages-row${r.active ? " active" : ""}${drag !== null && drag !== i ? " drop-target" : ""}`}
            draggable={rows.length > 1}
            onDragStart={(e) => {
              setDrag(i);
              e.dataTransfer.effectAllowed = "move";
              e.dataTransfer.setData("text/x-openetch-page", String(i));
            }}
            onDragOver={(e) => drag !== null && e.preventDefault()}
            onDrop={(e) => {
              e.preventDefault();
              if (drag !== null && drag !== i) move(drag, i);
              setDrag(null);
            }}
            onDragEnd={() => setDrag(null)}
            onClick={() => activate(r)}
            onDoubleClick={() => setRename({ row: r, text: r.name })}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({ x: e.clientX, y: e.clientY, row: i });
            }}
          >
            <span className="pages-thumb" aria-hidden="true">
              {i + 1}
            </span>
            <span className="pages-name">{r.name}</span>
            <span className="pages-meta">{t("files.pageObjects", { n: r.objects })}</span>
          </li>
        ))}
      </ul>
      <p className="hint">{t("files.pagesHint")}</p>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.row)} onClose={() => setMenu(null)} />}
      {rename && (
        <Modal
          title={t("files.pageRename")}
          onClose={() => setRename(null)}
          onSubmit={submitRename}
          footer={
            <>
              <button className="btn" onClick={() => setRename(null)}>
                {t("common.cancel")}
              </button>
              <button className="btn btn-primary" disabled={!rename.text.trim()} onClick={submitRename}>
                {t("files.ok")}
              </button>
            </>
          }
        >
          <label className="field">
            <span className="field-label">{t("files.pageName")}</span>
            <span className="field-input">
              <input type="text" autoFocus maxLength={100} value={rename.text} onChange={(e) => setRename({ ...rename, text: e.target.value })} />
            </span>
          </label>
        </Modal>
      )}
    </Dock>
  );
}

/** Context menu entries: "Move selection to page <name>" for every other page. */
export function sendToPageItems(doc: GDocument | null, ids: string[], has: (id: string) => boolean, run: Run, report: (e: unknown) => void, label: (name: string) => string): MenuItem[] {
  const rows = pagesOf(doc);
  if (!has(PAGE_CMD.send) || ids.length === 0 || rows.length < 2) return [];
  return rows
    .filter((r) => !r.active && r.id)
    .map((r) => ({ label: label(r.name), action: () => void run({ id: PAGE_CMD.send, params: { ids, page: r.id as string } }).catch(report) }));
}
