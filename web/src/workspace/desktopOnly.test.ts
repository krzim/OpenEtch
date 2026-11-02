// Unit tests for the phone's desktop features (workspace/desktopOnly.ts): which tools, menu items and command search
// entries the phone leaves out, and that a desktop-only tool that is active keeps its button.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { MenuItem } from "../components/Menu.tsx";
import { ACTION_BY_ID } from "./hotkeys.ts";
import { actionOffered, DESKTOP_ACTION_IDS, DESKTOP_FEATURES, DESKTOP_TOOLS, desktopFeatures, forLayout, isDesktopOnly, toolOffered } from "./desktopOnly.ts";

const noop = () => {};
const item = (label: string, extra: Partial<Extract<MenuItem, { label: string }>> = {}): MenuItem => ({ label, action: noop, ...extra });
const labels = (items: MenuItem[]): string[] => items.map((i) => (i.sep ? "---" : i.label));

test("the plan's off-the-phone list, and only the phone leaves it out", () => {
  for (const f of ["nodes", "pen", "polygon", "trim", "weld", "docking", "artLibrary", "devices", "console"] as const) assert.ok(DESKTOP_FEATURES.includes(f), f);
  for (const f of ["addTabs", "userTools"] as const) assert.ok(DESKTOP_FEATURES.includes(f), `main's ${f}`);
  assert.equal(desktopFeatures("phone"), false);
  for (const l of ["wide", "compact", "tablet"] as const) assert.equal(desktopFeatures(l), true, l);
  for (const id of DESKTOP_ACTION_IDS) assert.ok(ACTION_BY_ID.has(id), `${id} is a hotkeys.ts action`);
});

test("desktop-only tools: off the phone's strip unless active; every tool elsewhere", () => {
  for (const tool of DESKTOP_TOOLS) {
    assert.equal(toolOffered("phone", tool, "select"), false, tool);
    assert.equal(toolOffered("phone", tool, tool), true, `${tool} while active (to switch it off)`);
    assert.equal(toolOffered("tablet", tool, "select"), true, tool);
  }
  for (const tool of ["rect", "ellipse", "circle", "line", "text", "measure"]) assert.equal(toolOffered("phone", tool, "select"), true, tool);
});

test("command search on the phone leaves out the desktop-only actions", () => {
  assert.equal(actionOffered("phone", "tool.nodes"), false);
  assert.equal(actionOffered("phone", "tool.trim"), false);
  assert.equal(actionOffered("phone", "window.devices"), false);
  assert.equal(actionOffered("phone", "tool.rect"), true);
  assert.equal(actionOffered("phone", undefined), true, "menu items without an action id");
  assert.equal(actionOffered("wide", "tool.nodes"), true);
});

test("forLayout drops flagged and desktop-only items, empty submenus and stray separators on the phone only", () => {
  const items: MenuItem[] = [
    item("Edit nodes", { actionId: "tool.nodes" }),
    { sep: true },
    item("Weld", { actionId: "tool.weld" }),
    item("Union", { actionId: "tool.union" }),
    { sep: true },
    { sep: true },
    item("Desktop", { submenu: [item("Art library", { actionId: "window.artLibrary" }), { sep: true }, item("Add tabs", { desktopOnly: true })] }),
    item("Mixed", { submenu: [item("Dogbone"), item("Manage", { desktopOnly: true })] }),
    { sep: true },
    item("Trim", { actionId: "tool.trim" }),
  ];
  assert.deepEqual(labels(forLayout(items, "wide")), labels(items), "unchanged off the phone");
  const phone = forLayout(items, "phone");
  assert.deepEqual(labels(phone), ["Union", "---", "Mixed"]);
  const mixed = phone[2];
  assert.ok(!mixed.sep && mixed.submenu);
  assert.deepEqual(labels(mixed.submenu ?? []), ["Dogbone"]);
  assert.equal(isDesktopOnly({ sep: true }), false);
  assert.equal(isDesktopOnly(item("x", { desktopOnly: true })), true);
});
