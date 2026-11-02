// Unit tests for the friendly timeline chip labels (timelineLabel.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { translate, type MsgKey, type Vars } from "../workspace/i18n.ts";
import { timelineLabel } from "./timelineLabel.ts";

const t = (key: MsgKey, vars?: Vars) => translate("en", key, vars);
const step = (commands: string, summary = "", label = "") => ({ commands, summary, label });

test("timelineLabel: shapes", () => {
  assert.deepEqual(timelineLabel(step("shape.rect", "added rectangle o1 20x10 mm", "rect"), t), { text: "Rectangle", icon: "rect" });
  assert.equal(timelineLabel(step("shape.ellipse", "added ellipse o2 rx 5 ry 5 mm"), t).text, "Circle");
  assert.equal(timelineLabel(step("shape.ellipse", "added ellipse o2 rx 5 ry 3 mm"), t).text, "Ellipse");
  assert.equal(timelineLabel(step("shape.path", "added path o3"), t).text, "Path");
});

test("timelineLabel: transforms are named by their first part", () => {
  assert.deepEqual(timelineLabel(step("object.transform", "move [1, 2] on o1", "transform"), t), { text: "Move", icon: "move" });
  assert.equal(timelineLabel(step("object.transform", "rotate 15° on o1"), t).text, "Rotate");
  assert.equal(timelineLabel(step("object.transform", "scale 2x2 on o1, o2"), t).text, "Scale");
  assert.equal(timelineLabel(step("object.transform", "mirror x on o1"), t).text, "Flip");
  assert.equal(timelineLabel(step("object.transform", "move [3, 0] on copies o5 of o1"), t).text, "Move");
});

test("timelineLabel: text shows what it says", () => {
  assert.equal(timelineLabel(step("text.add", 'added text t3 "Hello"'), t).text, 'Text "Hello"');
  // What the Text tool actually sends (seen in the app: the chip read "text").
  assert.equal(timelineLabel(step("shape.text", 'added text obj-3 "Hello"'), t).text, 'Text "Hello"');
  assert.equal(timelineLabel(step("text.add", "added text t3"), t).text, "Text");
  assert.equal(timelineLabel(step("text.add", `added text t4 "${"x".repeat(40)}"`), t).text, `Text "${"x".repeat(23)}…"`, "long texts are shortened");
  assert.equal(timelineLabel(step("text.update", "edited text t3"), t).text, "Edit text");
  assert.equal(timelineLabel(step("object.rename", 'renamed o1 to "Logo"'), t).text, 'Rename to "Logo"');
});

test("timelineLabel: several commands count the rest; unknown commands keep their short label", () => {
  assert.equal(timelineLabel(step("object.transform, layer.update", "move [1, 0] on o1; updated C01"), t).text, "Move +1");
  assert.equal(timelineLabel(step("layer.update", "power 40"), t).text, "Layer settings");
  assert.equal(timelineLabel(step("group.create", "grouped o1, o2"), t).text, "Group");
  const unknown = timelineLabel(step("warp.mesh", "warped o1", "mesh"), t);
  assert.equal(unknown.text, "mesh", "an unknown command keeps the engine's short label");
  assert.deepEqual(unknown.icon, "pencil");
  assert.equal(timelineLabel(step("", "", ""), t).text, "?");
});

test("timelineLabel: translated with the language's table", () => {
  const es = (key: MsgKey, vars?: Vars) => translate("es", key, vars);
  assert.equal(timelineLabel(step("shape.rect"), es).text, "Rectángulo");
  assert.equal(timelineLabel(step("object.transform", "move [1, 1] on o1"), es).text, "Mover");
});
