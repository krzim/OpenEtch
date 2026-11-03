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

test("headings: GitHub's ids, made from the text; duplicates numbered", () => {
  const ids = (src: string) => parseBlocks(src).flatMap((x) => (x.t === "heading" ? [x.id] : []));
  const b = parseBlocks("# Title\n\n## Cut settings\n\n## Plain heading\n\n## Plain heading\n");
  assert.deepEqual(
    b.map((x) => (x.t === "heading" ? [x.level, x.id, inlineText(x.c)] : null)),
    [
      [1, "title", "Title"],
      [2, "cut-settings", "Cut settings"],
      [2, "plain-heading", "Plain heading"],
      [2, "plain-heading-1", "Plain heading"],
    ],
  );
  // Lower case; punctuation other than "-" and "_" removed; each space a hyphen, nothing collapsed or trimmed.
  assert.equal(slugify("Layers & materials: 2-tone!"), "layers--materials-2-tone");
  assert.equal(slugify("The Cuts / Layers window"), "the-cuts--layers-window");
  assert.equal(slugify("3D models (STL, OBJ) and snake_case"), "3d-models-stl-obj-and-snake_case");
  assert.equal(slugify("C++ & C#? Yes."), "c--c-yes");
  // Letters of any script stay, lower-cased; symbols such as emoji go (their spaces stay).
  assert.equal(slugify("Café Über GRÖSSE"), "café-über-grösse");
  assert.equal(slugify("Done 🎉 now"), "done--now");
  // What counts is the rendered text: code spans, emphasis and link text, escapes resolved; not an image's alt text.
  assert.deepEqual(ids("## The `Ctrl+Z` key and *undo* in [the menu](x.md)\n## \\*Starred\\* \\#1\n## Logo ![A logo](img/a.webp) here\n"), [
    "the-ctrlz-key-and-undo-in-the-menu",
    "starred-1",
    "logo--here",
  ]);
  // `{#custom-id}` is not markdown on GitHub: it stays in the text (and so in the id).
  const custom = parseBlocks("## Cut settings {#cut}\n")[0];
  assert.ok(custom.t === "heading" && custom.id === "cut-settings-cut" && inlineText(custom.c) === "Cut settings {#cut}");
  // Duplicates as github-slugger numbers them: a heading that reads like a numbered one moves on to the next number.
  assert.deepEqual(ids("# A\n## A\n## A 1\n### A\n"), ["a", "a-1", "a-1-1", "a-2"]);
  // Closing #s are not text; a heading in a block quote ("hello-1", not listed: ids() reads the top level) counts
  // towards the numbering, as on GitHub.
  assert.deepEqual(ids("## Hello ##\n\n> ## Hello\n\n## Hello\n"), ["hello", "hello-2"]);
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
  // GitHub ids may start with a digit, hold "_", "--" or letters of any script (raw or percent-encoded).
  assert.deepEqual(linkTarget("files.md#3d-models--gerber_files"), { kind: "page", slug: "files", anchor: "3d-models--gerber_files" });
  assert.deepEqual(linkTarget("#café"), { kind: "anchor", anchor: "café" });
  assert.deepEqual(linkTarget("#caf%C3%A9"), { kind: "anchor", anchor: "café" });
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
