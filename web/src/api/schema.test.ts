// Unit tests for api/schema.ts: command palette templates from JSON Schema.

import assert from "node:assert/strict";
import { test } from "node:test";
import { paramsTemplate } from "./schema.ts";

test("required properties get placeholders, optional ones are left out", () => {
  const schema = {
    type: "object",
    required: ["ids", "translate", "name", "mode", "flag"],
    properties: {
      ids: { type: "array", items: { type: "string" } },
      translate: { type: "array", items: { type: "number", format: "double" }, minItems: 2, maxItems: 2 },
      name: { type: "string" },
      mode: { $ref: "#/definitions/Mode" },
      flag: { type: "boolean", default: true },
      optional: { type: ["number", "null"] },
    },
    definitions: { Mode: { type: "string", enum: ["replace", "merge"] } },
  };
  assert.deepEqual(paramsTemplate(schema), { ids: [], translate: [0, 0], name: "", mode: "replace", flag: true });
});

test("odd input gives an empty object", () => {
  assert.deepEqual(paramsTemplate(null), {});
  assert.deepEqual(paramsTemplate({ type: "object" }), {});
  assert.deepEqual(paramsTemplate({ $ref: "#/definitions/Missing" }), {});
  const nested = { type: "object", required: ["a"], properties: { a: { anyOf: [{ type: "null" }, { type: "object", required: ["b"], properties: { b: { type: "integer" } } }] } } };
  assert.deepEqual(paramsTemplate(nested), { a: { b: 0 } });
});
