#!/usr/bin/env node
// Rewrite the contents list at the end of docs/guide/index.md (from "## Contents" on) from the pages'
// front matter, so the guide reads in order on GitHub too. `npm run guide:index`; src/guide/docs.test.ts fails while
// it is out of date.

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildGuide, withContents } from "../src/guide/pages.ts";

const dir = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "docs", "guide");
const files = Object.fromEntries(readdirSync(dir).filter((f) => f.endsWith(".md")).map((f) => [f, readFileSync(join(dir, f), "utf8")]));
const index = join(dir, "index.md");
const before = readFileSync(index, "utf8");
const after = withContents(before, buildGuide(files));
if (after !== before.replace(/\r\n?/g, "\n")) {
  writeFileSync(index, after);
  console.log("docs/guide/index.md: contents rewritten");
} else console.log("docs/guide/index.md: contents up to date");
