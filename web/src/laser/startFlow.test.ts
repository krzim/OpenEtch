// Unit tests for laser/startFlow.ts: the gate matrix (enabled and the visible reason), the scope and time label, the
// hotkey decision (fire or open the checklist), and that Start is never enabled while offline or running.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { MachineStatus } from "../api/types.ts";
import { setRunOptions } from "../components/jobStore.ts";
import { getLaserFacts, getLaserFlow, resetLaserStore, setLaserFlow, setLaserSource } from "./laserContext.ts";
import {
  runUnderWay,
  sharedChecklistIncomplete,
  shortClock,
  startBlock,
  startCtxOf,
  startDecision,
  startFromHotkey,
  startFlow,
  startLabel,
  startScope,
  startState,
  type StartCtx,
  type StartSource,
} from "./startFlow.ts";

const CLICK = { mode: "click", pin_set: false } as const;
const PIN = { mode: "pin", pin_set: true } as const;

function machine(over: Partial<MachineStatus> = {}, approval: Record<string, unknown> = CLICK): MachineStatus {
  return { connected: true, busy: false, framing: false, running: false, runs: [], pending_runs: [], approval, ...over } as MachineStatus;
}

function ctx(over: Partial<StartCtx> = {}): StartCtx {
  return {
    source: "dock",
    machine: machine(),
    online: true,
    designOnly: null,
    workspace: "laser",
    selection: [],
    revision: 7,
    openLaser: () => undefined,
    notify: () => undefined,
    report: () => undefined,
    ...over,
  };
}

test("gate matrix: the first blocking gate, in the order a person fixes them", () => {
  resetLaserStore();
  assert.equal(startBlock(ctx()), null);
  assert.equal(startBlock(ctx({ online: false })), "offline");
  assert.equal(startBlock(ctx({ online: false, machine: null })), "offline");
  assert.equal(startBlock(ctx({ machine: null })), "loading");
  assert.equal(startBlock(ctx({ machine: machine({ shutting_down: true }) })), "shutdown");
  assert.equal(startBlock(ctx({ designOnly: "Design-only device" })), "designOnly");
  assert.equal(startBlock(ctx({ machine: machine({ connected: false }) })), "noLaser");
  assert.equal(startBlock(ctx({ machine: machine({ running: true }) })), "running");
  assert.equal(startBlock(ctx({ machine: machine({ job: { active: true } }) })), "running");
  assert.equal(startBlock(ctx({ machine: machine({ runs: [{ id: "r1", state: "running" }] }) })), "running");
  assert.equal(startBlock(ctx({ machine: machine({ framing: true }) })), "framing");
  assert.equal(startBlock(ctx({ checklistIncomplete: true })), "checklist");
  // Offline beats everything else; running beats framing.
  assert.equal(startBlock(ctx({ online: false, machine: machine({ running: true, framing: true }) })), "offline");
  assert.equal(startBlock(ctx({ machine: machine({ running: true, framing: true }) })), "running");
});

test("the checklist only gates a no-PIN Start (with a PIN it belongs to the approval step)", () => {
  resetLaserStore();
  assert.equal(startBlock(ctx({ machine: machine({}, PIN), checklistIncomplete: true })), null);
  assert.equal(startBlock(ctx({ machine: machine({}, CLICK), checklistIncomplete: true })), "checklist");
});

test("the shared ticks decide the checklist when the caller does not say", () => {
  resetLaserStore();
  assert.equal(sharedChecklistIncomplete(), false, "unknown gates never block (the engine checks them)");
  setLaserFlow({ gates: { checklistOn: true, items: 2, requireFraming: false }, ticks: [true] });
  assert.equal(sharedChecklistIncomplete(), true);
  assert.equal(startBlock(ctx()), "checklist");
  setLaserFlow({ ticks: [true, true] });
  assert.equal(sharedChecklistIncomplete(), false);
  assert.equal(startBlock(ctx()), null);
  setLaserFlow({ gates: { checklistOn: false, items: 2, requireFraming: false }, ticks: [] });
  assert.equal(sharedChecklistIncomplete(), false);
  resetLaserStore();
});

test("startState: enabled, the visible reason and never enabled while offline or running", () => {
  resetLaserStore();
  const ok = startState(ctx());
  assert.equal(ok.enabled, true);
  assert.equal(ok.reason, null);
  for (const source of ["toolbar", "dock", "sheet", "hotkey", "startHere"] as StartSource[]) {
    for (const bad of [ctx({ source, online: false }), ctx({ source, machine: machine({ running: true }) }), ctx({ source, machine: machine({ job: { active: true } }) })]) {
      const s = startState(bad);
      assert.equal(s.enabled, false, `${source} must be disabled`);
      assert.ok(s.reason && s.reason.length > 5, "a visible reason");
    }
  }
  assert.match(startState(ctx({ online: false })).reason ?? "", /offline/i);
  assert.match(startState(ctx({ machine: machine({ framing: true }) })).reason ?? "", /fram/i);
  assert.equal(startState(ctx({ designOnly: "This device is design-only." })).reason, "This device is design-only.");
  // While a request is in flight nothing else starts.
  setLaserFlow({ starting: true });
  assert.equal(startState(ctx()).enabled, false);
  resetLaserStore();
});

test("an unticked checklist: disabled in the dock and sheet, opens the checklist from the toolbar and the hotkey", () => {
  resetLaserStore();
  for (const source of ["dock", "sheet"] as StartSource[]) {
    assert.equal(startDecision(ctx({ source, checklistIncomplete: true })), "blocked");
    assert.equal(startState(ctx({ source, checklistIncomplete: true })).enabled, false);
  }
  for (const source of ["toolbar", "hotkey", "startHere"] as StartSource[]) {
    assert.equal(startDecision(ctx({ source, checklistIncomplete: true })), "openChecklist");
    const s = startState(ctx({ source, checklistIncomplete: true }));
    assert.equal(s.enabled, true, "pressing it opens the checklist");
    assert.match(s.reason ?? "", /checklist/i);
  }
});

test("hotkey decision: fire in Laser, open the Laser panel and fire from Design", () => {
  resetLaserStore();
  assert.equal(startDecision(ctx({ source: "hotkey", workspace: "laser" })), "fire");
  assert.equal(startDecision(ctx({ source: "hotkey", workspace: "design" })), "openLaserAndFire");
  assert.equal(startDecision(ctx({ source: "toolbar", workspace: "design" })), "openLaserAndFire");
  assert.equal(startDecision(ctx({ source: "dock", workspace: "design" })), "fire");
  assert.equal(startDecision(ctx({ source: "hotkey", online: false })), "blocked");
});

test("scope and label: Start · selection (3), Start · all · ~2:14, Request run with a PIN", () => {
  resetLaserStore();
  setRunOptions({ cutSelected: false });
  assert.deepEqual(startScope({ selection: ["a", "b", "c"] }), { kind: "all" });
  assert.equal(startLabel(ctx({ estimateS: 134 })), "Start · all · ~2:14");
  assert.equal(startLabel(ctx({ estimateS: null })), "Start · all");
  setRunOptions({ cutSelected: true });
  assert.deepEqual(startScope({ selection: ["a", "b", "c"] }), { kind: "selection", count: 3 });
  // The estimate is the whole job's, so a selection shows no time.
  assert.equal(startLabel(ctx({ selection: ["a", "b", "c"], estimateS: 134 })), "Start · selection (3)");
  // Cut Selected with nothing selected burns everything.
  assert.equal(startLabel(ctx({ selection: [], estimateS: 134 })), "Start · all · ~2:14");
  setRunOptions({ cutSelected: false });
  assert.equal(startLabel(ctx({ fromSegment: 12 })), "Start · from 12");
  assert.equal(startLabel(ctx({ machine: machine({}, PIN), estimateS: 5 })), "Request run · all · ~0:05");
  assert.equal(startLabel(ctx(), true), "Starting…");
  assert.equal(startLabel(ctx({ machine: machine({}, PIN) }), true), "Requesting…");
});

test("shortClock", () => {
  assert.equal(shortClock(0), "0:00");
  assert.equal(shortClock(0.2), "0:01");
  assert.equal(shortClock(42.4), "0:42");
  assert.equal(shortClock(134), "2:14");
  assert.equal(shortClock(3723), "1:02:03");
});

test("runUnderWay", () => {
  assert.equal(runUnderWay(null), false);
  assert.equal(runUnderWay(machine()), false);
  assert.equal(runUnderWay(machine({ running: true })), true);
  assert.equal(runUnderWay(machine({ runs: [{ id: "x", state: "awaiting_confirm" }] })), true);
  assert.equal(runUnderWay(machine({ runs: [{ id: "x", state: "pending" }] })), false);
});

test("startFlow: a blocked press only tells why; the checklist press opens the Laser panel without a request", async () => {
  resetLaserStore();
  const said: string[] = [];
  let opened = 0;
  await startFlow(ctx({ source: "hotkey", online: false, notify: (m) => said.push(m), openLaser: () => opened++ }));
  assert.equal(opened, 0);
  assert.match(said[0] ?? "", /offline/i);
  await startFlow(ctx({ source: "hotkey", checklistIncomplete: true, notify: (m) => said.push(m), openLaser: () => opened++ }));
  assert.equal(opened, 1);
  assert.match(said[1] ?? "", /checklist/i);
  assert.equal(getLaserFlow().starting, false, "nothing was requested");
});

test("startFromHotkey reads the shared facts: offline only tells why, the checklist opens the Laser panel", async () => {
  resetLaserStore();
  const said: string[] = [];
  let opened = 0;
  setLaserSource("app", { link: "closed", machine: null, notify: (m) => said.push(m), openLaser: () => opened++ });
  await startFromHotkey();
  assert.match(said[0] ?? "", /offline/i);
  setLaserSource("app", { link: "open", machine: machine(), workspace: "design", notify: (m) => said.push(m), openLaser: () => opened++ });
  setLaserFlow({ gates: { checklistOn: true, items: 1, requireFraming: false }, ticks: [] });
  await startFromHotkey();
  assert.equal(opened, 1);
  assert.match(said[1] ?? "", /checklist/i);
  const c = startCtxOf(getLaserFacts(), "toolbar");
  assert.equal(c.online, true);
  assert.equal(c.workspace, "design");
  resetLaserStore();
});

test("an unchanged source update keeps the facts snapshot (no render loop) but swaps its functions in", () => {
  resetLaserStore();
  const sel: string[] = [];
  const first = () => undefined;
  setLaserSource("dock", { workspace: "laser", selection: sel, notify: first });
  const snap = getLaserFacts();
  const second = () => undefined;
  // What LaserDock does on every render: the same values in a new object, with new callbacks.
  setLaserSource("dock", { workspace: "laser", selection: sel, notify: second });
  assert.equal(getLaserFacts(), snap, "same snapshot when nothing visible changed");
  assert.equal(getLaserFacts().notify, second, "the new callback is used");
  setLaserSource("dock", { workspace: "design", selection: sel, notify: second });
  assert.notEqual(getLaserFacts(), snap, "a visible change is a new snapshot");
  resetLaserStore();
});

test("Start is disabled with a visible reason while the document is empty", () => {
  // Seen in the app: an empty document left Start enabled ("Start · all"); only the engine's refusal said why.
  const c = ctx({ docEmpty: true });
  assert.equal(startBlock(c), "empty");
  const st = startState(c);
  assert.equal(st.enabled, false);
  assert.match(st.reason ?? "", /empty/i);
  assert.equal(startBlock(ctx({ docEmpty: false })), null);
  assert.equal(startBlock(ctx({ docEmpty: null })), null, "not known: the engine decides");
  assert.equal(startBlock(ctx({ docEmpty: true, machine: machine({ running: true }) })), "running", "a run under way says so first");
});
