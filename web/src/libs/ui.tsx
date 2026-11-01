// Small pieces the libs windows share: a dropdown menu button, a name prompt, a confirm dialog, a preview frame
// and the download helper.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { saveBlob } from "../api/jobApi.ts";
import { Icon } from "../components/ui.tsx";
import { WsModal } from "../workspace/WsModal.tsx";
import { useT } from "../workspace/usePrefs.ts";
import "./libs.css";

export type DropItem = { sep: true } | { label: string; hint?: string; disabled?: boolean; danger?: boolean; action: () => void; sep?: false };

/** A button that opens a menu of actions (LightBurn's "Manage Library" button). */
export function DropMenu(p: { label: ReactNode; title?: string; items: DropItem[]; className?: string; align?: "left" | "right"; up?: boolean; disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => root.current && !root.current.contains(e.target as Node) && setOpen(false);
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", down, true);
    window.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", down, true);
      window.removeEventListener("keydown", key);
    };
  }, [open]);
  return (
    <div className={`lx-drop${p.className ? ` ${p.className}` : ""}`} ref={root}>
      <button className={`btn small${open ? " active" : ""}`} aria-haspopup="menu" aria-expanded={open} title={p.title} disabled={p.disabled} onClick={() => setOpen((v) => !v)}>
        {p.label}
        <Icon name="chevronDown" size={14} />
      </button>
      {open && (
        <div className={`menu lx-drop-menu${p.align === "right" ? " menu-right" : ""}${p.up ? " up" : ""}`} role="menu">
          {p.items.map((it, i) =>
            it.sep ? (
              <div key={i} className="menu-sep" role="separator" />
            ) : (
              <button
                key={i}
                role="menuitem"
                className={`menu-item${it.danger ? " danger" : ""}`}
                disabled={it.disabled}
                onClick={() => {
                  setOpen(false);
                  it.action();
                }}
              >
                <span>{it.label}</span>
                {it.hint && <span className="menu-hint">{it.hint}</span>}
              </button>
            ),
          )}
        </div>
      )}
    </div>
  );
}

/** Ask for a name (New Library, Save As, Rename). `problem` explains why a name cannot be used. */
export function NameDialog(p: { title: string; label: string; initial: string; ok: string; problem: (name: string) => string | null; onOk: (name: string) => void; onClose: () => void; note?: string }) {
  const t = useT();
  const [name, setName] = useState(p.initial);
  const err = p.problem(name);
  const submit = () => {
    if (!err) p.onOk(name.trim());
  };
  return (
    <WsModal
      title={p.title}
      onClose={p.onClose}
      onSubmit={submit}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!!err} onClick={submit}>
            {p.ok}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{p.label}</span>
        <span className="field-input">
          <input type="text" autoFocus value={name} onChange={(e) => setName(e.target.value)} />
        </span>
      </label>
      {err && name.trim() !== "" && <p className="form-error">{err}</p>}
      {p.note && <p className="hint">{p.note}</p>}
    </WsModal>
  );
}

/** A yes / no question. */
export function ConfirmDialog(p: { title: string; children: ReactNode; ok: string; danger?: boolean; onOk: () => void; onClose: () => void; extra?: ReactNode }) {
  const t = useT();
  return (
    <WsModal
      title={p.title}
      onClose={p.onClose}
      footer={
        <>
          {p.extra}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className={`btn ${p.danger ? "btn-warn" : "btn-primary"}`} autoFocus onClick={p.onOk}>
            {p.ok}
          </button>
        </>
      }
    >
      {p.children}
    </WsModal>
  );
}

/** Save an engine download through a temporary link. */
export async function saveDownload(dl: Promise<{ blob: Blob; filename: string | null }>, fallback: string): Promise<string> {
  const { blob, filename } = await dl;
  const name = filename ?? fallback;
  saveBlob(blob, name);
  return name;
}

/** Read a picked file as standard base64. */
export async function fileBase64(f: Blob): Promise<string> {
  const bytes = new Uint8Array(await f.arrayBuffer());
  let s = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) s += String.fromCharCode(...bytes.subarray(i, i + chunk));
  return btoa(s);
}

/** A hidden file input with a button that opens it. */
export function FilePick(p: { label: string; accept: string; onFile: (f: File) => void; disabled?: boolean; className?: string; title?: string }) {
  const ref = useRef<HTMLInputElement>(null);
  return (
    <>
      <input
        ref={ref}
        type="file"
        accept={p.accept}
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = "";
          if (f) p.onFile(f);
        }}
      />
      <button className={p.className ?? "btn"} disabled={p.disabled} title={p.title} onClick={() => ref.current?.click()}>
        {p.label}
      </button>
    </>
  );
}

/** The engine's error text of a failure. */
export function errText(e: unknown): string {
  if (e && typeof e === "object" && "detail" in e && typeof (e as { detail?: unknown }).detail === "string") return (e as { detail: string }).detail;
  return e instanceof Error ? e.message : String(e);
}
