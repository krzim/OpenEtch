// Small helpers for answers in the engine's wire format: JSON bodies, `{ok: false, error, code?}` errors with the
// engine's status codes, query strings and JSON request bodies (crates/engine/src/error.rs).

/** A request as the worker shell sees it. */
export interface ShellRequest {
  method: string;
  /** The path without the query, e.g. `/api/projects/save`. */
  path: string;
  /** The raw query without `?`. */
  query: string;
  body: Uint8Array;
  /** The `X-OpenEtch-Since` header, passed on to webcore. */
  since?: string;
}

/** An answer to the page. */
export interface ShellReply {
  status: number;
  headers: [string, string][];
  body: Uint8Array;
}

const enc = new TextEncoder();
const dec = new TextDecoder();

export function text(bytes: Uint8Array): string {
  return dec.decode(bytes);
}

export function bytes(s: string): Uint8Array {
  return enc.encode(s);
}

export function json(value: unknown, status = 200): ShellReply {
  return { status, headers: [["content-type", "application/json"]], body: enc.encode(JSON.stringify(value)) };
}

export function binary(body: Uint8Array, contentType: string, extra: [string, string][] = []): ShellReply {
  return { status: 200, headers: [["content-type", contentType], ...extra], body };
}

/** An error answer as the engine sends it. */
export class HttpError extends Error {
  readonly status: number;
  readonly code: string | undefined;
  constructor(status: number, message: string, code?: string) {
    super(message);
    this.name = "HttpError";
    this.status = status;
    this.code = code;
  }
}

export const badRequest = (m: string) => new HttpError(400, m);
export const notFound = (m: string) => new HttpError(404, m);
export const conflict = (m: string, code?: string) => new HttpError(409, m, code);

export function errorReply(e: HttpError): ShellReply {
  const body: Record<string, unknown> = { ok: false, error: e.message };
  if (e.code) body.code = e.code;
  return json(body, e.status);
}

/** The query as a map; a key outside `allowed` is a 400, as the engine's routes answer. */
export function parseQuery(query: string, allowed: string[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const [k, v] of new URLSearchParams(query)) {
    if (!allowed.includes(k)) throw badRequest(`unknown query parameter '${k}' (allowed: ${allowed.join(", ")})`);
    out.set(k, v);
  }
  return out;
}

/**
 * A JSON object body. `fields` lists the keys allowed (serde's deny_unknown_fields); `required` ones must be there.
 * An empty body is `{}` when nothing is required.
 */
export function parseBody(body: Uint8Array, fields: string[], required: string[] = []): Record<string, unknown> {
  const t = text(body).trim();
  let v: unknown;
  if (t === "") {
    if (required.length) throw badRequest(`bad JSON body: missing field \`${required[0]}\``);
    v = {};
  } else {
    try {
      v = JSON.parse(t);
    } catch (e) {
      throw badRequest(`bad JSON body: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  if (!v || typeof v !== "object" || Array.isArray(v)) throw badRequest("bad JSON body: expected a JSON object");
  const o = v as Record<string, unknown>;
  const unknownKey = Object.keys(o).find((k) => !fields.includes(k));
  if (unknownKey !== undefined) throw badRequest(`bad JSON body: unknown field \`${unknownKey}\`, expected one of ${fields.map((f) => `\`${f}\``).join(", ")}`);
  const missing = required.find((k) => o[k] === undefined);
  if (missing !== undefined) throw badRequest(`bad JSON body: missing field \`${missing}\``);
  return o;
}

export function str(o: Record<string, unknown>, key: string): string {
  const v = o[key];
  if (typeof v !== "string") throw badRequest(`bad JSON body: ${key} must be a string`);
  return v;
}

export function optBool(o: Record<string, unknown>, key: string): boolean | undefined {
  const v = o[key];
  if (v === undefined || v === null) return undefined;
  if (typeof v !== "boolean") throw badRequest(`bad JSON body: ${key} must be true or false`);
  return v;
}

/** The parsed JSON of a webcore answer (or null when it is not JSON). */
export function replyJson(r: { body: Uint8Array }): Record<string, unknown> | null {
  try {
    const v = JSON.parse(text(r.body)) as unknown;
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

export function header(r: { headers: [string, string][] }, name: string): string | null {
  const n = name.toLowerCase();
  return r.headers.find(([k]) => k.toLowerCase() === n)?.[1] ?? null;
}
