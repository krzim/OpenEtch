// Tests for the wiring between the UX packages (docs/ux/ui-plan-2026-11.md, integration on ui-base): the action runner
// that menus and command search use, and the creation-rail preference that moved from the ribbon into prefs.ts.

import { test } from "node:test";
import assert from "node:assert/strict";
import { runKeymapAction } from "./useKeymap.ts";
import { DEFAULT_PREFS, getPrefs, sanitizePrefs, updatePrefs } from "./prefs.ts";
import { getCreationRail, migrateCreationRail, setCreationRail } from "../components/ribbonPrefs.ts";
import { canRunAction, runHotkeyAction, setActionRunner } from "../components/commands.ts";

test("runKeymapAction: runs the handler, falls back when it is missing, declines or the key map is blocked", () => {
  const calls: string[] = [];
  const handlers = {
    "view.zoomIn": (chord: string) => void calls.push(`zoom:${chord}`),
    "tool.rect": () => false as const,
  };
  assert.equal(runKeymapAction({ handlers, blocked: () => false }, "view.zoomIn"), true);
  assert.deepEqual(calls, ["zoom:"]);
  // A handler that declines (false) and a missing one leave the action to the key press route.
  assert.equal(runKeymapAction({ handlers, blocked: () => false }, "tool.rect"), false);
  assert.equal(runKeymapAction({ handlers, blocked: () => false }, "help.useAsApp"), false);
  // An App dialog is open: nothing runs.
  assert.equal(runKeymapAction({ handlers, blocked: () => true }, "view.zoomIn"), false);
  assert.deepEqual(calls, ["zoom:"]);
});

test("the registered runner lets command search run actions that have no key bound", () => {
  // help.useAsApp has no default chord: without a runner it cannot run.
  setActionRunner(null);
  assert.equal(canRunAction("help.useAsApp"), false);
  let ran = 0;
  setActionRunner((id) => runKeymapAction({ handlers: { "help.useAsApp": () => void ran++ }, blocked: () => false }, id));
  try {
    assert.equal(canRunAction("help.useAsApp"), true);
    assert.equal(runHotkeyAction("help.useAsApp"), true);
    assert.equal(ran, 1);
  } finally {
    setActionRunner(null);
  }
});

test("creation rail: a preference, off by default, sanitised", () => {
  assert.equal(DEFAULT_PREFS.creationRail, false);
  assert.equal(sanitizePrefs({ creationRail: "yes" }).creationRail, false);
  assert.equal(sanitizePrefs({ creationRail: true }).creationRail, true);
});

test("creation rail: toggled through the preferences, and an old ribbon key is carried over once", () => {
  const before = getPrefs().creationRail;
  try {
    setCreationRail(true);
    assert.equal(getPrefs().creationRail, true);
    assert.equal(getCreationRail(), true);
    setCreationRail(false);
    assert.equal(getCreationRail(), false);

    const store = new Map<string, string>([["openetch.ribbon.creationRail", "1"]]);
    const fake = { getItem: (k: string) => store.get(k) ?? null, removeItem: (k: string) => void store.delete(k) };
    migrateCreationRail(fake);
    assert.equal(getCreationRail(), true);
    assert.equal(store.has("openetch.ribbon.creationRail"), false);
    // Nothing left to carry over: a later change sticks.
    setCreationRail(false);
    migrateCreationRail(fake);
    assert.equal(getCreationRail(), false);
  } finally {
    updatePrefs({ creationRail: before });
  }
});
