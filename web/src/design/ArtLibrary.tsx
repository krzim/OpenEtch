// The Art Library dock (LightBurn's Art Library window, docs "Art Library"): libraries of reusable artwork kept by the
// engine in its projects folder, so every browser and agent sees the same ones (crates/engine/src/artlib.rs). Pick a
// library (New, Load .oeart, Download, Rename, Delete), see its items as thumbnails (searchable), Add Selection,
// Import a file as an item, drag an item onto the canvas (it lands centred on the drop point) or double-click /
// "Add to project" (centred on the field), and rename, delete, copy or move items (right-click). LightBurn's .lbart
// libraries load (Load), import into the current library (Import) and download (Download .lbart) on engines that list
// "lbart" among their formats. Follows `art_library_changed`.
// Touch (docs/ux/ui-plan-2026-11.md 1.2): a long-press on an item is its right-click, the selected item has a visible
// Insert and ⋯ button (no double-click needed), items are not HTML5-draggable on coarse pointers (an iPad long-press
// would start a system drag), and the Load picker has no `accept` on iOS (checked after the pick, touch/filePick.ts).

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { onEngineEvent } from "../api/events.ts";
import type { Vec2 } from "../api/types.ts";
import { ContextMenu, type MenuItem } from "../components/ContextMenu.tsx";
import { Icon, Modal } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { setVisible, updateLayout } from "../workspace/layout.ts";
import { updatePrefs } from "../workspace/prefs.ts";
import { Dock } from "../workspace/Dock.tsx";
import { artApi, type ArtItem, type ArtLibraryRow } from "./designApi.ts";
import { artImportFormat, libraryNameProblem } from "./model.ts";
import { useLongPress } from "../touch/useLongPress.ts";
import { acceptLabel, acceptsFile, pickerAccept } from "../touch/filePick.ts";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { errorText } from "../api/errorText.ts";

const LOAD_ACCEPT = ".oeart,.lbart";

/** The drag payload type of an Art Library item (`{library, id}` JSON). */
export const ART_DRAG_TYPE = "application/x-openetch-art";
const SELECTED_KEY = "openetch.artLibrary";

export const ART_DOCK_ID = "artLibrary";

/** Show and expand the Art Library window (Window menu layout and the dock's collapsed state). */
export function openArtDock(): void {
  updateLayout((l) => setVisible(l, ART_DOCK_ID, true));
  updatePrefs((cur) => ({ docks: { ...cur.docks, [ART_DOCK_ID]: false } }));
}

type NameDlg = { kind: "new" | "rename" | "add" | "renameItem"; item?: ArtItem } | { kind: "copy"; item: ArtItem; move: boolean } | { kind: "delete" } | { kind: "deleteItem"; item: ArtItem };

export function ArtLibraryDock(p: {
  selection: string[];
  design: boolean;
  notify: (msg: string, kind?: "info" | "error") => void;
  /** Insert an item; `center` null for the field centre. Selects the result. */
  onInsert: (library: string, id: string, center: Vec2 | null) => void;
}) {
  const { notify } = p;
  const t = useT();
  const [libs, setLibs] = useState<ArtLibraryRow[] | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const [lbartOk, setLbartOk] = useState(false);
  const [selected, setSelectedRaw] = useState<string | null>(() => {
    try {
      return localStorage.getItem(SELECTED_KEY);
    } catch {
      return null;
    }
  });
  const [items, setItems] = useState<ArtItem[]>([]);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [dlg, setDlg] = useState<NameDlg | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; item: ArtItem } | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const loadRef = useRef<HTMLInputElement>(null);
  const importRef = useRef<HTMLInputElement>(null);
  const setSelected = (n: string | null) => {
    setSelectedRaw(n);
    try {
      if (n) localStorage.setItem(SELECTED_KEY, n);
      else localStorage.removeItem(SELECTED_KEY);
    } catch {
      // Storage unavailable: the choice lasts for this visit.
    }
  };
  const fail = useCallback((e: unknown) => notify(errorText(e), "error"), [notify]);

  const refreshList = useCallback(() => {
    artApi
      .list()
      .then((r) => {
        setUnsupported(false);
        setLibs(r.libraries ?? []);
        setLbartOk((r.formats ?? []).includes("lbart"));
      })
      .catch((e: unknown) => {
        if ((e as { status?: number }).status === 404) setUnsupported(true);
        else fail(e);
      });
  }, [fail]);
  const refreshItems = useCallback(
    (name: string | null) => {
      if (!name) {
        setItems([]);
        return;
      }
      artApi
        .view(name)
        .then((r) => setItems(r.items ?? []))
        .catch((e: unknown) => {
          setItems([]);
          if ((e as { status?: number }).status !== 404) fail(e);
        });
    },
    [fail],
  );
  useEffect(() => refreshList(), [refreshList]);
  // The chosen library, else the first one.
  const current = libs ? (libs.find((l) => l.name === selected)?.name ?? libs[0]?.name ?? null) : null;
  useEffect(() => refreshItems(current), [current, refreshItems]);
  const live = useRef({ current, refreshList, refreshItems });
  live.current = { current, refreshList, refreshItems };
  useEffect(
    () =>
      onEngineEvent((ev) => {
        if (ev.type !== "art_library_changed") return;
        const lib = typeof ev.library === "string" ? ev.library : null;
        if (lib === null) live.current.refreshList();
        if (lib === null || lib.toLowerCase() === live.current.current?.toLowerCase()) live.current.refreshItems(live.current.current);
        if (lib !== null) live.current.refreshList();
      }),
    [],
  );

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q ? items.filter((i) => i.name.toLowerCase().includes(q)) : items;
  }, [items, query]);

  const task = async (f: () => Promise<unknown>, ok?: string) => {
    setBusy(true);
    try {
      await f();
      if (ok) notify(ok);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
      refreshList();
      refreshItems(current);
    }
  };

  const loadFile = (file: File) => {
    if (/\.lbart$/i.test(file.name) && !lbartOk) {
      notify(t("art.noLbart"), "error");
      return;
    }
    void task(async () => {
      const r = await artApi.load(file);
      if (r.name) setSelected(r.name);
      for (const w of r.warnings ?? []) notify(w, "info");
    }, t("art.loaded", { name: file.name }));
  };
  const importFile = (file: File) => {
    if (!current) return;
    const f = artImportFormat(file.name);
    if ("error" in f || (f.format === "lbart" && !lbartOk)) {
      notify("error" in f ? f.error : t("art.noLbart"), "error");
      return;
    }
    void task(async () => {
      const r = await artApi.importFile(current, file, f.format);
      for (const w of r.warnings ?? []) notify(w, "info");
    }, t("art.addedFile", { name: file.name, library: current }));
  };
  const download = (format: "oeart" | "lbart") => {
    if (!current) return;
    void task(async () => {
      const blob = await artApi.download(current, format);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${current}.${format}`;
      a.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 10000);
    });
  };

  if (unsupported) return null;
  const names = (libs ?? []).map((l) => l.name);
  const itemMenu = (it: ArtItem): MenuItem[] => [
    { label: t("art.addToProject"), action: () => current && p.onInsert(current, it.id, null) },
    { sep: true },
    { label: t("art.renameItem"), action: () => setDlg({ kind: "renameItem", item: it }) },
    { label: t("art.copyTo"), disabled: names.length < 2, action: () => setDlg({ kind: "copy", item: it, move: false }) },
    { label: t("art.moveTo"), disabled: names.length < 2, action: () => setDlg({ kind: "copy", item: it, move: true }) },
    { sep: true },
    { label: t("art.delete"), action: () => setDlg({ kind: "deleteItem", item: it }) },
  ];

  return (
    <Dock help="art-library#the-art-library-window" id={ART_DOCK_ID} title={t("art.title")} defaultCollapsed>
      <div className="dz-art">
        <div className="dz-art-row">
          <select aria-label={t("art.library")} value={current ?? ""} onChange={(e) => setSelected(e.target.value || null)} disabled={!libs?.length}>
            {!libs?.length && <option value="">{t("art.noLibrary")}</option>}
            {(libs ?? []).map((l) => (
              <option key={l.name} value={l.name}>
                {l.name} ({l.items})
              </option>
            ))}
          </select>
          <button className="btn btn-small" onClick={() => setDlg({ kind: "new" })} disabled={busy} title={t("art.newTitle")}>
            {t("art.new")}
          </button>
          <button className="btn btn-small" onClick={() => loadRef.current?.click()} disabled={busy} title={t("art.loadTitle")}>
            {t("art.load")}
          </button>
        </div>
        {current && (
          <div className="dz-art-row">
            <button className="btn btn-small" onClick={() => setDlg({ kind: "add" })} disabled={busy || !p.selection.length || !p.design} title={t("art.addSelectionTitle")}>
              {t("art.addSelection")}
            </button>
            <button className="btn btn-small" onClick={() => importRef.current?.click()} disabled={busy} title={t("art.importTitle")}>
              {t("art.import")}
            </button>
            <button className="btn btn-small" onClick={() => download("oeart")} disabled={busy} title={t("art.downloadTitle")}>
              {t("art.download")}
            </button>
            {lbartOk && (
              <button className="btn btn-small" onClick={() => download("lbart")} disabled={busy} title={t("art.downloadLbartTitle")}>
                {t("art.downloadLbart")}
              </button>
            )}
            <button className="btn btn-small" onClick={() => setDlg({ kind: "rename" })} disabled={busy}>
              {t("art.rename")}
            </button>
            <button className="btn btn-small" onClick={() => setDlg({ kind: "delete" })} disabled={busy} title={t("art.deleteTitle")}>
              {t("art.delete")}
            </button>
          </div>
        )}
        {current && items.length > 6 && <input type="search" className="dz-art-search" placeholder={t("art.search")} value={query} onChange={(e) => setQuery(e.target.value)} />}
        {libs && !libs.length && <p className="hint">{t("art.noLibraries")}</p>}
        {current && !items.length && <p className="hint">{t("art.empty")}</p>}
        <div className="dz-art-grid" role="listbox" aria-label={t("art.items")}>
          {current &&
            shown.map((it) => (
              <ArtThumb
                key={it.id}
                library={current}
                item={it}
                active={active === it.id}
                onSelect={() => setActive(it.id)}
                onInsert={() => p.onInsert(current, it.id, null)}
                onMenu={(x, y) => {
                  setActive(it.id);
                  setMenu({ x, y, item: it });
                }}
              />
            ))}
        </div>
        {current && active && shown.some((it) => it.id === active) && (
          <div className="dz-art-row row-actions">
            <button className="btn btn-small" onClick={() => p.onInsert(current, active, null)}>
              {t("touch.insert")}
            </button>
            <button
              className="btn btn-small"
              title={t("touch.rowMenu")}
              aria-label={t("touch.rowMenu")}
              onClick={(e) => {
                const it = items.find((x) => x.id === active);
                const b = e.currentTarget.getBoundingClientRect();
                if (it) setMenu({ x: b.left, y: b.bottom, item: it });
              }}
            >
              <Icon name="more" size={16} />
            </button>
          </div>
        )}
        {current && items.length > 0 && <p className="hint">{t("art.hint")}</p>}
        <input
          ref={loadRef}
          type="file"
          accept={pickerAccept(LOAD_ACCEPT)}
          hidden
          onChange={(e) => {
            const f = e.target.files?.[0];
            e.target.value = "";
            if (f && !acceptsFile(f, LOAD_ACCEPT)) notify(t("touch.wrongFileType", { name: f.name, types: acceptLabel(LOAD_ACCEPT) }), "error");
            else if (f) loadFile(f);
          }}
        />
        <input
          ref={importRef}
          type="file"
          hidden
          onChange={(e) => {
            const f = e.target.files?.[0];
            e.target.value = "";
            if (f) importFile(f);
          }}
        />
      </div>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={itemMenu(menu.item)} onClose={() => setMenu(null)} />}
      {dlg && (dlg.kind === "new" || dlg.kind === "rename" || dlg.kind === "add" || dlg.kind === "renameItem") && (
        <NameDialog
          title={dlg.kind === "new" ? t("art.newLibrary") : dlg.kind === "rename" ? t("art.renameLibrary", { name: current ?? "" }) : dlg.kind === "add" ? t("art.addToLibrary") : t("art.renameItemTitle")}
          initial={dlg.kind === "rename" ? (current ?? "") : dlg.kind === "renameItem" ? (dlg.item?.name ?? "") : ""}
          placeholder={dlg.kind === "add" ? t("art.itemName") : undefined}
          optional={dlg.kind === "add"}
          problem={(n) => (dlg.kind === "new" ? libraryNameProblem(n, names) : dlg.kind === "rename" ? libraryNameProblem(n, names, current ?? undefined) : n.trim().length > 200 ? t("art.nameTooLong") : null)}
          onClose={() => setDlg(null)}
          onOk={(n) => {
            setDlg(null);
            const name = n.trim();
            if (dlg.kind === "new")
              void task(async () => {
                await artApi.create(name);
                setSelected(name);
              }, t("art.created", { name }));
            else if (dlg.kind === "rename" && current)
              void task(async () => {
                await artApi.rename(current, name);
                setSelected(name);
              }, t("art.renamed", { name }));
            else if (dlg.kind === "add" && current) void task(() => artApi.addSelection(current, p.selection, name || undefined), t("art.added", { library: current }));
            else if (dlg.kind === "renameItem" && current && dlg.item) void task(() => artApi.renameItem(current, dlg.item!.id, name));
          }}
        />
      )}
      {dlg?.kind === "copy" && current && (
        <PickLibraryDialog
          title={t(dlg.move ? "art.moveItemTo" : "art.copyItemTo", { name: dlg.item.name })}
          options={names.filter((n) => n !== current)}
          onClose={() => setDlg(null)}
          onOk={(to) => {
            const d = dlg;
            setDlg(null);
            void task(() => artApi.copyItem(current, d.item.id, to, d.move), t(d.move ? "art.moved" : "art.copied", { library: to }));
          }}
        />
      )}
      {(dlg?.kind === "delete" || dlg?.kind === "deleteItem") && current && (
        <Modal
          title={dlg.kind === "delete" ? t("art.deleteLibraryAsk", { name: current }) : t("art.deleteItemAsk", { name: dlg.item.name })}
          onClose={() => setDlg(null)}
          footer={
            <>
              <button className="btn" onClick={() => setDlg(null)}>
                {t("art.cancel")}
              </button>
              <button
                className="btn btn-danger"
                onClick={() => {
                  const d = dlg;
                  setDlg(null);
                  if (d.kind === "delete") void task(() => artApi.remove(current), t("art.deleted", { name: current }));
                  else void task(() => artApi.deleteItem(current, d.item.id));
                }}
              >
                {t("art.delete")}
              </button>
            </>
          }
        >
          <p>{dlg.kind === "delete" ? t("art.deleteLibraryNote") : t("art.deleteItemNote")}</p>
        </Modal>
      )}
    </Dock>
  );
}

function ArtThumb(p: { library: string; item: ArtItem; active: boolean; onSelect: () => void; onInsert: () => void; onMenu: (x: number, y: number) => void }) {
  const t = useT();
  const [src, setSrc] = useState<string | null>(null);
  const url = p.item.thumbnail;
  useEffect(() => {
    if (!url) return;
    let live = true;
    let made: string | null = null;
    artApi
      .thumbnail(url)
      .then((u) => {
        made = u;
        if (live) setSrc(u);
        else URL.revokeObjectURL(u);
      })
      .catch(() => undefined);
    return () => {
      live = false;
      if (made) URL.revokeObjectURL(made);
    };
  }, [url]);
  const size = p.item.size_mm ? `${p.item.size_mm[0].toFixed(1)} × ${p.item.size_mm[1].toFixed(1)} mm` : "";
  const longPress = useLongPress((x, y) => p.onMenu(x, y));
  const coarse = useLayoutMode().pointer === "coarse";
  return (
    <button
      className={`dz-art-item${p.active ? " active" : ""}`}
      role="option"
      aria-selected={p.active}
      draggable={!coarse}
      title={`${p.item.name}${size ? ` · ${size}` : ""}${p.item.objects ? ` · ${p.item.objects === 1 ? t("art.objects1") : t("art.objectsN", { n: p.item.objects })}` : ""}`}
      {...longPress}
      onClick={p.onSelect}
      onDoubleClick={p.onInsert}
      onDragStart={(e) => {
        e.dataTransfer.setData(ART_DRAG_TYPE, JSON.stringify({ library: p.library, id: p.item.id }));
        e.dataTransfer.effectAllowed = "copy";
      }}
    >
      {src ? <img src={src} alt="" draggable={false} /> : <span className="dz-art-ph" />}
      <span className="dz-art-name">{p.item.name}</span>
    </button>
  );
}

function NameDialog(p: { title: string; initial: string; placeholder?: string; optional?: boolean; problem: (n: string) => string | null; onClose: () => void; onOk: (n: string) => void }) {
  const t = useT();
  const [v, setV] = useState(p.initial);
  const problem = p.optional && !v.trim() ? null : p.problem(v);
  const ok = () => !problem && p.onOk(v);
  return (
    <Modal
      title={p.title}
      onClose={p.onClose}
      onSubmit={ok}
      footer={
        <>
          {problem && v && <span className="form-error">{problem}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("art.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!!problem} onClick={ok}>
            {t("art.ok")}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{t("art.name")}</span>
        <span className="field-input">
          <input autoFocus type="text" value={v} placeholder={p.placeholder} onChange={(e) => setV(e.target.value)} />
        </span>
      </label>
    </Modal>
  );
}

function PickLibraryDialog(p: { title: string; options: string[]; onClose: () => void; onOk: (n: string) => void }) {
  const t = useT();
  const [v, setV] = useState(p.options[0] ?? "");
  return (
    <Modal
      title={p.title}
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("art.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!v} onClick={() => p.onOk(v)}>
            {t("art.ok")}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{t("art.library")}</span>
        <span className="field-input">
          <select value={v} onChange={(e) => setV(e.target.value)}>
            {p.options.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </span>
      </label>
    </Modal>
  );
}
