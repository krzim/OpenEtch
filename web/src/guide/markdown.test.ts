// The guide's markdown parser (markdown.ts): front matter, blocks, inline markup, links and images.

import { test } from "node:test";
import assert from "node:assert/strict";
import { blocksText, collectRefs, imageName, inlineText, linkTarget, parseBlocks, parseDoc, parseInline, slugify, splitFrontMatter, tableCells } from "./markdown.ts";

test("front matter: key: value lines, quotes stripped, body after the fence", () => {
  const { meta, body } = splitFrontMatter('---\ntitle: "Fit a design"\norder: 3\nsection: Designing\n---\n# Fit\n');
  assert.deepEqual(meta, { title: "Fit a design", order: "3", section: "Designing" });
  assert.equal(body, "# Fit\n");
  assert.deepEqual(splitFrontMatter("# No front matter").meta, {});
  // CRLF files (Windows checkouts) parse the same.
  assert.equal(splitFrontMatter("---\r\ntitle: X\r\n---\r\nBody").meta.title, "X");
});

test("headings: explicit {#anchor} ids, GitHub slugs otherwise, duplicates numbered", () => {
  const b = parseBlocks("# Title\n\n## Cut settings {#cut-settings}\n\n## Plain heading\n\n## Plain heading\n");
  assert.deepEqual(
    b.map((x) => (x.t === "heading" ? [x.level, x.id, inlineText(x.c), x.explicit] : null)),
    [
      [1, "title", "Title", false],
      [2, "cut-settings", "Cut settings", true],
      [2, "plain-heading", "Plain heading", false],
      [2, "plain-heading-1", "Plain heading", false],
    ],
  );
  assert.equal(slugify("Layers & materials: 2-tone!"), "layers--materials-2-tone");
});

test("inline: bold, italic, code, strikethrough, escapes, nesting", () => {
  assert.deepEqual(parseInline("a **bold** and *it* and `co*de*` ~~x~~"), [
    { t: "text", v: "a " },
    { t: "strong", c: [{ t: "text", v: "bold" }] },
    { t: "text", v: " and " },
    { t: "em", c: [{ t: "text", v: "it" }] },
    { t: "text", v: " and " },
    { t: "code", v: "co*de*" },
    { t: "text", v: " " },
    { t: "del", c: [{ t: "text", v: "x" }] },
  ]);
  assert.deepEqual(parseInline("**bold with *italic* inside**"), [
    { t: "strong", c: [{ t: "text", v: "bold with " }, { t: "em", c: [{ t: "text", v: "italic" }] }, { t: "text", v: " inside" }] },
  ]);
  assert.deepEqual(parseInline("***both***"), [{ t: "strong", c: [{ t: "em", c: [{ t: "text", v: "both" }] }] }]);
  // Intraword underscores, lone stars and escaped stars stay text.
  assert.deepEqual(parseInline("snake_case_name 2 * 3 \\*not\\*"), [{ t: "text", v: "snake_case_name 2 * 3 *not*" }]);
  assert.deepEqual(parseInline("``a ` b``"), [{ t: "code", v: "a ` b" }]);
});

test("raw HTML is not markup: it stays text", () => {
  assert.deepEqual(parseInline("<kbd>Ctrl</kbd> <script>x</script>"), [{ t: "text", v: "<kbd>Ctrl</kbd> <script>x</script>" }]);
  const b = parseBlocks("<div onclick=alert(1)>hi</div>");
  assert.deepEqual(b, [{ t: "para", c: [{ t: "text", v: "<div onclick=alert(1)>hi</div>" }] }]);
});

test("links and images", () => {
  assert.deepEqual(parseInline("See [the **layers** page](layers.md#cut-settings) and ![A dialog](img/dialog.webp)."), [
    { t: "text", v: "See " },
    { t: "link", href: "layers.md#cut-settings", c: [{ t: "text", v: "the " }, { t: "strong", c: [{ t: "text", v: "layers" }] }, { t: "text", v: " page" }] },
    { t: "text", v: " and " },
    { t: "img", src: "img/dialog.webp", alt: "A dialog" },
    { t: "text", v: "." },
  ]);
  // A title is ignored; parentheses in a URL are balanced.
  assert.deepEqual(parseInline('[w](https://en.wikipedia.org/wiki/Fiber_(laser) "t")'), [
    { t: "link", href: "https://en.wikipedia.org/wiki/Fiber_(laser)", c: [{ t: "text", v: "w" }] },
  ]);
  // Not a link: brackets without a destination.
  assert.deepEqual(parseInline("[not a link] (x"), [{ t: "text", v: "[not a link] (x" }]);
});

test("link targets: pages, anchors, http(s) and mailto only", () => {
  assert.deepEqual(linkTarget("layers.md#cut-settings"), { kind: "page", slug: "layers", anchor: "cut-settings" });
  assert.deepEqual(linkTarget("./index.md"), { kind: "page", slug: "index", anchor: null });
  assert.deepEqual(linkTarget("#top"), { kind: "anchor", anchor: "top" });
  assert.deepEqual(linkTarget("https://example.com/a?b=c"), { kind: "external", href: "https://example.com/a?b=c" });
  assert.deepEqual(linkTarget("mailto:someone@example.com"), { kind: "external", href: "mailto:someone@example.com" });
  for (const bad of ["javascript:alert(1)", "JavaScript:alert(1)", "data:text/html,x", "../../README.md", "//evil.example", "file:///c:/x", "vbscript:x", "https://"])
    assert.equal(linkTarget(bad).kind, "invalid", bad);
  assert.equal(imageName("img/cuts-layers.webp"), "cuts-layers.webp");
  assert.equal(imageName("../img/x.webp"), null);
  assert.equal(imageName("https://example.com/x.png"), null);
  assert.equal(imageName("img/Bad Name.png"), null);
});

test("fenced code keeps its text verbatim", () => {
  const b = parseBlocks("```sh\nnpm run guide:shots -- --url x\n  <b>*not markup*</b>\n```\n\n~~~\nplain\n~~~");
  assert.deepEqual(b, [
    { t: "code", lang: "sh", v: "npm run guide:shots -- --url x\n  <b>*not markup*</b>" },
    { t: "code", lang: "", v: "plain" },
  ]);
});

test("lists: ordered with start, nested, tight and loose", () => {
  const b = parseBlocks("3. Open **File**.\n4. Pick a file:\n   - an SVG\n   - a PNG\n5. Done.\n");
  assert.equal(b.length, 1);
  const l = b[0];
  assert.ok(l.t === "list");
  assert.equal(l.ordered, true);
  assert.equal(l.start, 3);
  assert.equal(l.tight, true);
  assert.equal(l.items.length, 3);
  const nested = l.items[1][1];
  assert.ok(nested?.t === "list" && !nested.ordered && nested.items.length === 2);
  assert.equal(blocksText(nested.items[1]), "a PNG");

  const loose = parseBlocks("- one\n\n- two\n\n  more about two\n");
  assert.ok(loose[0].t === "list");
  assert.equal(loose[0].tight, false);
  assert.equal(loose[0].items[1].length, 2);

  // A different marker starts a new list; a paragraph line continues an item lazily.
  const two = parseBlocks("- a\nlazy\n* b\n");
  assert.equal(two.length, 2);
  assert.ok(two[0].t === "list");
  assert.equal(blocksText(two[0].items[0]), "a lazy");
});

test("pipe tables: header, alignment, escaped pipes, missing cells", () => {
  const b = parseBlocks("| Setting | Value |\n|:--|--:|\n| Speed | 1000 mm/s |\n| Pipe \\| char |\n");
  assert.equal(b.length, 1);
  const t = b[0];
  assert.ok(t.t === "table");
  assert.deepEqual(t.align, ["left", "right"]);
  assert.deepEqual(t.head.map(inlineText), ["Setting", "Value"]);
  assert.deepEqual(t.rows.map((r) => r.map(inlineText)), [
    ["Speed", "1000 mm/s"],
    ["Pipe | char", ""],
  ]);
  assert.deepEqual(tableCells("a | `b` | c"), ["a", "`b`", "c"]);
});

test("block quotes: callouts by their bold label, plain quotes otherwise", () => {
  const b = parseBlocks("> **Warning:** Wear eye protection.\n> Never leave a job.\n\n> Just a quote.\n\n> **Tip:**\n> Use `Ctrl+Z`.");
  assert.deepEqual(
    b.map((x) => (x.t === "quote" ? x.kind : x.t)),
    ["warning", null, "tip"],
  );
  assert.ok(b[0].t === "quote");
  assert.equal(blocksText(b[0].c), "Warning: Wear eye protection. Never leave a job.");
});

test("paragraphs, rules and hard breaks", () => {
  const b = parseBlocks("one\ntwo  \nthree\n\n---\n\nfour");
  assert.deepEqual(b, [
    { t: "para", c: [{ t: "text", v: "one two" }, { t: "br" }, { t: "text", v: "three" }] },
    { t: "hr" },
    { t: "para", c: [{ t: "text", v: "four" }] },
  ]);
});

test("parseDoc and collectRefs", () => {
  const d = parseDoc("---\ntitle: T\n---\n# T\n\n- [a](a.md)\n\n| x |\n|---|\n| ![i](img/i.webp) |\n\n> **Note:** see [b](https://example.com).");
  assert.equal(d.meta.title, "T");
  assert.deepEqual(collectRefs(d.blocks), { links: ["a.md", "https://example.com"], images: ["img/i.webp"] });
});
