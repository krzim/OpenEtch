// The guide model (pages.ts), its route (route.ts) and the screenshot step language (e2e/guideSteps.mjs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { parseKey, shotsProblems, stepProblem } from "../../e2e/guideSteps.mjs";
import { buildGuide, contents, contentsMarkdown, prevNext, searchGuide, sectionsOf, slugOfPath, visiblePages, withContents } from "./pages.ts";
import { guideHash, guideMode, guidePaneWidth, parseGuideHash, parseTarget, standaloneUrl } from "./route.ts";

const page = (title: string, order: number, section: string, needs = "design", body = "") =>
  `---\ntitle: ${title}\norder: ${order}\nsection: ${section}\nneeds: ${needs}\nsummary: About ${title.toLowerCase()}.\n---\n# ${title}\n\n${body}`;

const g = buildGuide({
  "../../../docs/guide/index.md": page("Start", 0, "Getting started"),
  "../../../docs/guide/frame.md": page("Frame a job", 10, "Running jobs", "machine", "## Frame {#frame}\n\nThe red pointer traces the outline.\n"),
  "../../../docs/guide/fit.md": page("Fit to a coin", 20, "Designing", "design", "## Centre it {#centre}\n\nUse **Centre on stock** to centre the design.\n\n### Size {#size}\n\nSet the width.\n"),
  "../../../docs/guide/text.md": page("Text", 10, "Designing"),
  "../../../docs/guide/README.md": "# Not a page",
});

test("pages: slugs from paths, sorted by section then order, README left out", () => {
  assert.equal(slugOfPath("../../../docs/guide/fit.md"), "fit");
  assert.equal(slugOfPath("docs\\guide\\README.md"), null);
  assert.deepEqual(
    g.pages.map((p) => p.slug),
    ["index", "text", "fit", "frame"],
  );
  assert.deepEqual(g.bySlug.get("frame")?.problems, []);
  const bad = buildGuide({ "x.md": "---\ntitle: X\nsection: Nope\ncolour: red\n---\n# X" }).pages[0];
  assert.ok(bad.problems.some((p) => /order/.test(p)));
  assert.ok(bad.problems.some((p) => /section "Nope"/.test(p)));
  assert.ok(bad.problems.some((p) => /needs/.test(p)));
  assert.ok(bad.problems.some((p) => /unknown front matter key "colour"/.test(p)));
});

test("contents, prev / next, and the design-only build without needs: machine", () => {
  assert.deepEqual(
    contents(g, false).map((s) => [s.section, s.pages.map((p) => p.slug)]),
    [
      ["Getting started", ["index"]],
      ["Designing", ["text", "fit"]],
      ["Running jobs", ["frame"]],
    ],
  );
  assert.deepEqual(contents(g, true).map((s) => s.section), ["Getting started", "Designing"]);
  assert.deepEqual(visiblePages(g, true).map((p) => p.slug), ["index", "text", "fit"]);
  const pn = prevNext(g, "fit", false);
  assert.equal(pn.prev?.slug, "text");
  assert.equal(pn.next?.slug, "frame");
  assert.equal(prevNext(g, "fit", true).next, null);
});

test("search: titles, headings and text; every word must match; headings rank higher", () => {
  const s = sectionsOf(g.bySlug.get("fit")!);
  assert.deepEqual(
    s.map((x) => [x.anchor, x.heading]),
    [
      [null, "Fit to a coin"],
      ["centre", "Centre it"],
      ["size", "Size"],
    ],
  );
  const hits = searchGuide(g, "centre", false);
  assert.equal(hits[0].anchor, "centre");
  assert.match(hits[0].snippet, /Centre on stock/);
  assert.equal(searchGuide(g, "centre pointer", false).length, 0);
  assert.equal(searchGuide(g, "red pointer", false)[0].slug, "frame");
  assert.equal(searchGuide(g, "red pointer", true).length, 0, "the design-only build does not find machine pages");
  assert.deepEqual(searchGuide(g, "  ", false), []);
  // Accents and case do not matter.
  assert.equal(searchGuide(g, "CÉNTRE", false)[0].anchor, "centre");
});

test("the start page's contents list", () => {
  const md = contentsMarkdown(g);
  assert.match(md, /^## Contents \{#contents\}\n\n\*\*Designing\*\*\n\n- \[Text\]\(text\.md\): About text\.\n- \[Fit to a coin\]\(fit\.md\)/);
  assert.match(md, /\[Frame a job\]\(frame\.md\): About frame a job\. \(Needs a laser\)/);
  assert.ok(!md.includes("(index.md)"));
  const idx = "# Start\n\nHello.\n\n## Contents {#contents}\n\nold\n";
  assert.equal(withContents(idx, g), `# Start\n\nHello.\n\n${md}`);
  assert.equal(withContents("# Start\n", g), `# Start\n\n${md}`);
});

test("routes: #/guide, #/guide/slug, #/guide/slug#anchor", () => {
  assert.deepEqual(parseGuideHash("#/guide"), { slug: "index", anchor: null });
  assert.deepEqual(parseGuideHash("#/guide/"), { slug: "index", anchor: null });
  assert.deepEqual(parseGuideHash("#/guide#open-the-guide"), { slug: "index", anchor: "open-the-guide" });
  assert.deepEqual(parseGuideHash("#/guide/fit-to-coin"), { slug: "fit-to-coin", anchor: null });
  assert.deepEqual(parseGuideHash("#/guide/fit-to-coin#size"), { slug: "fit-to-coin", anchor: "size" });
  for (const no of ["", "#", "#/cockpit", "#/guidebook", "#/guide/Bad Slug", "#/guide/a/b", "#/guide/x#"]) assert.equal(parseGuideHash(no), null, no);
  assert.equal(guideHash({ slug: "index", anchor: null }), "#/guide");
  assert.equal(guideHash({ slug: "index", anchor: "a" }), "#/guide#a");
  assert.equal(guideHash({ slug: "fit", anchor: "size" }), "#/guide/fit#size");
  for (const r of [{ slug: "fit", anchor: "size" }, { slug: "index", anchor: null }, { slug: "index", anchor: "x" }]) assert.deepEqual(parseGuideHash(guideHash(r)), r);
  assert.deepEqual(parseTarget("layers#cut-settings"), { slug: "layers", anchor: "cut-settings" });
  assert.deepEqual(parseTarget("#getting-help"), { slug: "index", anchor: "getting-help" });
  assert.equal(parseTarget("javascript:alert(1)"), null);
  assert.equal(standaloneUrl({ slug: "fit", anchor: "size" }, { origin: "http://openetch.local", pathname: "/" }), "http://openetch.local/?guide#/guide/fit#size");
});

test("where the guide shows: beside the canvas when there is room, else a sheet", () => {
  assert.equal(guideMode("phone", 1200), "sheet");
  assert.equal(guideMode("tablet", 820), "sheet");
  assert.equal(guideMode("tablet", 1180), "pane");
  assert.equal(guideMode("compact", 1470), "pane");
  assert.equal(guidePaneWidth(1000), 340);
  assert.equal(guidePaneWidth(1470), 412);
  assert.equal(guidePaneWidth(2560), 460);
});

test("shot steps: one action each, private paths and file commands refused", () => {
  const ok = [
    { exec: "layer.update", params: { id: "layer-1", speed: 1000 } },
    { click: "Cuts / Layers" },
    { click: { css: "tr.cuts-row", text: "Rim" }, in: ".panel-right" },
    { dblclick: { object: "Name" } },
    { context: "selection" },
    { menu: ["File", "Laser", "Devices"] },
    { type: "Grace Hopper", into: { css: "#tpl-name" } },
    { key: "Ctrl+Shift+Z" },
    { hash: "#/guide/example#tables" },
    { wait: ".guide-md" },
    { wait: 500 },
    { select: "all" },
    { select: ["Name", "obj-3"] },
    { workspace: "laser" },
    { tab: "Layers", sheet: "half" },
    { escape: true },
    { scroll: ".guide-toc" },
  ];
  for (const s of ok) assert.equal(stepProblem(s), null, JSON.stringify(s));
  const bad: [unknown, RegExp][] = [
    [{}, /exactly one/],
    [{ click: "A", key: "B" }, /exactly one/],
    [{ click: "A", nth: 2 }, /unknown option nth/],
    [{ exec: "doc.load", params: {} }, /reads or writes files/],
    [{ exec: "art.import_svg" }, /reads or writes files/],
    [{ exec: "shape.text", params: { text: "C:\\Users\\someone\\x" } }, /file path/],
    [{ type: "/home/someone/designs" }, /file path/],
    [{ type: "http://172.16.0.10:8080" }, /LAN address/],
    [{ exec: "layer.update", params: { name: "logo.lbrn2" } }, /file path/],
    [{ wait: 60000 }, /at most/],
    [{ key: "Hyper+Q" }, /not a key/],
    [{ hash: "guide" }, /hash/],
    [{ tab: "Settings" }, /phone tabs/],
    [{ workspace: "cam" }, /design/],
  ];
  for (const [s, re] of bad) assert.match(stepProblem(s) ?? "", re, JSON.stringify(s));
  assert.deepEqual(parseKey("Ctrl+Shift+Z"), { key: "Z", modifiers: 10 });
  assert.deepEqual(parseKey("Escape"), { key: "Escape", modifiers: 0 });
  assert.deepEqual(parseKey("Ctrl++"), { key: "+", modifiers: 2 });
  const env = { devices: ["iphone15-portrait"], examples: ["two-tone-coin"] };
  assert.deepEqual(shotsProblems([{ name: "a-b", device: "iphone15-portrait", example: "two-tone-coin", steps: [] }], env), []);
  const probs = shotsProblems([{ name: "A B", device: "nokia", example: "my-file", steps: [{}], doc: "x.lbrn2" }], env);
  for (const re of [/kebab-case/, /device/, /example/, /step 1/, /unknown key "doc"/]) assert.ok(probs.some((p) => re.test(p)), `${re}: ${probs.join("; ")}`);
});
