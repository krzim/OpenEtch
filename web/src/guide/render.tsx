// Guide markdown (markdown.ts) to React elements. Every piece of text goes in as a React text child, so React escapes
// it: there is no dangerouslySetInnerHTML and no HTML string anywhere. Links: other guide pages and anchors navigate
// inside the viewer; http(s) and mailto open in a new tab (noopener noreferrer); anything else is shown as plain text.
// Images resolve through the bundle (content.ts) and load lazily.

import { Fragment, type ReactNode } from "react";
import { imageName, linkTarget, type Block, type Inline } from "./markdown.ts";
import { guideHash, type GuideRoute } from "./route.ts";

export interface RenderCtx {
  /** The page being shown (for `#anchor` links). */
  slug: string;
  /** Go to a guide page / anchor inside the viewer. */
  go: (r: GuideRoute) => void;
  /** The bundled URL of docs/guide/img/<name>. */
  image: (name: string) => string | undefined;
  /** "(opens in a new tab)", for screen readers. */
  newTab: string;
  /** "Image not found: {name}". */
  missingImage: (name: string) => string;
  /** The picture's pixel size (docs/guide/img-sizes.json), so it takes its room before it loads. */
  imageSize?: (name: string) => readonly [number, number] | undefined;
  /** Show a picture full size (the viewer's lightbox); without it pictures are not buttons. */
  zoom?: (url: string, alt: string) => void;
  /** "Show the picture full size", for the picture's button. */
  zoomLabel?: string;
}

/** The DOM id of a heading anchor (prefixed: the guide shares the document with the app). */
export const anchorDomId = (anchor: string) => `guide-${anchor}`;

function inlines(c: readonly Inline[], ctx: RenderCtx): ReactNode[] {
  return c.map((n, i) => <Fragment key={i}>{inline(n, ctx)}</Fragment>);
}

function inline(n: Inline, ctx: RenderCtx): ReactNode {
  switch (n.t) {
    case "text":
      return n.v;
    case "code":
      return <code>{n.v}</code>;
    case "strong":
      return <strong>{inlines(n.c, ctx)}</strong>;
    case "em":
      return <em>{inlines(n.c, ctx)}</em>;
    case "del":
      return <del>{inlines(n.c, ctx)}</del>;
    case "br":
      return <br />;
    case "img": {
      const name = imageName(n.src);
      const url = name ? ctx.image(name) : undefined;
      if (!url) return <span className="guide-img-missing">{ctx.missingImage(name ?? n.src)}</span>;
      const size = name ? ctx.imageSize?.(name) : undefined;
      const img = <img className="guide-img" src={url} alt={n.alt} width={size?.[0]} height={size?.[1]} loading="lazy" decoding="async" />;
      const zoom = ctx.zoom;
      if (!zoom) return img;
      return (
        <button type="button" className="guide-img-btn" title={ctx.zoomLabel} aria-label={ctx.zoomLabel ? `${ctx.zoomLabel}: ${n.alt}` : n.alt} onClick={() => zoom(url, n.alt)}>
          {img}
        </button>
      );
    }
    case "link": {
      const to = linkTarget(n.href);
      const kids = inlines(n.c, ctx);
      if (to.kind === "external")
        return (
          <a href={to.href} target="_blank" rel="noopener noreferrer" className="guide-ext">
            {kids}
            <span className="guide-sr"> {ctx.newTab}</span>
          </a>
        );
      if (to.kind === "page" || to.kind === "anchor") {
        const r: GuideRoute = to.kind === "page" ? { slug: to.slug, anchor: to.anchor } : { slug: ctx.slug, anchor: to.anchor };
        return (
          <a
            href={guideHash(r)}
            onClick={(e) => {
              if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
              e.preventDefault();
              ctx.go(r);
            }}
          >
            {kids}
          </a>
        );
      }
      return <span className="guide-badlink">{kids}</span>;
    }
  }
}

function list(b: Extract<Block, { t: "list" }>, ctx: RenderCtx): ReactNode {
  const items = b.items.map((it, i) => (
    <li key={i}>
      {/* A tight list's paragraphs are bare text (no paragraph spacing inside the item). */}
      {b.tight ? it.map((x, k) => (x.t === "para" ? <Fragment key={k}>{inlines(x.c, ctx)}</Fragment> : <Fragment key={k}>{block(x, ctx)}</Fragment>)) : blocks(it, ctx)}
    </li>
  ));
  return b.ordered ? <ol start={b.start !== 1 ? b.start : undefined}>{items}</ol> : <ul>{items}</ul>;
}

function block(b: Block, ctx: RenderCtx): ReactNode {
  switch (b.t) {
    case "heading": {
      const Tag = `h${Math.min(6, b.level)}` as "h1";
      return (
        <Tag id={anchorDomId(b.id)} tabIndex={-1}>
          {inlines(b.c, ctx)}
        </Tag>
      );
    }
    case "para":
      return <p>{inlines(b.c, ctx)}</p>;
    case "code":
      return (
        <pre className="guide-code">
          <code>{b.v}</code>
        </pre>
      );
    case "list":
      return list(b, ctx);
    case "quote":
      return b.kind ? (
        <aside className={`guide-callout guide-callout-${b.kind}`} role="note">
          {blocks(b.c, ctx)}
        </aside>
      ) : (
        <blockquote>{blocks(b.c, ctx)}</blockquote>
      );
    case "table":
      return (
        <div className="guide-table-wrap">
          <table className="guide-table">
            <thead>
              <tr>
                {b.head.map((c, i) => (
                  <th key={i} style={b.align[i] ? { textAlign: b.align[i]! } : undefined}>
                    {inlines(c, ctx)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {b.rows.map((r, k) => (
                <tr key={k}>
                  {r.map((c, i) => (
                    <td key={i} style={b.align[i] ? { textAlign: b.align[i]! } : undefined}>
                      {inlines(c, ctx)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case "hr":
      return <hr />;
  }
}

export function blocks(bs: readonly Block[], ctx: RenderCtx): ReactNode[] {
  return bs.map((b, i) => <Fragment key={i}>{block(b, ctx)}</Fragment>);
}

/** A page body: its blocks, the `# Title` included. */
export function Markdown(p: { blocks: readonly Block[]; ctx: RenderCtx }) {
  return <div className="guide-md">{blocks(p.blocks, p.ctx)}</div>;
}
