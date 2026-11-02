// LightBurn's Library window (docked Fusion-style under Cuts / Layers): the Select Library dropdown with Manage Library
// (Load, Save, Save As, Create New, Merge With, Rename, Unload, Download), the tree Material > Thickness or Title >
// Description, and for the picked preset Assign (copy to the layer), Link (the layer follows the preset and its
// settings cannot be edited until unlinked), Edit Cut, Update (from the layer), Edit Description, Duplicate and
// Delete, plus Create new from layer. Entry edits stay in the engine's memory until Save Library (the dirty dot).
// Routes: crates/engine/src/api_libs.rs. Linked layers show a link badge in Cuts / Layers and Properties.
// Touch: a long-press on an entry is its right-click (every entry action also has a button below the tree), and Load
// has no `accept` on iOS (checked after the pick, touch/filePick.ts).

import { useEffect, useMemo, useState } from "react";
import type { GDocument, Layer } from "../api/types.ts";
import { ContextMenu, type MenuItem } from "../components/ContextMenu.tsx";
import { CheckField, Icon, NumberField } from "../components/ui.tsx";
import { Dock } from "../workspace/Dock.tsx";
import { useLongPress } from "../touch/useLongPress.ts";
import { acceptLabel, acceptsFile, pickerAccept } from "../touch/filePick.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { formatSpeed } from "../workspace/units.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { libsApi } from "./api.ts";
import {
  cutEditOf,
  cutEditProblem,
  editCutSettings,
  entryMeta,
  entryMetaProblem,
  entrySummaryText,
  filterTree,
  layerLink,
  libraryNameFromFile,
  libraryNameProblem,
  linkedLayers,
  newEntryMeta,
  treeEntries,
  type CutEdit,
  type EntryMeta,
  type LibEntry,
} from "./model.ts";
import { takeLibraryView, useLibrary } from "./store.ts";
import { ConfirmDialog, DropMenu, FilePick, NameDialog, errText, saveDownload } from "./ui.tsx";
import "./libs.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

type Dlg =
  | { kind: "new" | "saveAs" | "rename" }
  | { kind: "merge" }
  | { kind: "unload"; name: string }
  | { kind: "meta"; entry: LibEntry | null }
  | { kind: "cut"; entry: LibEntry }
  | { kind: "delete"; entry: LibEntry }
  | null;

export function LibraryDock(p: { doc: GDocument | null; activeLayer: string | null; notify: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const { speedUnit } = usePrefs();
  const lib = useLibrary();
  const view = lib.view;
  const [picked, setPicked] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [closed, setClosed] = useState<Record<string, boolean>>({});
  const [dlg, setDlg] = useState<Dlg>(null);
  const [busy, setBusy] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number; entry: LibEntry } | null>(null);
  // Long-press (touch) or right-click on an entry: its menu. The entry row carries its id.
  const entryPress = useLongPress((x, y, el) => {
    const e = entries.find((q) => q.id === el.getAttribute("data-id"));
    if (!e) return;
    setPicked(e.id);
    setMenu({ x, y, entry: e });
  });
  const [layerId, setLayerId] = useState<string | null>(null);

  // The target layer follows the selected layer; the dropdown overrides it until the selection changes.
  useEffect(() => setLayerId(null), [p.activeLayer]);
  const layers = p.doc?.layers ?? [];
  const layer: Layer | null = layers.find((l) => l.id === (layerId ?? p.activeLayer)) ?? layers[0] ?? null;

  const selected = view?.selected ?? null;
  const tree = useMemo(() => filterTree(view?.tree ?? [], query), [view, query]);
  const entries = useMemo(() => treeEntries(view?.tree ?? []), [view]);
  const entry = entries.find((e) => e.id === picked) ?? null;
  const info = view?.libraries.find((l) => l.name === selected) ?? null;
  const names = view?.libraries.map((l) => l.name) ?? [];
  const link = layerLink(layer);
  const speed = (v: number) => formatSpeed(v, speedUnit);

  // An entry id names a position; after edits the same path is picked again.
  const pickPath = (path: string | null | undefined, fallback?: string | null) => {
    if (!path) return;
    const next = treeEntries(lib.view?.tree ?? []).find((e) => e.path === path);
    setPicked(next?.id ?? fallback ?? null);
  };

  const act = async (fn: () => Promise<unknown>, ok?: string | ((r: unknown) => string | null)) => {
    setBusy(true);
    try {
      const r = await fn();
      takeLibraryView(r);
      const msg = typeof ok === "function" ? ok(r) : ok;
      if (msg) p.notify(msg);
      return r;
    } catch (e) {
      p.report(new Error(errText(e)));
      return null;
    } finally {
      setBusy(false);
    }
  };

  // After an entry edit the tree is refetched (entry routes do not return it).
  const actEntry = async (fn: () => Promise<unknown>, ok: (r: Record<string, unknown>) => string | null) => {
    setBusy(true);
    try {
      const r = (await fn()) as Record<string, unknown>;
      await lib.refresh();
      const msg = ok(r ?? {});
      if (msg) p.notify(msg);
      return r;
    } catch (e) {
      p.report(new Error(errText(e)));
      return null;
    } finally {
      setBusy(false);
    }
  };

  const countText = (r: Record<string, unknown>, key: string) => {
    const n = Array.isArray(r[key]) ? (r[key] as unknown[]).length : 0;
    return n ? t("lib.layersFollowed", { count: n }) : "";
  };

  const assign = (e: LibEntry, linkIt: boolean) => {
    if (!selected || !layer) return;
    void act(() => libsApi.entryApply(selected, e.id, layer.id, linkIt), linkIt ? t("lib.linked", { layer: layer.name, path: e.path }) : t("lib.assigned", { layer: layer.name, path: e.path }));
  };
  const update = (e: LibEntry) => {
    if (!selected || !layer) return;
    void actEntry(() => libsApi.entryUpdateFromLayer(selected, e.id, layer.id), (r) => `${t("lib.updated", { path: e.path, layer: layer.name })} ${countText(r, "updated_layers")}`.trim());
  };
  const duplicate = (e: LibEntry) => {
    if (!selected) return;
    void actEntry(() => libsApi.entryDuplicate(selected, e.id), (r) => {
      if (typeof r.id === "string") setPicked(r.id);
      return t("lib.duplicated", { path: e.path });
    });
  };

  const entryMenu = (e: LibEntry): MenuItem[] => [
    { label: t("lib.assign"), disabled: !layer, action: () => assign(e, false) },
    { label: t("lib.link"), disabled: !layer, action: () => assign(e, true) },
    { sep: true },
    { label: t("lib.editCut"), disabled: !e.summary, action: () => setDlg({ kind: "cut", entry: e }) },
    { label: t("lib.update"), disabled: !layer, action: () => update(e) },
    { label: t("lib.editDescription"), action: () => setDlg({ kind: "meta", entry: e }) },
    { label: t("lib.duplicate"), action: () => duplicate(e) },
    { sep: true },
    { label: t("lib.delete"), action: () => setDlg({ kind: "delete", entry: e }) },
  ];

  const load = (f: File) => {
    let name = libraryNameFromFile(f.name);
    for (let i = 2; names.some((n) => n.toLowerCase() === name.toLowerCase()); i++) name = `${libraryNameFromFile(f.name)} (${i})`;
    void act(() => libsApi.libraryLoad(name, f), t("lib.loadedToast", { name }));
  };

  if (lib.supported === false) {
    return (
      <Dock id="library" title={t("lib.title")} defaultCollapsed>
        <p className="hint">{t("lib.unsupported")}</p>
      </Dock>
    );
  }

  return (
    <Dock id="library" title={t("lib.title")} defaultCollapsed>
      <div className="lx-lib-head">
        <label className="lx-inline">
          <span className="sr-only">{t("lib.select")}</span>
          <select
            value={selected ?? ""}
            title={t("lib.select")}
            disabled={busy || names.length === 0}
            onChange={(e) => {
              const name = e.target.value;
              setPicked(null);
              void act(() => libsApi.librarySelect(name), (r) => {
                const n = Array.isArray((r as { relinked_layers?: unknown })?.relinked_layers) ? (r as { relinked_layers: unknown[] }).relinked_layers.length : 0;
                return n ? t("lib.relinked", { count: n, name }) : null;
              });
            }}
          >
            {names.length === 0 && <option value="">{t("lib.none")}</option>}
            {view?.libraries.map((l) => (
              <option key={l.name} value={l.name}>
                {l.dirty ? `● ${l.name}` : l.name}
              </option>
            ))}
          </select>
        </label>
        <FilePick
          label={t("lib.load")}
          accept={pickerAccept(".clb") ?? ""}
          className="btn small"
          onFile={(f) => (acceptsFile(f, ".clb") ? load(f) : p.notify(t("touch.wrongFileType", { name: f.name, types: acceptLabel(".clb") })))}
          disabled={busy}
          title={t("lib.loadTitle")}
        />
        <DropMenu
          label={t("lib.manage")}
          align="right"
          disabled={busy}
          items={[
            { label: t("lib.save"), disabled: !selected, action: () => selected && void act(() => libsApi.librarySave(selected), t("lib.savedToast", { name: selected })) },
            { label: t("lib.saveAs"), disabled: !selected, action: () => setDlg({ kind: "saveAs" }) },
            { label: t("lib.new"), action: () => setDlg({ kind: "new" }) },
            { label: t("lib.merge"), disabled: !selected || names.length < 2, action: () => setDlg({ kind: "merge" }) },
            { sep: true },
            { label: t("lib.rename"), disabled: !selected, action: () => setDlg({ kind: "rename" }) },
            { label: t("lib.unload"), disabled: !selected, action: () => selected && setDlg({ kind: "unload", name: selected }) },
            { sep: true },
            {
              label: t("lib.download"),
              disabled: !selected,
              action: () => selected && void saveDownload(libsApi.libraryDownload(selected), `${selected}.clb`).catch(p.report),
            },
          ]}
        />
      </div>
      {info && (
        <p className="hint lx-lib-info">
          {t("lib.info", { entries: info.entries, materials: info.materials })}
          {info.dirty && <span className="lx-dirty"> · {t("lib.unsaved")}</span>}
        </p>
      )}
      {selected && (
        <input className="lx-search" type="search" placeholder={t("lib.search")} aria-label={t("lib.search")} value={query} onChange={(e) => setQuery(e.target.value)} />
      )}
      <div className="lx-tree" role="tree" aria-label={t("lib.tree")}>
        {!selected && <p className="hint">{t("lib.empty")}</p>}
        {selected && tree.length === 0 && <p className="hint">{query ? t("lib.noMatch") : t("lib.noEntries")}</p>}
        {tree.map((m) => {
          const mk = `m:${m.name}`;
          const mOpen = !closed[mk] || !!query;
          return (
            <div key={mk} role="treeitem" aria-expanded={mOpen}>
              <button className="lx-tree-row lx-tree-mat" onClick={() => setClosed((c) => ({ ...c, [mk]: mOpen }))}>
                <span className={`chevron${mOpen ? " open" : ""}`}>
                  <Icon name="chevron" size={12} />
                </span>
                {m.name}
              </button>
              {mOpen &&
                m.groups.map((g) => {
                  const gk = `${mk}/${g.title}`;
                  const gOpen = !closed[gk] || !!query;
                  return (
                    <div key={gk} role="group">
                      <button className="lx-tree-row lx-tree-group" onClick={() => setClosed((c) => ({ ...c, [gk]: gOpen }))}>
                        <span className={`chevron${gOpen ? " open" : ""}`}>
                          <Icon name="chevron" size={12} />
                        </span>
                        {g.title}
                      </button>
                      {gOpen &&
                        g.entries.map((e) => {
                          const users = linkedLayers(p.doc, selected ?? "", e.path);
                          return (
                            <button
                              key={e.id}
                              role="treeitem"
                              aria-selected={picked === e.id}
                              className={`lx-tree-row lx-tree-entry${picked === e.id ? " selected" : ""}`}
                              title={`${e.path}\n${entrySummaryText(e.summary, speed)}`}
                              data-id={e.id}
                              {...entryPress}
                              onClick={() => setPicked(e.id)}
                              onDoubleClick={() => assign(e, false)}
                            >
                              <span className="lx-entry-name">{e.description}</span>
                              {users.length > 0 && (
                                <span className="lx-link-badge" title={t("lib.linkedTo", { layers: users.map((l) => l.name).join(", ") })}>
                                  <LinkIcon /> {users.length}
                                </span>
                              )}
                            </button>
                          );
                        })}
                    </div>
                  );
                })}
            </div>
          );
        })}
      </div>

      <div className="lx-lib-layer">
        <label className="field">
          <span className="field-label">{t("lib.layer")}</span>
          <span className="field-input">
            <select value={layer?.id ?? ""} onChange={(e) => setLayerId(e.target.value)} disabled={layers.length === 0}>
              {layers.map((l) => (
                <option key={l.id} value={l.id}>
                  {layerLink(l) ? `🔗 ${l.name}` : l.name}
                </option>
              ))}
            </select>
          </span>
        </label>
        {link && (
          <p className="hint">
            <LinkIcon /> {t("lib.layerLinked", { path: link.path, library: link.library })}
          </p>
        )}
      </div>

      {entry && (
        <div className="lx-entry">
          <div className="lx-entry-path">{entry.path}</div>
          <div className="muted lx-entry-sum">{entrySummaryText(entry.summary, speed)}</div>
        </div>
      )}
      <div className="lx-btns">
        <button className="btn small btn-primary" disabled={busy || !entry || !layer} title={link ? t("lib.assignLinkedTitle") : t("lib.assignTitle")} onClick={() => entry && assign(entry, false)}>
          {t("lib.assign")}
        </button>
        <button className="btn small" disabled={busy || !entry || !layer} title={t("lib.linkTitle")} onClick={() => entry && assign(entry, true)}>
          <LinkIcon /> {t("lib.link")}
        </button>
        <button className="btn small" disabled={busy || !entry?.summary} title={t("lib.editCutTitle")} onClick={() => entry && setDlg({ kind: "cut", entry })}>
          {t("lib.editCut")}
        </button>
        <button className="btn small" disabled={busy || !entry || !layer} title={t("lib.updateTitle")} onClick={() => entry && update(entry)}>
          {t("lib.update")}
        </button>
        <button className="btn small" disabled={busy || !entry} onClick={() => entry && setDlg({ kind: "meta", entry })}>
          {t("lib.editDescription")}
        </button>
        <button className="btn small" disabled={busy || !entry} onClick={() => entry && duplicate(entry)}>
          {t("lib.duplicate")}
        </button>
        <button className="btn small btn-warn" disabled={busy || !entry} onClick={() => entry && setDlg({ kind: "delete", entry })}>
          {t("lib.delete")}
        </button>
      </div>
      <button className="btn small lx-create" disabled={busy || !selected || !layer} title={t("lib.createTitle")} onClick={() => setDlg({ kind: "meta", entry: null })}>
        <Icon name="plus" size={14} /> {t("lib.create")}
      </button>

      {menu && <ContextMenu x={menu.x} y={menu.y} items={entryMenu(menu.entry)} onClose={() => setMenu(null)} />}

      {dlg && (dlg.kind === "new" || dlg.kind === "saveAs" || dlg.kind === "rename") && (
        <NameDialog
          title={t(dlg.kind === "new" ? "lib.new" : dlg.kind === "saveAs" ? "lib.saveAs" : "lib.rename")}
          label={t("lib.name")}
          initial={dlg.kind === "new" ? "" : dlg.kind === "saveAs" ? `${selected ?? ""} copy` : (selected ?? "")}
          ok={t(dlg.kind === "new" ? "lib.createOk" : dlg.kind === "saveAs" ? "lib.save" : "lib.rename")}
          note={dlg.kind === "rename" ? t("lib.renameNote") : dlg.kind === "saveAs" ? t("lib.saveAsNote") : undefined}
          problem={(n) => libraryNameProblem(n, names, dlg.kind === "rename" ? (selected ?? undefined) : undefined)}
          onClose={() => setDlg(null)}
          onOk={(name) => {
            const k = dlg.kind;
            setDlg(null);
            if (k === "new") void act(() => libsApi.libraryNew(name), t("lib.createdToast", { name }));
            else if (k === "saveAs" && selected) void act(() => libsApi.librarySave(selected, name), t("lib.savedToast", { name }));
            else if (k === "rename" && selected) void act(() => libsApi.libraryRename(selected, name), t("lib.renamedToast", { name }));
          }}
        />
      )}
      {dlg?.kind === "merge" && selected && (
        <MergeDialog
          into={selected}
          names={names.filter((n) => n !== selected)}
          onClose={() => setDlg(null)}
          onMerge={(from) => {
            setDlg(null);
            void act(() => libsApi.libraryMerge(selected, from), (r) => t("lib.mergedToast", { count: Number((r as { added?: unknown })?.added ?? 0), from, into: selected }));
          }}
        />
      )}
      {dlg?.kind === "unload" && (
        <ConfirmDialog
          title={t("lib.unload")}
          ok={view?.libraries.find((l) => l.name === dlg.name)?.dirty ? t("lib.unloadDiscard") : t("lib.unload")}
          danger={!!view?.libraries.find((l) => l.name === dlg.name)?.dirty}
          onClose={() => setDlg(null)}
          onOk={() => {
            const n = dlg.name;
            const dirty = !!view?.libraries.find((l) => l.name === n)?.dirty;
            setDlg(null);
            setPicked(null);
            void act(() => libsApi.libraryUnload(n, dirty), t("lib.unloadedToast", { name: n }));
          }}
          extra={
            view?.libraries.find((l) => l.name === dlg.name)?.dirty ? (
              <button
                className="btn"
                onClick={() => {
                  const n = dlg.name;
                  setDlg(null);
                  void act(async () => {
                    await libsApi.librarySave(n);
                    return libsApi.libraryUnload(n, false);
                  }, t("lib.unloadedToast", { name: n }));
                }}
              >
                {t("lib.saveUnload")}
              </button>
            ) : undefined
          }
        >
          <p>{view?.libraries.find((l) => l.name === dlg.name)?.dirty ? t("lib.unloadDirty", { name: dlg.name }) : t("lib.unloadText", { name: dlg.name })}</p>
        </ConfirmDialog>
      )}
      {dlg?.kind === "meta" && (
        <EntryMetaDialog
          entry={dlg.entry}
          layer={layer}
          materials={view?.tree.map((m) => m.name) ?? []}
          onClose={() => setDlg(null)}
          onOk={(meta) => {
            const e = dlg.entry;
            setDlg(null);
            if (!selected) return;
            if (e)
              void actEntry(() => libsApi.entryEditMeta(selected, e.id, meta), (r) => {
                pickPath(typeof r.path === "string" ? r.path : null, typeof r.id === "string" ? r.id : null);
                return `${t("lib.metaSaved", { path: String(r.path ?? "") })} ${countText(r, "updated_layers")}`.trim();
              });
            else if (layer)
              void actEntry(() => libsApi.entryCreate(selected, layer.id, meta), (r) => {
                if (typeof r.id === "string") setPicked(r.id);
                return t("lib.createdEntry", { path: String(r.path ?? ""), layer: layer.name });
              });
          }}
        />
      )}
      {dlg?.kind === "cut" && selected && dlg.entry.summary && (
        <EditCutDialog
          entry={dlg.entry}
          base={(linkedLayers(p.doc, selected, dlg.entry.path)[0]?.settings as unknown as Record<string, unknown>) ?? null}
          onClose={() => setDlg(null)}
          onOk={(edit, base) => {
            const e = dlg.entry;
            setDlg(null);
            void actEntry(() => libsApi.entryEditCut(selected, e.id, editCutSettings(base, edit)), (r) => `${t("lib.cutSaved", { path: e.path })} ${countText(r, "updated_layers")}`.trim());
          }}
        />
      )}
      {dlg?.kind === "delete" && selected && (
        <ConfirmDialog
          title={t("lib.delete")}
          ok={t("lib.delete")}
          danger
          onClose={() => setDlg(null)}
          onOk={() => {
            const e = dlg.entry;
            setDlg(null);
            setPicked(null);
            void actEntry(() => libsApi.entryDelete(selected, e.id), (r) => `${t("lib.deleted", { path: e.path })} ${countText(r, "unlinked_layers")}`.trim());
          }}
        >
          <p>{t("lib.deleteText", { path: dlg.entry.path })}</p>
          {linkedLayers(p.doc, selected, dlg.entry.path).length > 0 && <p className="run-hint">{t("lib.deleteLinked", { count: linkedLayers(p.doc, selected, dlg.entry.path).length })}</p>}
        </ConfirmDialog>
      )}
    </Dock>
  );
}

export function LinkIcon() {
  return (
    <svg className="icon lx-link-icon" width={13} height={13} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M10 14a4 4 0 005.7 0l3-3a4 4 0 00-5.7-5.7l-1 1 M14 10a4 4 0 00-5.7 0l-3 3a4 4 0 005.7 5.7l1-1" />
    </svg>
  );
}

function MergeDialog(p: { into: string; names: string[]; onClose: () => void; onMerge: (from: string) => void }) {
  const t = useT();
  const [from, setFrom] = useState(p.names[0] ?? "");
  return (
    <ConfirmDialog title={t("lib.merge")} ok={t("lib.mergeOk")} onClose={p.onClose} onOk={() => from && p.onMerge(from)}>
      <label className="field">
        <span className="field-label">{t("lib.mergeFrom")}</span>
        <span className="field-input">
          <select value={from} onChange={(e) => setFrom(e.target.value)}>
            {p.names.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </span>
      </label>
      <p className="hint">{t("lib.mergeNote", { into: p.into })}</p>
    </ConfirmDialog>
  );
}

/** Create new from layer / Edit Description: Material Name, Thickness or No Thickness + Title, Description. */
function EntryMetaDialog(p: { entry: LibEntry | null; layer: Layer | null; materials: string[]; onClose: () => void; onOk: (m: EntryMeta) => void }) {
  const t = useT();
  const [m, setM] = useState<EntryMeta>(() => (p.entry ? entryMeta(p.entry) : newEntryMeta(p.layer, p.materials[0] ?? "")));
  const [thick, setThick] = useState<number>(m.thickness_mm ?? 3);
  const noThick = m.thickness_mm === null;
  const err = entryMetaProblem(m);
  const ok = Object.keys(err).length === 0;
  const submit = () => ok && p.onOk({ ...m, material: m.material.trim(), title: m.title.trim(), description: m.description.trim() });
  return (
    <WsModal
      title={p.entry ? t("lib.editDescription") : t("lib.createFrom", { layer: p.layer?.name ?? "" })}
      onClose={p.onClose}
      onSubmit={submit}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!ok} onClick={submit}>
            {t("jobui.ok")}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{t("lib.material")}</span>
        <span className="field-input">
          <input type="text" list="lx-materials" autoFocus value={m.material} onChange={(e) => setM({ ...m, material: e.target.value })} />
          <datalist id="lx-materials">
            {p.materials.map((x) => (
              <option key={x} value={x} />
            ))}
          </datalist>
        </span>
      </label>
      {err.material && m.material && <p className="form-error">{err.material}</p>}
      <CheckField label={t("lib.noThickness")} checked={noThick} onChange={(v) => setM({ ...m, thickness_mm: v ? null : thick })} />
      {noThick ? (
        <label className="field">
          <span className="field-label">{t("lib.titleField")}</span>
          <span className="field-input">
            <input type="text" value={m.title} placeholder={t("lib.titlePlaceholder")} onChange={(e) => setM({ ...m, title: e.target.value })} />
          </span>
        </label>
      ) : (
        <NumberField
          label={t("lib.thickness")}
          unit="mm"
          value={m.thickness_mm}
          min={0.001}
          max={1000}
          digits={3}
          onCommit={(v) => {
            if (v === null) return;
            setThick(v);
            setM({ ...m, thickness_mm: v });
          }}
        />
      )}
      {(err.title || err.thickness_mm) && <p className="form-error">{err.title ?? err.thickness_mm}</p>}
      <label className="field">
        <span className="field-label">{t("lib.description")}</span>
        <span className="field-input">
          <input type="text" value={m.description} onChange={(e) => setM({ ...m, description: e.target.value })} />
        </span>
      </label>
      {err.description && m.description && <p className="form-error">{err.description}</p>}
      <p className="hint">{p.entry ? t("lib.metaHint") : t("lib.createHint")}</p>
    </WsModal>
  );
}

const MODES = [
  { v: "line", k: "lib.modeLine" },
  { v: "fill", k: "lib.modeFill" },
  { v: "fill_then_line", k: "lib.modeFillLine" },
  { v: "offset_fill", k: "lib.modeOffset" },
] as const;

/** Edit Cut: the preset's laser parameters (LightBurn's Cut Settings Editor for the preset). */
function EditCutDialog(p: { entry: LibEntry; base: Record<string, unknown> | null; onClose: () => void; onOk: (e: CutEdit, base: Record<string, unknown> | null) => void }) {
  const t = useT();
  const [e, setE] = useState<CutEdit>(() => cutEditOf(p.entry.summary!));
  const err = cutEditProblem(e);
  const num = (k: keyof CutEdit) => (v: number | null) => v !== null && setE((x) => ({ ...x, [k]: v }));
  return (
    <WsModal
      title={t("lib.editCutOf", { path: p.entry.path })}
      onClose={p.onClose}
      onSubmit={() => !err && p.onOk(e, p.base)}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!!err} onClick={() => p.onOk(e, p.base)}>
            {t("jobui.ok")}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{t("lib.mode")}</span>
        <span className="field-input">
          <select value={e.mode} onChange={(x) => setE({ ...e, mode: x.target.value })}>
            {MODES.map((m) => (
              <option key={m.v} value={m.v}>
                {t(m.k)}
              </option>
            ))}
          </select>
        </span>
      </label>
      <NumberField label={t("lib.power")} unit="%" value={e.power_pct} min={0} max={100} digits={1} onCommit={num("power_pct")} />
      <NumberField label={t("lib.speed")} unit="mm/s" value={e.speed_mm_s} min={0.01} digits={1} onCommit={num("speed_mm_s")} />
      <NumberField label={t("lib.frequency")} unit="kHz" value={e.frequency_khz} min={0.1} digits={1} onCommit={num("frequency_khz")} />
      <NumberField label={t("lib.pulse")} unit="ns" value={e.pulse_width_ns} min={1} digits={0} allowEmpty placeholder={t("lib.sourceDefault")} onCommit={(v) => setE({ ...e, pulse_width_ns: v })} />
      <NumberField label={t("lib.passes")} value={e.passes} min={1} max={1000} digits={0} onCommit={(v) => v !== null && setE({ ...e, passes: Math.round(v) })} />
      {e.mode !== "line" && <NumberField label={t("lib.interval")} unit="mm" value={e.interval_mm} min={0.001} digits={4} onCommit={num("interval_mm")} />}
      {err && <p className="form-error">{err}</p>}
      <p className="hint">{p.base ? t("lib.editCutLinked") : t("lib.editCutHint")}</p>
    </WsModal>
  );
}
