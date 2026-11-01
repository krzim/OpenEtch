// Font picker (LightBurn's Font box): the engine's fonts from `font.list`, grouped as installed fonts, the bundled
// single-line Hershey fonts, AutoCAD SHX and EzCad JSF fonts, with a search box. Installed fonts are previewed in
// their own face. A font the engine does not have is shown with the "missing" badge.

import { useEffect, useMemo, useRef, useState } from "react";
import type { FontKind } from "../api/types.ts";
import { useT } from "../workspace/usePrefs.ts";
import { cssFontFamily, fontGroups, fontMissing, type FontInfo } from "./textModel.ts";

const KIND_KEY = {
  system: "text.font.kind.system",
  stroke: "text.font.kind.stroke",
  shx: "text.font.kind.shx",
  jsf: "text.font.kind.jsf",
} as const;

export function FontPicker(p: {
  value: { family: string; kind: FontKind } | null;
  fonts: FontInfo[] | null;
  onPick: (f: { family: string; kind: FontKind }) => void;
  onReload?: () => void;
  compact?: boolean;
  /** Shown when `value` is null (the engine's default font). */
  placeholder?: string;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const groups = useMemo(() => (p.fonts ? fontGroups(p.fonts, query) : []), [p.fonts, query]);
  const missing = !!p.value && fontMissing(p.value, p.fonts);

  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", down, true);
    return () => document.removeEventListener("pointerdown", down, true);
  }, [open]);

  const pick = (f: FontInfo) => {
    setOpen(false);
    setQuery("");
    p.onPick({ family: f.family, kind: f.kind });
  };

  return (
    <div className={`tx-font${p.compact ? " compact" : ""}`} ref={box}>
      <button
        type="button"
        className={`tx-font-btn${missing ? " missing" : ""}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={missing ? t("text.missingFontTitle", { font: p.value?.family ?? "" }) : t("text.font")}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="tx-font-name" style={p.value ? { fontFamily: cssFontFamily(p.value) } : undefined}>
          {p.value?.family ?? p.placeholder ?? t("text.font.default")}
        </span>
        {p.value && p.value.kind !== "system" && <span className="tx-kind">{t(KIND_KEY[p.value.kind])}</span>}
        {missing && <span className="tx-badge warn">{t("text.missing")}</span>}
        <span className="tx-caret" aria-hidden="true">
          ▾
        </span>
      </button>
      {open && (
        <div className="tx-font-pop" role="dialog" aria-label={t("text.font")}>
          <div className="tx-font-search">
            <input
              autoFocus
              type="search"
              placeholder={t("text.font.search")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Escape") setOpen(false);
                if (e.key === "Enter") {
                  const first = groups[0]?.fonts[0];
                  if (first) pick(first);
                }
              }}
            />
            {p.onReload && (
              <button type="button" className="btn btn-small" title={t("text.font.reloadTitle")} onClick={p.onReload}>
                {t("text.font.reload")}
              </button>
            )}
          </div>
          <div className="tx-font-list" role="listbox">
            {!p.fonts && <div className="tx-empty">{t("common.loading")}</div>}
            {p.fonts && groups.length === 0 && <div className="tx-empty">{t("text.font.none")}</div>}
            {groups.map((g) => (
              <div key={g.kind} role="group" aria-label={t(KIND_KEY[g.kind])}>
                <div className="tx-font-group">
                  {t(KIND_KEY[g.kind])} <span className="tx-count">{g.fonts.length}</span>
                </div>
                {g.fonts.map((f) => {
                  const sel = !!p.value && p.value.kind === f.kind && p.value.family.toLowerCase() === f.family.toLowerCase();
                  return (
                    <button
                      type="button"
                      role="option"
                      aria-selected={sel}
                      key={`${f.kind}:${f.family}`}
                      className={`tx-font-item${sel ? " sel" : ""}`}
                      style={{ fontFamily: cssFontFamily(f) }}
                      onClick={() => pick(f)}
                    >
                      {f.family}
                    </button>
                  );
                })}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
