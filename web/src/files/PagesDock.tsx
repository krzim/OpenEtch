// LightBurn 2.2's Pages window as a dock: one row per page (name and object count; the active page highlighted),
// click to switch (`page.activate`), double-click or the context menu to rename, Add / Duplicate / Delete buttons,
// move up / down and drag to reorder (`page.move`). Layers and cut settings are shared by every page; jobs, previews
// and every shape command work on the active page. "Move selection to page" is in the canvas context menu.
// Touch: a long-press on a row is its right-click; rows have Rename (pencil) and ⋯ buttons and a reorder grip that
// works with a finger (pointer events, touch/reorder.ts, instead of HTML5 drag-and-drop).

import { useRef, useState } from "react";
import { useLongPress } from "../touch/useLongPress.ts";
import { useReorder } from "../touch/reorder.ts";
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
  const listRef = useRef<HTMLUListElement>(null);
  const go = (inv: Invocation) => void p.run(inv).catch(p.report);
  const reorder = useReorder(listRef, rows.length, (from, to) => go({ id: PAGE_CMD.move, params: { ...ref(rows[from]), to_index: to } }));
  const longPress = useLongPress((x, y, el) => {
    const i = Number(el.getAttribute("data-row"));
    if (Number.isInteger(i)) setMenu({ x, y, row: i });
  });
  if (!p.has(PAGE_CMD.add) || !p.doc) return null;
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
      <ul className="pages-list" role="listbox" aria-label={t("files.pages")} ref={listRef}>
        {rows.map((r, i) => (
          <li
            key={r.id ?? "single"}
            role="option"
            aria-selected={r.active}
            data-reorder-row=""
            data-row={i}
            className={`pages-row${r.active ? " active" : ""}${reorder.dropClass(i)}${reorder.dragging === i ? " dragging" : ""}`}
            {...longPress}
            onClick={() => activate(r)}
            onDoubleClick={() => setRename({ row: r, text: r.name })}
          >
            {rows.length > 1 && (
              <button type="button" className="row-grip" title={t("touch.reorder")} aria-label={t("touch.reorder")} onClick={(e) => e.stopPropagation()} {...reorder.grip(i)}>
                <Icon name="grip" size={14} />
              </button>
            )}
            <span className="pages-thumb" aria-hidden="true">
              {i + 1}
            </span>
            <span className="pages-name">{r.name}</span>
            <span className="pages-meta">{t("files.pageObjects", { n: r.objects })}</span>
            <button
              type="button"
              className="icon-btn small row-rename"
              title={t("touch.rename")}
              aria-label={t("touch.rename")}
              onClick={(e) => {
                e.stopPropagation();
                setRename({ row: r, text: r.name });
              }}
            >
              <Icon name="pencil" size={16} />
            </button>
            <button
              type="button"
              className="icon-btn small row-more"
              // A double-tap on ⋯ is two presses of ⋯, not the row's double-click (which opens its settings).
              onDoubleClick={(e) => e.stopPropagation()}
              title={t("touch.rowMenu")}
              aria-label={t("touch.rowMenu")}
              onClick={(e) => {
                e.stopPropagation();
                const b = e.currentTarget.getBoundingClientRect();
                setMenu({ x: b.left, y: b.bottom, row: i });
              }}
            >
              <Icon name="more" size={16} />
            </button>
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
