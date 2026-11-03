// The user guide's pages (docs/guide/*.md) and screenshots (docs/guide/shots.json, docs/guide/img/): front matter,
// anchors on every ## and ### heading, links to pages and anchors that exist, pictures that are listed and taken,
// shots that some page uses, no raw HTML, and index.md's contents up to date. Reads the files with fs.

import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { DEVICES } from "../../e2e/devices.mjs";
import { shotsProblems } from "../../e2e/guideSteps.mjs";
import { collectRefs, imageName, linkTarget, type Block, type Inline } from "./markdown.ts";
import { anchorsOf, buildGuide, withContents } from "./pages.ts";

const repo = fileURLToPath(new URL("../../../", import.meta.url));
const dir = join(repo, "docs", "guide");
const files = Object.fromEntries(
  readdirSync(dir)
    .filter((f) => f.endsWith(".md") && f !== "README.md")
    .map((f) => [f, readFileSync(join(dir, f), "utf8")]),
);
const guide = buildGuide(files);
const shots: { name: string }[] = JSON.parse(readFileSync(join(dir, "shots.json"), "utf8"));
const shotNames = new Set(shots.map((s) => s.name));
const imgFiles = existsSync(join(dir, "img")) ? readdirSync(join(dir, "img")) : [];

/** The source without fenced code and code spans (raw-HTML check). */
function prose(src: string): string {
  return src.replace(/^ {0,3}(`{3,}|~{3,})[\s\S]*?^ {0,3}\1[ \t]*$/gm, "").replace(/(`+)[\s\S]*?\1/g, "");
}

test("the guide has a start page and every page's front matter is complete", () => {
  assert.ok(guide.bySlug.has("index"), "docs/guide/index.md is the start page");
  for (const p of guide.pages) assert.deepEqual(p.problems, [], `${p.slug}.md`);
});

test("every page has one # heading (its title) and an anchor on every ## and ### heading", () => {
  for (const p of guide.pages) {
    const h1 = p.headings.filter((h) => h.level === 1);
    assert.equal(h1.length, 1, `${p.slug}.md: exactly one # heading`);
    assert.equal(p.blocks[0]?.t, "heading", `${p.slug}.md: the # heading comes first`);
    assert.equal(h1[0].text, p.title, `${p.slug}.md: the # heading is the title`);
    const seen = new Set<string>();
    for (const b of p.blocks) {
      if (b.t !== "heading" || b.level === 1) continue;
      if (b.level <= 3) assert.ok(b.explicit, `${p.slug}.md: "${b.id}" needs an anchor: ## Heading {#kebab-case-id}`);
      if (b.explicit) assert.match(b.id, /^[a-z0-9]+(-[a-z0-9]+)*$/, `${p.slug}.md: anchor {#${b.id}} is not kebab-case`);
      assert.ok(!seen.has(b.id), `${p.slug}.md: anchor {#${b.id}} is used twice`);
      seen.add(b.id);
    }
    // An implicit slug ("plain-heading") could clash with an explicit one: keep them apart.
    assert.equal(anchorsOf(p).size, p.headings.length, `${p.slug}.md: heading ids are unique`);
  }
});

test("no raw HTML in the pages (keys are code spans: `Ctrl+Z`)", () => {
  for (const [f, src] of Object.entries(files)) {
    const m = /<\/?[A-Za-z][\w-]*(\s[^>]*)?\/?>|<!--/.exec(prose(src));
    assert.equal(m, null, `${f}: raw HTML ${m?.[0]}`);
  }
});

test("links go to pages and anchors that exist, or to http(s) / mailto", () => {
  for (const p of guide.pages) {
    for (const href of collectRefs(p.blocks).links) {
      const to = linkTarget(href);
      const at = `${p.slug}.md: link (${href})`;
      assert.notEqual(to.kind, "invalid", `${at}: use other-page.md#anchor, #anchor or https://`);
      if (to.kind === "page") {
        const q = guide.bySlug.get(to.slug);
        assert.ok(q, `${at}: no docs/guide/${to.slug}.md`);
        if (to.anchor) assert.ok(anchorsOf(q).has(to.anchor), `${at}: no {#${to.anchor}} in ${to.slug}.md`);
      } else if (to.kind === "anchor") assert.ok(anchorsOf(p).has(to.anchor), `${at}: no {#${to.anchor}} on this page`);
    }
  }
});

test("every picture is in img/, listed in shots.json, with alt text", () => {
  for (const p of guide.pages) {
    const walk = (bs: Block[]): { src: string; alt: string }[] =>
      bs.flatMap((b) => {
        const inl = (c: Inline[]): { src: string; alt: string }[] =>
          c.flatMap((n) => (n.t === "img" ? [{ src: n.src, alt: n.alt }] : "c" in n ? inl(n.c) : []));
        if (b.t === "heading" || b.t === "para") return inl(b.c);
        if (b.t === "list") return b.items.flatMap(walk);
        if (b.t === "quote") return walk(b.c);
        if (b.t === "table") return [b.head, ...b.rows].flatMap((r) => r.flatMap(inl));
        return [];
      });
    for (const img of walk(p.blocks)) {
      const at = `${p.slug}.md: ![${img.alt}](${img.src})`;
      const name = imageName(img.src);
      assert.ok(name, `${at}: pictures are img/<kebab-case-name>.webp`);
      const shot = name.replace(/\.[a-z]+$/, "");
      assert.ok(shotNames.has(shot), `${at}: "${shot}" is not in docs/guide/shots.json`);
      assert.ok(imgFiles.includes(name), `${at}: docs/guide/img/${name} does not exist yet (npm run guide:shots -- --only ${shot})`);
      assert.ok(img.alt.trim().length >= 10 && img.alt !== shot, `${at}: alt text must say what the picture shows`);
    }
  }
});

test("shots.json is valid, every shot is used by a page and every file in img/ is a shot", () => {
  const ex = readFileSync(join(repo, "crates", "commands", "src", "builtin", "examples.rs"), "utf8");
  const examples = [...ex.slice(ex.indexOf("pub const EXAMPLES")).matchAll(/^\s+id: "([a-z0-9-]+)",/gm)].map((m) => m[1]);
  assert.ok(examples.includes("two-tone-coin"), "the built-in examples are read from examples.rs");
  assert.deepEqual(shotsProblems(shots, { devices: DEVICES.map((d: { id: string }) => d.id), examples }), []);
  const used = new Set(Object.values(files).flatMap((src) => [...src.matchAll(/\]\(img\/([a-z0-9-]+)\.[a-z]+\)/g)].map((m) => m[1])));
  for (const s of shots) assert.ok(used.has(s.name), `shot "${s.name}" is not used by any page`);
  for (const f of imgFiles) assert.ok(shotNames.has(f.replace(/\.[a-z]+$/, "")), `docs/guide/img/${f} is not a shot in shots.json`);
});

test("index.md's contents list is up to date (npm run guide:index)", () => {
  const src = files["index.md"].replace(/\r\n?/g, "\n");
  assert.equal(src, withContents(src, guide), "run npm run guide:index");
});
