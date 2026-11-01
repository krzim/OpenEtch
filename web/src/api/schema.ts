// A starting JSON object for the command palette, built from a command's JSON Schema (as schemars writes it): required
// properties get a placeholder of their type (or their default / first enum value), optional ones are left out.
// Pure, so `npm test` covers it.

type Schema = {
  type?: string | string[];
  properties?: Record<string, Schema>;
  required?: string[];
  default?: unknown;
  enum?: unknown[];
  const?: unknown;
  items?: Schema | Schema[];
  minItems?: number;
  $ref?: string;
  allOf?: Schema[];
  anyOf?: Schema[];
  oneOf?: Schema[];
  definitions?: Record<string, Schema>;
  $defs?: Record<string, Schema>;
};

function resolve(s: Schema | undefined, root: Schema, depth: number): Schema | undefined {
  if (!s || depth > 8) return s;
  if (s.$ref) {
    const m = /^#\/(definitions|\$defs)\/(.+)$/.exec(s.$ref);
    const defs = m ? (m[1] === "definitions" ? root.definitions : root.$defs) : undefined;
    return resolve(m && defs ? defs[m[2]] : undefined, root, depth + 1);
  }
  if (s.allOf && s.allOf.length === 1) return resolve(s.allOf[0], root, depth + 1);
  return s;
}

function sample(s0: Schema | undefined, root: Schema, depth: number): unknown {
  const s = resolve(s0, root, depth);
  if (!s || depth > 6) return null;
  if (s.default !== undefined) return s.default;
  if (s.const !== undefined) return s.const;
  if (s.enum && s.enum.length) return s.enum[0];
  const alt = s.oneOf ?? s.anyOf;
  if (alt && alt.length) {
    const nonNull = alt.find((a) => resolve(a, root, depth + 1)?.type !== "null") ?? alt[0];
    return sample(nonNull, root, depth + 1);
  }
  const t = Array.isArray(s.type) ? (s.type.find((x) => x !== "null") ?? s.type[0]) : s.type;
  switch (t) {
    case "string":
      return "";
    case "number":
    case "integer":
      return 0;
    case "boolean":
      return false;
    case "array": {
      if (Array.isArray(s.items)) return s.items.map((i) => sample(i, root, depth + 1));
      const n = s.minItems ?? 0;
      return Array.from({ length: n }, () => sample(s.items as Schema | undefined, root, depth + 1));
    }
    case "object":
    default: {
      if (!s.properties) return t === "object" ? {} : null;
      const out: Record<string, unknown> = {};
      for (const k of s.required ?? []) out[k] = sample(s.properties[k], root, depth + 1);
      return out;
    }
  }
}

/** Parameters to start from for a command whose input schema is `schema`. */
export function paramsTemplate(schema: unknown): Record<string, unknown> {
  if (!schema || typeof schema !== "object") return {};
  const v = sample(schema as Schema, schema as Schema, 0);
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
}
