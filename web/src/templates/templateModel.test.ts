// Templates and examples in the UI (templateModel.ts): the stored spec, the fill-in form's draft, problems and the
// values it sends, new fields and example lists.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument } from "../api/types.ts";
import {
  TEMPLATE_KEY,
  choiceFromLayer,
  draftFont,
  draftOf,
  draftProblems,
  fieldIdFor,
  fillInvocation,
  fillReport,
  fontDraft,
  hasFields,
  newField,
  parseExamples,
  parseSpec,
  setFieldInvocation,
  specOf,
  today,
} from "./templateModel.ts";

const SPEC = parseSpec({
  lock_others: true,
  fields: [
    { id: "name", label: "Name", kind: "text", targets: ["t1"], required: true, max_chars: 5, current: "Ada" },
    { id: "day", label: "Date", kind: "date", targets: ["t2"], format: "dd.MM.yyyy", value: "2026-11-03", current: "2026-11-03" },
    { id: "font", label: "Font", kind: "font", targets: ["t1"], choices: [{ label: "Plain", font: { family: "Hershey Sans", kind: "stroke" } }], current: "Plain" },
    { id: "anyfont", label: "Any font", kind: "font", targets: ["t1"], current: { family: "Hershey Sans", kind: "stroke" } },
    { id: "ring", label: "Ring", kind: "preset", targets: ["l1"], choices: [{ label: "Gold" }, { label: "Blue" }], current: null },
    { id: "sn", label: "First", kind: "serial", current: 7 },
    { id: "bad", label: "Bad", kind: "nope" },
  ],
});

test("the spec is read from template.fields or the document", () => {
  assert.equal(SPEC.fields.length, 6, "an unknown kind is dropped");
  assert.ok(SPEC.lock_others);
  const doc = { extras: { [TEMPLATE_KEY]: JSON.stringify({ fields: [{ id: "a", label: "A", kind: "serial" }] }) } } as unknown as GDocument;
  assert.ok(hasFields(doc));
  assert.equal(specOf(doc).fields[0].required, true, "required defaults to true");
  assert.ok(!hasFields({ extras: { [TEMPLATE_KEY]: "{broken" } } as unknown as GDocument));
  assert.ok(!hasFields(null));
});

test("the draft starts from the current values and sends only what changed", () => {
  const now = new Date(2027, 0, 9);
  const d = draftOf(SPEC, now);
  assert.deepEqual(d, { name: "Ada", day: "2026-11-03", font: "Plain", anyfont: fontDraft({ family: "Hershey Sans", kind: "stroke" }), ring: "", sn: "7" });
  assert.equal(today(now), "2027-01-09");
  assert.equal(fillInvocation(SPEC, d, d), null, "nothing changed");
  const next = { ...d, name: "Bo\r\n", sn: " 12 ", anyfont: fontDraft({ family: "romans", kind: "shx" }), ring: "Blue" };
  assert.deepEqual(fillInvocation(SPEC, next, d), {
    id: "template.fill",
    params: { values: { name: "Bo\n", anyfont: { family: "romans", kind: "shx" }, ring: "Blue", sn: 12 } },
  });
  assert.deepEqual(draftFont(fontDraft({ family: "A,B", kind: "jsf" })), { family: "A,B", kind: "jsf" });
});

test("problems: required, too long, one line, dates, numbers, choices", () => {
  const d = draftOf(SPEC);
  assert.deepEqual(draftProblems(SPEC, { ...d, ring: "Gold" }), {});
  const p = draftProblems(SPEC, { ...d, name: " ", day: "3.11.2026", sn: "x", ring: "Red" });
  assert.deepEqual(Object.keys(p).sort(), ["day", "name", "ring", "sn"]);
  assert.equal(p.name.key, "tpl.required");
  assert.deepEqual(draftProblems(SPEC, { ...d, ring: "Gold", name: "toolong" }).name, { key: "tpl.tooLong", n: 5 });
  assert.equal(draftProblems(SPEC, { ...d, ring: "Gold", name: "a\nb" }).name.key, "tpl.oneLine");
});

test("new fields, ids and the set_field call", () => {
  assert.equal(fieldIdFor("Name on the coin", []), "name-on-the-coin");
  assert.equal(fieldIdFor("Zoë", ["zoe"]), "zoe-2");
  assert.equal(fieldIdFor("!!!", []), "field");
  const f = newField("serial", "First", ["t1"], []);
  assert.deepEqual(f.targets, [], "a serial field has no targets");
  const date = newField("date", "Date", ["t2"], []);
  assert.deepEqual(setFieldInvocation(date, 0), {
    id: "template.set_field",
    params: { field: { id: "date", label: "Date", kind: "date", targets: ["t2"], format: "yyyy-MM-dd" }, index: 0 },
  });
  const text = { ...newField("text", "Name", ["t1"], []), max_chars: 12, hint: "  Fits the coin " };
  assert.deepEqual(setFieldInvocation(text).params.field, {
    id: "name",
    label: "Name",
    kind: "text",
    targets: ["t1"],
    required: true,
    max_chars: 12,
    multiline: false,
    hint: "Fits the coin",
  });
  const layer = { settings: { power_pct: 30 } as never, sub_layers: [], color: "#c08a00" };
  assert.deepEqual(choiceFromLayer(" Gold ", layer), { label: "Gold", settings: { power_pct: 30 }, swatch: "#c08a00" });
});

test("fill reports and example lists are checked", () => {
  assert.deepEqual(fillReport({ overflow: [{ id: "t1", message: "runs outside Rim" }, { x: 1 }], warnings: ["no glyph", 3] }), {
    overflow: [{ id: "t1", message: "runs outside Rim" }],
    warnings: ["no glyph"],
  });
  assert.deepEqual(parseExamples([{ id: "coin", title: "Coin", summary: "A coin", tags: ["a", 1], steps: ["x"] }, { id: 3 }]), [
    { id: "coin", title: "Coin", summary: "A coin", tags: ["a"], steps: ["x"] },
  ]);
});
