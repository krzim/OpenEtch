// Font picker (LightBurn's Font box and its 2.1 Font Manager): the engine's fonts from `font.list`, favourites first,
// then installed fonts, the bundled single-line Hershey fonts, AutoCAD SHX and EzCad JSF fonts, with a search box.
// Every row shows a live sample drawn by the engine (`font.preview`, fontSamples.ts: the browser has neither the
// single-line fonts nor the engine computer's installed fonts), in the selected text or a sample typed here. A star
// marks a favourite and the eye hides a font from the list (fontPrefs.ts; kept by the engine for every device); "Show
// hidden" brings hidden fonts back. A font the engine does not have is shown with the "missing" badge.

import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { FontKind } from "../api/types.ts";
import { Icon } from "../components/ui.tsx";
import { errorText } from "../api/errorText.ts";
import { useT } from "../workspace/usePrefs.ts";
import { fontPrefs, isFavourite, isHidden, loadFontPrefs, pickerGroups, saveFontPrefs, subscribeFontPrefs, toggleFavourite, toggleHidden, type PickerGroupId } from "./fontPrefs.ts";
import { cachedSample, clearSamples, requestSample, sampleText, subscribeSamples } from "./fontSamples.ts";
import { cssFontFamily, fontMissing, type FontInfo } from "./textModel.ts";

const GROUP_KEY = {
  favourites: "text.font.favourites",
  system: "text.font.kind.system",
  stroke: "text.font.kind.stroke",
  shx: "text.font.kind.shx",
  jsf: "text.font.kind.jsf",
} as const satisfies Record<PickerGroupId, string>;

const KIND_KEY = {
  system: "text.font.kind.system",
  stroke: "text.font.kind.stroke",
  shx: "text.font.kind.shx",
  jsf: "text.font.kind.jsf",
} as const;

function useFontPrefs() {
  useEffect(() => void loadFontPrefs(), []);
  return useSyncExternalStore(subscribeFontPrefs, fontPrefs, fontPrefs);
}

/** Re-render when samples arrive. */
function useSamplesVersion(): number {
  const [n, setN] = useState(0);
  useEffect(() => subscribeSamples(() => setN((x) => x + 1)), []);
  return n;
}

export function FontPicker(p: {
  value: { family: string; kind: FontKind } | null;
  fonts: FontInfo[] | null;
  onPick: (f: { family: string; kind: FontKind }) => void;
  onReload?: () => void;
  compact?: boolean;
  /** Shown when `value` is null (the engine's default font). */
  placeholder?: string;
  /** The text the samples show (the selected text); default "AaBbCc 123". */
  sample?: string;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [own, setOwn] = useState("");
  const [showHidden, setShowHidden] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const prefs = useFontPrefs();
  useSamplesVersion();
  const { groups, hiddenCount } = useMemo(
    () => (p.fonts ? pickerGroups(p.fonts, query, prefs, showHidden, p.value) : { groups: [], hiddenCount: 0 }),
    [p.fonts, query, prefs, showHidden, p.value],
  );
  const missing = !!p.value && fontMissing(p.value, p.fonts);
  const text = sampleText(own || p.sample);

  // One observer for the open list: a row asks for its sample when it scrolls into view.
  const [observer, setObserver] = useState<IntersectionObserver | null>(null);
  useEffect(() => {
    if (!open || typeof IntersectionObserver === "undefined") return;
    const o = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (!e.isIntersecting) continue;
          const el = e.target as HTMLElement;
          const family = el.dataset.family;
          const kind = el.dataset.kind as FontKind | undefined;
          if (family && kind) requestSample({ family, kind }, el.dataset.sample ?? text);
        }
      },
      { root: list.current, rootMargin: "120px 0px" },
    );
    setObserver(o);
    return () => {
      o.disconnect();
      setObserver(null);
    };
  }, [open, text]);

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
  const save = (next: typeof prefs) => {
    setError(null);
    void saveFontPrefs(next).catch((e) => setError(errorText(e)));
  };
  const first = groups.find((g) => g.id !== "favourites")?.fonts[0] ?? groups[0]?.fonts[0];

  return (
    <div className={`tx-font${p.compact ? " compact" : ""}`} ref={box}>
      <button
        type="button"
        className={`tx-font-btn${missing ? " missing" : ""}`}
        aria-haspopup="dialog"
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
        <div className="tx-font-pop popover" role="dialog" aria-label={t("text.font")}>
          <div className="tx-font-search">
            <input
              autoFocus
              type="search"
              placeholder={t("text.font.search")}
              aria-label={t("text.font.search")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Escape") setOpen(false);
                if (e.key === "Enter" && first) pick(first);
              }}
            />
            {p.onReload && (
              <button
                type="button"
                className="btn btn-small"
                title={t("text.font.reloadTitle")}
                onClick={() => {
                  clearSamples();
                  p.onReload?.();
                }}
              >
                {t("text.font.reload")}
              </button>
            )}
          </div>
          <div className="tx-font-tools">
            <input
              type="text"
              className="tx-font-sample-in"
              placeholder={t("text.font.sample")}
              aria-label={t("text.font.sample")}
              title={t("text.font.sampleTitle")}
              value={own}
              maxLength={40}
              onChange={(e) => setOwn(e.target.value)}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Escape") setOpen(false);
              }}
            />
            {(hiddenCount > 0 || showHidden) && (
              <button type="button" className={`btn btn-small${showHidden ? " on" : ""}`} aria-pressed={showHidden} onClick={() => setShowHidden((v) => !v)}>
                {t("text.font.showHidden", { n: hiddenCount })}
              </button>
            )}
          </div>
          {error && <p className="form-error tx-font-error">{error}</p>}
          <div className="tx-font-list" ref={list}>
            {!p.fonts && <div className="tx-empty">{t("common.loading")}</div>}
            {p.fonts && groups.length === 0 && <div className="tx-empty">{t(hiddenCount ? "text.font.allHidden" : "text.font.none")}</div>}
            {groups.map((g) => (
              <div key={g.id} role="group" aria-label={t(GROUP_KEY[g.id])}>
                <div className="tx-font-group">
                  {t(GROUP_KEY[g.id])} <span className="tx-count">{g.fonts.length}</span>
                </div>
                <ul className="tx-font-rows">
                  {g.fonts.map((f) => (
                    <FontRow
                      key={`${f.kind}:${f.family}`}
                      font={f}
                      text={text}
                      observer={observer}
                      selected={!!p.value && p.value.kind === f.kind && p.value.family.toLowerCase() === f.family.toLowerCase()}
                      favourite={isFavourite(prefs, f)}
                      hidden={isHidden(prefs, f)}
                      onPick={() => pick(f)}
                      onFavourite={() => save(toggleFavourite(prefs, f))}
                      onHide={() => save(toggleHidden(prefs, f))}
                    />
                  ))}
                </ul>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function FontRow(p: {
  font: FontInfo;
  text: string;
  observer: IntersectionObserver | null;
  selected: boolean;
  favourite: boolean;
  hidden: boolean;
  onPick: () => void;
  onFavourite: () => void;
  onHide: () => void;
}) {
  const t = useT();
  const ref = useRef<HTMLButtonElement>(null);
  const { font: f } = p;
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (!p.observer) {
      // No IntersectionObserver (old browsers, tests): ask right away.
      requestSample(f, p.text);
      return;
    }
    p.observer.observe(el);
    return () => p.observer?.unobserve(el);
  }, [p.observer, f, p.text]);
  const s = cachedSample(f, p.text);
  const stroke = f.kind !== "system";
  return (
    <li className={`tx-font-row${p.selected ? " sel" : ""}${p.hidden ? " is-hidden" : ""}`}>
      <button
        type="button"
        className={`tx-fav${p.favourite ? " on" : ""}`}
        aria-pressed={p.favourite}
        aria-label={t(p.favourite ? "text.font.unfavourite" : "text.font.favourite", { font: f.family })}
        title={t(p.favourite ? "text.font.unfavourite" : "text.font.favourite", { font: f.family })}
        onClick={p.onFavourite}
      >
        <Icon name="star" size={16} />
      </button>
      <button
        type="button"
        ref={ref}
        className="tx-font-item"
        aria-current={p.selected ? "true" : undefined}
        data-family={f.family}
        data-kind={f.kind}
        data-sample={p.text}
        onClick={p.onPick}
      >
        <span className="tx-font-label" style={{ fontFamily: cssFontFamily(f) }}>
          {f.family}
        </span>
        {s && "d" in s ? (
          <svg className={`tx-sample${stroke ? " stroke" : ""}`} viewBox={s.viewBox.join(" ")} preserveAspectRatio="xMinYMid meet" aria-hidden="true" focusable="false">
            <path d={s.d} />
          </svg>
        ) : s ? (
          <span className="tx-sample-none">{t("text.font.noSample")}</span>
        ) : (
          <span className="tx-sample-wait" aria-hidden="true" />
        )}
      </button>
      <button
        type="button"
        className={`tx-hide${p.hidden ? " on" : ""}`}
        aria-pressed={p.hidden}
        aria-label={t(p.hidden ? "text.font.unhide" : "text.font.hide", { font: f.family })}
        title={t(p.hidden ? "text.font.unhide" : "text.font.hide", { font: f.family })}
        onClick={p.onHide}
      >
        <Icon name={p.hidden ? "eye" : "eyeOff"} size={16} />
      </button>
    </li>
  );
}
