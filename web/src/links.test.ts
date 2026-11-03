// The links between the feature packages on the web side: a jig pocket on the cockpit's slots and in both ledgers,
// and the pre-flight findings that come from the contact sheet and the serial slots.

import assert from "node:assert/strict";
import { test } from "node:test";
import { preflightKey, type PreflightWarning } from "./jigs/jigModel.ts";
import { parseEntry } from "./ledger/ledgerModel.ts";
import { ledgerRows, slotBadge, slotLabel, slotLine, type LedgerEntry, type Slot } from "./production/model.ts";
import { EN } from "./workspace/locales/en.ts";
import { ES } from "./workspace/locales/es.ts";

const pocket = (number: number) => ({ jig: "jig-1", jig_name: "Tags", pocket: `p${number}`, number });

test("a slot in a jig pocket shows the pocket's number, a loose one its slot number", () => {
  const slot: Slot = { number: 2, offset: 1, label: "Tags P3", objects: ["a"], empty: false, past_end: false, value: 11, values: [], pocket: pocket(3) };
  assert.equal(slotBadge(slot), "P3");
  assert.equal(slotLine(slot).badge, "P3");
  assert.equal(slotLabel(slot), "Tags P3", "the label already names the pocket");
  assert.equal(slotLabel({ label: "Coin A", pocket: pocket(3) }), "Coin A · Tags P3");
  assert.equal(slotBadge({ number: 4 }), "4");
  assert.equal(slotLabel({ label: "Slot 4" }), "Slot 4");
});

test("the serial ledger's rows carry the pocket", () => {
  const e: LedgerEntry = {
    time_unix_s: 5,
    run_id: "run-1",
    state: "finished",
    device: "b6",
    project: null,
    document: "Tags",
    completed: 1,
    counter: 10,
    slots: [{ number: 1, offset: 0, label: "Name", value: 10, values: [], marked: true, pocket: pocket(1) }],
  };
  assert.equal(ledgerRows([e])[0].label, "Name · Tags P1");
});

test("the job ledger reads each value's pocket and the run's jigs", () => {
  const e = parseEntry({
    time_unix_s: 9,
    run_id: "run-2",
    variables: { current: 10, texts: [{ object: "o1", name: "SN", mode: "serial", value: "SN 010", pocket: pocket(1) }, { object: "o2", name: "x", mode: "serial", value: "1" }] },
    jigs: [{ id: "jig-1", name: "Tags", pockets_on: [1, 3], pockets: 3 }],
  });
  assert.ok(e);
  assert.deepEqual(e.variables?.texts[0].pocket, { jig: "jig-1", jigName: "Tags", pocket: "p1", number: 1 });
  assert.equal(e.variables?.texts[1].pocket, null);
  assert.deepEqual(e.jigs, [{ id: "jig-1", name: "Tags", pocketsOn: [1, 3], pockets: 3 }]);
  assert.deepEqual(parseEntry({ run_id: "old" })?.jigs, [], "older lines have no jigs");
  for (const k of ["led.jigs", "led.jigPockets"]) assert.ok(k in EN && k in ES, k);
});

test("pre-flight findings from the contact sheet and Stop at End have their own text", () => {
  const w = (check: string, params: Record<string, unknown>): PreflightWarning => ({ check, blocking: false, text: "", params, layers: [], objects: [] });
  const row = preflightKey(w("contact_sheet", { kind: "overflow", name: "SN", value: 5, message: "runs outside \"Tag\"" }));
  assert.equal(row.key, "pf.rowCheck");
  assert.deepEqual(row.vars, { name: "SN", value: 5, message: "runs outside \"Tag\"" });
  assert.equal(preflightKey(w("contact_sheet", { kind: "partial", checked: 200 })).key, "pf.rowPartial");
  assert.equal(preflightKey(w("unfilled_variable", { kind: "stop_at_end", names: "\"Tags P3\"", end: 100 })).key, "pf.stopAtEnd");
  for (const k of ["pf.rowCheck", "pf.rowPartial", "pf.stopAtEnd", "pf.check.contact_sheet"]) assert.ok(k in EN && k in ES, k);
});
