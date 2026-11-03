// The user guide viewer (the lazy chunk GuideHost.tsx loads): a page of docs/guide rendered by render.tsx, the
// contents grouped by section, search over titles, headings and text, previous / next, and anchors scrolled into view.
// Three modes: "pane" beside the canvas (desktop, MacBook, iPad landscape) with Open in new tab, "sheet" full screen
// under the top bar (phone, iPad portrait) with a back button, and "page", the guide alone in its own tab (`?guide`).

import { useEffect, useMemo, useRef, useState, type KeyboardEvent as RKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Icon } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { imageSize, imageUrl, loadGuide } from "./content.ts";
import { designOnlyBuild } from "./flags.ts";
import { contents, prevNext, searchGuide, type GuidePage } from "./pages.ts";
import { anchorDomId, Markdown, type RenderCtx } from "./render.tsx";
import { backGuide, closeGuide, GUIDE_FOCUS_EVENT, guideDepth, guideHash, navigateGuide, standaloneUrl, type GuideRoute } from "./route.ts";
import "./guide.css";

export interface GuideViewerProps {
  route: GuideRoute;
  mode: "pane" | "sheet" | "page";
  /** Offer Open in new tab (not on a phone: a second tab there is just another screen). */
  newTab?: boolean;
}

export default function GuideViewer(p: GuideViewerProps) {
  const t = useT();
  const guide = useMemo(loadGuide, []);
  const designOnly = designOnlyBuild();
  const [q, setQ] = useState("");
  const [toc, setToc] = useState(false);
  const [nonce, setNonce] = useState(0);
  // A picture shown full size over the guide (render.tsx's zoom button), or null.
  const [zoom, setZoom] = useState<{ url: string; alt: string } | null>(null);
  const [zoomActual, setZoomActual] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const found = guide.bySlug.get(p.route.slug);
  const page = found && !(designOnly && found.needs === "machine") ? found : null;
  const toc2 = useMemo(() => contents(guide, designOnly), [guide, designOnly]);
  const hits = useMemo(() => (q.trim() ? searchGuide(guide, q, designOnly) : []), [guide, q, designOnly]);
  const { prev, next } = page ? prevNext(guide, page.slug, designOnly) : { prev: null, next: null };
  // The page alone in a wide tab keeps the contents in a column of its own.
  const [wide, setWide] = useState(false);
  useEffect(() => {
    if (p.mode !== "page") return;
    const on = () => setWide(window.innerWidth >= 1000);
    on();
    window.addEventListener("resize", on);
    return () => window.removeEventListener("resize", on);
  }, [p.mode]);
  const showToc = toc || (wide && !q.trim());

  // The guide alone in its tab: the tab and its bookmarks name the page.
  useEffect(() => {
    if (p.mode !== "page") return;
    document.title = page && page.slug !== "index" ? `${page.title} · ${t("guide.docTitle")}` : t("guide.docTitle");
  }, [p.mode, page, t]);

  // F1, Help > User guide or a "?" while the guide is open already: take the focus (the anchor's heading, else the
  // page's title), so Esc closes the guide at once.
  useEffect(() => {
    const focus = () =>
      window.requestAnimationFrame(() => {
        const box = scroller.current;
        const el = box?.querySelector<HTMLElement>(p.route.anchor ? `#${CSS.escape(anchorDomId(p.route.anchor))}` : "h1") ?? null;
        (el ?? box)?.focus({ preventScroll: true });
      });
    window.addEventListener(GUIDE_FOCUS_EVENT, focus);
    return () => window.removeEventListener(GUIDE_FOCUS_EVENT, focus);
  }, [p.route.anchor]);

  const go = (r: GuideRoute) => {
    setQ("");
    setToc(false);
    if (r.slug === p.route.slug && r.anchor === p.route.anchor) setNonce((n) => n + 1);
    else navigateGuide(r, { replace: r.slug === p.route.slug });
  };

  // Scroll to the anchor (focused, for keyboard and screen-reader users), or to the top of a new page.
  useEffect(() => {
    const box = scroller.current;
    if (!box || q.trim() || (toc && !wide)) return;
    const el = p.route.anchor ? box.querySelector<HTMLElement>(`#${CSS.escape(anchorDomId(p.route.anchor))}`) : null;
    if (el) {
      // Only the guide's own scroller moves (scrollIntoView would also scroll the app's overflow-hidden boxes).
      box.scrollTop += el.getBoundingClientRect().top - box.getBoundingClientRect().top - 8;
      el.focus({ preventScroll: true });
    } else {
      box.scrollTop = 0;
      // Keyboard users land in the guide (Esc closes it); the page's title takes the focus without scrolling.
      if (p.mode !== "page") (box.querySelector<HTMLElement>("h1") ?? box).focus({ preventScroll: true });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.route.slug, p.route.anchor, nonce, toc]);

  // A new search shows its first results: the list replaces the page in the same scroller, which kept the page's
  // scroll position (a search typed half way down a page opened on its third hit, the first ones scrolled away).
  useEffect(() => {
    if (q.trim() && scroller.current) scroller.current.scrollTop = 0;
  }, [q]);

  const ctx: RenderCtx = {
    slug: p.route.slug,
    go,
    image: imageUrl,
    imageSize,
    newTab: t("guide.opensNewTab"),
    missingImage: (name) => t("guide.imageMissing", { name }),
    zoom: (url, alt) => {
      setZoomActual(false);
      setZoom({ url, alt });
    },
    zoomLabel: t("guide.zoomImage"),
  };

  const onKey = (e: RKeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    if (zoom) setZoom(null);
    else if (q) setQ("");
    else if (toc && !wide) setToc(false);
    else if (p.mode !== "page") closeGuide();
  };

  const pageLink = (pg: GuidePage, cls = "") => (
    <a
      href={guideHash({ slug: pg.slug, anchor: null })}
      className={`guide-toc-link${pg.slug === page?.slug ? " current" : ""}${cls}`}
      aria-current={pg.slug === page?.slug ? "page" : undefined}
      onClick={(e) => {
        if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
        e.preventDefault();
        go({ slug: pg.slug, anchor: null });
      }}
    >
      <span className="guide-toc-title">{pg.title}</span>
      {pg.needs === "machine" && <NeedsTag />}
    </a>
  );

  const contentsList = (
    <nav className="guide-toc" aria-label={t("guide.contents")}>
      {toc2.map((s) => (
        <section key={s.section}>
          <h2 className="guide-toc-section">{s.section}</h2>
          <ul>
            {s.pages.map((pg) => (
              <li key={pg.slug}>
                {pageLink(pg)}
                {!wide && pg.summary && <span className="guide-toc-summary">{pg.summary}</span>}
              </li>
            ))}
          </ul>
        </section>
      ))}
    </nav>
  );

  const results = (
    <div className="guide-results" role="region" aria-label={t("guide.results")} aria-live="polite">
      {hits.length === 0 ? (
        <p className="guide-empty">{t("guide.noResults", { q: q.trim() })}</p>
      ) : (
        <ul>
          {hits.map((h, i) => (
            <li key={`${h.slug}#${h.anchor ?? ""}#${i}`}>
              <a
                href={guideHash({ slug: h.slug, anchor: h.anchor })}
                className="guide-hit"
                onClick={(e) => {
                  if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
                  e.preventDefault();
                  go({ slug: h.slug, anchor: h.anchor });
                }}
              >
                <span className="guide-hit-title">{h.anchor ? `${h.pageTitle} › ${h.heading}` : h.pageTitle}</span>
                <span className="guide-hit-snippet">{h.snippet}</span>
              </a>
            </li>
          ))}
        </ul>
      )}
    </div>
  );

  // Long pages start with a list of their sections (the h2 headings), after the title and the first paragraph.
  const sections = page ? page.headings.filter((h) => h.level === 2) : [];
  const lead = page ? Math.max(1, page.blocks.findIndex((b, i) => i > 0 && b.t !== "para")) : 0;
  const onThisPage = page && sections.length > ON_THIS_PAGE_MIN && (
    <nav className="guide-onpage" aria-label={t("guide.onThisPage")}>
      <p className="guide-onpage-title">{t("guide.onThisPage")}</p>
      <ul>
        {sections.map((h) => (
          <li key={h.id}>
            <a
              href={guideHash({ slug: page.slug, anchor: h.id })}
              onClick={(e) => {
                if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
                e.preventDefault();
                go({ slug: page.slug, anchor: h.id });
              }}
            >
              {h.text}
            </a>
          </li>
        ))}
      </ul>
    </nav>
  );

  const body = page ? (
    <article className="guide-page" aria-labelledby={page.headings[0] ? anchorDomId(page.headings[0].id) : undefined}>
      {page.needs === "machine" && (
        <p className="guide-page-tags">
          <NeedsTag />
        </p>
      )}
      {onThisPage ? (
        <>
          <Markdown blocks={page.blocks.slice(0, lead)} ctx={ctx} />
          {onThisPage}
          <Markdown blocks={page.blocks.slice(lead)} ctx={ctx} />
        </>
      ) : (
        <Markdown blocks={page.blocks} ctx={ctx} />
      )}
      {(prev || next) && (
        <nav className="guide-prevnext" aria-label={t("guide.prevNext")}>
          {prev ? (
            <a href={guideHash({ slug: prev.slug, anchor: null })} className="guide-prev" onClick={(e) => (e.preventDefault(), go({ slug: prev.slug, anchor: null }))}>
              <span className="guide-pn-label">
                <Icon name="prev" size={16} />
                {t("guide.prev")}
              </span>
              <span className="guide-pn-title">{prev.title}</span>
            </a>
          ) : (
            <span />
          )}
          {next && (
            <a href={guideHash({ slug: next.slug, anchor: null })} className="guide-next" onClick={(e) => (e.preventDefault(), go({ slug: next.slug, anchor: null }))}>
              <span className="guide-pn-label">
                {t("guide.next")}
                <Icon name="next" size={16} />
              </span>
              <span className="guide-pn-title">{next.title}</span>
            </a>
          )}
        </nav>
      )}
    </article>
  ) : (
    <div className="guide-page">
      <p className="guide-empty">{t("guide.notFound", { slug: p.route.slug })}</p>
      <p>
        <a href={guideHash({ slug: "index", anchor: null })} onClick={(e) => (e.preventDefault(), go({ slug: "index", anchor: null }))}>
          {t("guide.start")}
        </a>
      </p>
    </div>
  );

  return (
    <div className={`guide guide-mode-${p.mode}${wide ? " guide-wide" : ""}`} ref={root} onKeyDown={onKey}>
      <header className="guide-head">
        <div className="guide-bar">
          {/* Back: on a sheet always (at the first page it closes the guide); beside the canvas once a link was
              followed (a Dock or Home Screen app has no browser Back). */}
          {(p.mode === "sheet" || (p.mode === "pane" && guideDepth() > 1)) && (
            <button type="button" className="icon-btn guide-back" title={t("guide.back")} aria-label={t("guide.back")} onClick={backGuide}>
              <Icon name="arrowLeft" />
            </button>
          )}
          <h2 className="guide-bar-title">
            <Icon name="book" size={18} />
            <span className="guide-bar-name">{t("guide.title")}</span>
            {page && page.slug !== "index" && (
              <span className="guide-bar-page">
                <span aria-hidden="true">› </span>
                {page.title}
              </span>
            )}
          </h2>
          {!wide && (
            <button
              type="button"
              className={`icon-btn${toc ? " active" : ""}`}
              title={t("guide.contents")}
              aria-label={t("guide.contents")}
              aria-pressed={toc}
              onClick={() => setToc((v) => !v)}
            >
              <Icon name="menu" />
            </button>
          )}
          {p.newTab && p.mode !== "page" && (
            <a className="icon-btn" href={standaloneUrl(p.route)} target="_blank" rel="noopener noreferrer" title={t("guide.openTab")} aria-label={t("guide.openTab")}>
              <Icon name="web" />
            </a>
          )}
          {p.mode !== "page" && (
            <button type="button" className="icon-btn" title={t("guide.close")} aria-label={t("guide.close")} onClick={closeGuide}>
              <Icon name="close" />
            </button>
          )}
        </div>
        <div className="guide-search">
          <Icon name="search" size={16} />
          <input type="search" value={q} placeholder={t("guide.search")} aria-label={t("guide.search")} onChange={(e) => setQ(e.target.value)} />
        </div>
      </header>
      <div className="guide-scroll" ref={scroller}>
        {wide ? (
          <div className="guide-columns">
            {contentsList}
            {q.trim() ? results : body}
          </div>
        ) : q.trim() ? (
          results
        ) : showToc ? (
          contentsList
        ) : (
          body
        )}
      </div>
      {zoom &&
        createPortal(
          // The picture full size, over everything but STOP: fitted to the screen, or at its own size (scroll, and on a
          // touch screen pinch) after a click on it. Esc, Close or a click beside it closes it.
          <div
            className={`guide-lightbox${zoomActual ? " actual" : ""}`}
            role="dialog"
            aria-modal="true"
            aria-label={zoom.alt}
            onKeyDown={(e) => {
              if (e.key !== "Escape") return;
              e.stopPropagation();
              setZoom(null);
            }}
            onClick={(e) => e.target === e.currentTarget && setZoom(null)}
          >
            <button type="button" className="icon-btn guide-lightbox-close" title={t("common.close")} aria-label={t("common.close")} onClick={() => setZoom(null)} autoFocus>
              <Icon name="close" />
            </button>
            <img src={zoom.url} alt={zoom.alt} onClick={() => setZoomActual((v) => !v)} title={zoomActual ? t("guide.zoomFit") : t("guide.zoomActual")} />
          </div>,
          document.body,
        )}
    </div>
  );
}

/** Pages with more sections than this start with an "On this page" list. */
const ON_THIS_PAGE_MIN = 6;

function NeedsTag() {
  const t = useT();
  return (
    <span className="guide-tag" title={t("guide.needsLaserTitle")}>
      <Icon name="laser" size={12} />
      {t("guide.needsLaser")}
    </span>
  );
}
