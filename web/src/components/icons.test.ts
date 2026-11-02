// Unit tests for the one icon registry (components/ui.tsx ICONS): every icon name the touch HUD, the nudge pad, the
// timeline chips and the phone tabs ask for is registered, and no other file keeps an icon set of its own. ui.tsx is
// JSX, so the test reads the registry's names from its source.

import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { barActions } from "../touch/hudModel.ts";
import { timelineLabel } from "./timelineLabel.ts";

const here = fileURLToPath(new URL(".", import.meta.url));
const src = join(here, "..");
const uiSource = readFileSync(join(here, "ui.tsx"), "utf8");
const body = uiSource.slice(uiSource.indexOf("const ICONS"), uiSource.indexOf("export const ICON_NAMES"));
const NAMES = new Set([...body.matchAll(/^ {2}([A-Za-z0-9]+):/gm)].map((m) => m[1]));

test("the registry is parsed and has no duplicate names", () => {
  const all = [...body.matchAll(/^ {2}([A-Za-z0-9]+):/gm)].map((m) => m[1]);
  assert.ok(all.length > 80, `found ${all.length} icons`);
  assert.equal(new Set(all).size, all.length, "a name is registered twice");
});

test("the touch HUD's action bar icons are registered", () => {
  for (const a of [...barActions(2, false), ...barActions(1, true)]) assert.ok(NAMES.has(a.icon), a.icon);
});

test("the shell's icons (phone tabs, nudge pad, rows, theme, lock, link) are registered", () => {
  for (const n of ["pointer", "layers", "zap", "more", "menu", "pencil", "pin", "sliders", "monitor", "file", "history", "settings"]) assert.ok(NAMES.has(n), n);
  for (const n of ["arrowUp", "arrowDown", "arrowLeft", "arrowRight", "rotate", "grip"]) assert.ok(NAMES.has(n), n);
  for (const n of ["sun", "moon", "themeSystem", "lock", "unlock", "link", "fullscreen", "fullscreenExit"]) assert.ok(NAMES.has(n), n);
});

test("every timeline chip icon is registered", () => {
  const t = ((k: string) => k) as Parameters<typeof timelineLabel>[1];
  const step = (command: string, summary: string) => ({ index: 1, label: command, summary, commands: command, state: "active" }) as unknown as Parameters<typeof timelineLabel>[0];
  const cmds = [
    "shape.rect",
    "shape.ellipse",
    "shape.text",
    "text.edit",
    "object.transform",
    "object.delete",
    "object.duplicate",
    "object.rename",
    "object.set_layer",
    "layer.update",
    "group.create",
    "path.weld",
    "unknown.thing",
  ];
  for (const c of cmds)
    for (const sum of ["", "rotate 15° on o1", "scale 2 on o1", "mirror x on o1", "move [1, 2] on o1"]) {
      const icon = timelineLabel(step(c, sum), t).icon;
      assert.ok(NAMES.has(icon), `${c} "${sum}": ${icon}`);
    }
});

test("no other source file keeps an icon set of its own", () => {
  const offenders: string[] = [];
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (p.endsWith(".tsx") && !p.endsWith(`components${"\\"}ui.tsx`) && !p.endsWith("components/ui.tsx")) {
        const s = readFileSync(p, "utf8");
        if (/viewBox="0 0 24 24"/.test(s)) offenders.push(p.slice(src.length + 1));
      }
    }
  };
  walk(src);
  assert.deepEqual(offenders, []);
});
