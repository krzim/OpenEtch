// The step language of the guide's screenshots (docs/guide/shots.json, run by guide-shots.mjs; web/e2e/README.md
// "Guide screenshots" documents it). Pure: validation and key parsing, shared by the script and the docs test
// (src/guide/docs.test.ts), so a bad shot fails `npm test` before anyone runs a browser.

/** One action key per step; the other keys of a step are its options. */
export const STEP_KINDS = ["exec", "click", "dblclick", "context", "menu", "type", "key", "hash", "wait", "select", "workspace", "tab", "escape", "scroll", "hover", "set"];

/** Options each step kind accepts besides its own key. */
const OPTIONS = { exec: ["params"], click: ["in"], dblclick: ["in"], context: ["in"], hover: ["in"], type: ["into"], set: ["into"], tab: ["sheet"] };

/** Engine commands a shot may not run: they read or write files (a shot shows only the built-in examples and
 *  shapes drawn fresh) or print. */
export const FORBIDDEN_EXEC = /^(doc\.(load|save|export|export_lbrn2|import_lbrn2|print)|art\.import_\w+|image\.(trace_files|export)|jig\.import|var\.load_csv)$/;

/** A string that looks like a file system path or a machine address (refused in a shot's parameters and text). */
export function looksPrivate(s) {
  if (typeof s !== "string") return false;
  return (
    /(^|[\s"'(])[A-Za-z]:[\\/]/.test(s) ||
    /^(\/|~\/|\.\.?[\\/])/.test(s) ||
    /\/(home|Users)\/[^/\s]+/.test(s) ||
    /\\\\/.test(s) ||
    /\.(lbrn2?|clb|svg|dxf|ai|pdf|png|jpe?g|csv|prefs)$/i.test(s) ||
    /\b(10|127|192\.168|172\.(1[6-9]|2\d|3[01]))\.\d{1,3}\.\d{1,3}(\.\d{1,3})?\b/.test(s)
  );
}

function strings(v, out = []) {
  if (typeof v === "string") out.push(v);
  else if (Array.isArray(v)) v.forEach((x) => strings(x, out));
  else if (v && typeof v === "object") Object.values(v).forEach((x) => strings(x, out));
  return out;
}

const SHOT_KEYS = ["name", "device", "theme", "example", "steps", "crop", "cropWidth", "cropPad", "maxWidth", "note"];
const SNAPS = ["peek", "half", "full"];

/** A click target: an accessible name, { css, text? } or { object: name or id } (a shape on the canvas). */
function targetProblem(t) {
  if (typeof t === "string") return t.trim() ? null : "an empty name";
  if (t && typeof t === "object" && !Array.isArray(t)) {
    const keys = Object.keys(t);
    if (typeof t.css === "string" && keys.every((k) => k === "css" || k === "text")) return null;
    if (typeof t.object === "string" && keys.length === 1) return null;
    if (Array.isArray(t.point) && t.point.length === 2 && t.point.every(Number.isFinite) && keys.length === 1) return null;
  }
  return 'a target must be "Accessible name", { "css": "selector", "text"?: "..." }, { "object": "shape name" } or { "point": [x_mm, y_mm] }';
}

/** What is wrong with one step, or null. */
export function stepProblem(step) {
  if (!step || typeof step !== "object" || Array.isArray(step)) return "a step must be an object";
  const kinds = Object.keys(step).filter((k) => STEP_KINDS.includes(k));
  if (kinds.length !== 1) return `a step needs exactly one of ${STEP_KINDS.join(", ")} (got ${kinds.join(", ") || "none"})`;
  const kind = kinds[0];
  const extra = Object.keys(step).filter((k) => k !== kind && !(OPTIONS[kind] ?? []).includes(k));
  if (extra.length) return `${kind}: unknown option ${extra.join(", ")}`;
  const v = step[kind];
  const priv = strings(step).find(looksPrivate);
  if (priv) return `${kind}: "${priv}" looks like a file path or a LAN address (shots use the built-in examples only)`;
  switch (kind) {
    case "exec":
      if (typeof v !== "string" || !/^[a-z_]+(\.[a-z_]+)+$/.test(v)) return "exec: a command id such as \"layer.update\"";
      if (FORBIDDEN_EXEC.test(v)) return `exec: ${v} reads or writes files; shots use the built-in examples only`;
      if (step.params !== undefined && (typeof step.params !== "object" || Array.isArray(step.params))) return "exec: params must be an object";
      return null;
    case "click":
    case "dblclick":
    case "hover":
    case "context": {
      if (kind === "context" && v === "selection") return null;
      const p = targetProblem(v);
      if (p) return `${kind}: ${p}`;
      if (step.in !== undefined && typeof step.in !== "string") return `${kind}: "in" must be a CSS selector`;
      return null;
    }
    case "menu":
      return Array.isArray(v) && v.length >= 1 && v.every((x) => typeof x === "string" && x.trim()) ? null : 'menu: a path such as ["File", "Laser", "Devices"]';
    case "type":
      if (typeof v !== "string") return "type: the text to type";
      return step.into === undefined || targetProblem(step.into) === null ? null : "type: into must be a target";
    case "set":
      if (typeof v !== "string") return "set: the value (a select's option value, a colour such as #c08a00, a date)";
      return step.into !== undefined && targetProblem(step.into) === null ? null : "set: into must be a target (the field, or its label)";
    case "key":
      return typeof v === "string" && parseKey(v) ? null : `key: "${v}" is not a key such as "Escape" or "Ctrl+Z"`;
    case "hash":
      return typeof v === "string" && /^#\//.test(v) ? null : 'hash: a route such as "#/guide/the-workspace#ribbon"';
    case "wait":
      if (typeof v === "number") return v >= 0 && v <= 10000 ? null : "wait: at most 10000 ms";
      return typeof v === "string" && v.trim() ? null : "wait: a CSS selector or a number of ms";
    case "select":
      if (v === "all" || v === "none") return null;
      return Array.isArray(v) && v.length && v.every((x) => typeof x === "string" && x.trim()) ? null : 'select: "all", "none" or a list of shape names / ids';
    case "workspace":
      return v === "design" || v === "laser" ? null : 'workspace: "design" or "laser"';
    case "tab":
      if (!["Canvas", "Layers", "Job", "More"].includes(v)) return 'tab: one of the phone tabs "Canvas", "Layers", "Job", "More"';
      return step.sheet === undefined || SNAPS.includes(step.sheet) || step.sheet === "closed" ? null : 'tab: sheet must be "peek", "half", "full" or "closed"';
    case "escape":
      return v === true ? null : "escape: true";
    case "scroll":
      return typeof v === "string" && v.trim() ? null : "scroll: a CSS selector to scroll into view";
  }
  return null;
}

/** Every problem in a shots.json list. `devices`: the ids of devices.mjs; `examples`: the built-in example ids. */
export function shotsProblems(shots, { devices, examples }) {
  const out = [];
  if (!Array.isArray(shots)) return ["shots.json must be a list"];
  const names = new Set();
  shots.forEach((s, i) => {
    const at = `shot ${i + 1}${s?.name ? ` "${s.name}"` : ""}`;
    if (!s || typeof s !== "object") return out.push(`${at}: not an object`);
    for (const k of Object.keys(s)) if (!SHOT_KEYS.includes(k)) out.push(`${at}: unknown key "${k}"`);
    if (typeof s.name !== "string" || !/^[a-z0-9]+(-[a-z0-9]+)*$/.test(s.name)) out.push(`${at}: name must be kebab-case`);
    else if (names.has(s.name)) out.push(`${at}: name used twice`);
    else names.add(s.name);
    if (!devices.includes(s.device)) out.push(`${at}: device must be one of ${devices.join(", ")}`);
    if (s.theme !== undefined && s.theme !== "light" && s.theme !== "dark") out.push(`${at}: theme must be "light" or "dark"`);
    if (!examples.includes(s.example)) out.push(`${at}: example must be one of ${examples.join(", ")}`);
    if (!Array.isArray(s.steps)) out.push(`${at}: steps must be a list (it may be empty)`);
    else
      s.steps.forEach((st, k) => {
        const p = stepProblem(st);
        if (p) out.push(`${at} step ${k + 1}: ${p}`);
      });
    if (s.crop !== undefined && !(typeof s.crop === "string" && s.crop.trim())) out.push(`${at}: crop must be a CSS selector`);
    if (s.maxWidth !== undefined && !(Number.isInteger(s.maxWidth) && s.maxWidth >= 200 && s.maxWidth <= 3000)) out.push(`${at}: maxWidth must be 200-3000 px`);
    if (s.cropPad !== undefined && !(Number.isInteger(s.cropPad) && s.cropPad >= 0 && s.cropPad <= 32 && s.crop)) out.push(`${at}: cropPad must be 0-32 CSS px, with a crop`);
    if (s.cropWidth !== undefined && !(Number.isInteger(s.cropWidth) && s.cropWidth >= 100 && s.cropWidth <= 3000 && s.crop)) out.push(`${at}: cropWidth must be 100-3000 CSS px, with a crop`);
    if (s.note !== undefined && typeof s.note !== "string") out.push(`${at}: note must be text`);
  });
  return out;
}

const NAMED = new Set(["Escape", "Enter", "Delete", "Backspace", "Tab", "ArrowLeft", "ArrowUp", "ArrowRight", "ArrowDown", "Home", "End", "PageUp", "PageDown", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "Space"]);
const MODS = { Alt: 1, Ctrl: 2, Control: 2, Meta: 4, Cmd: 4, Shift: 8 };

/** "Ctrl+Shift+Z" -> { key: "Z", modifiers: 10 } (CDP bit mask: 1 Alt, 2 Ctrl, 4 Meta, 8 Shift), or null. */
export function parseKey(s) {
  const parts = String(s).split("+");
  // "Ctrl++" names the plus key.
  if (s.endsWith("++")) parts.splice(parts.length - 2, 2, "+");
  const key = parts.pop();
  let modifiers = 0;
  for (const m of parts) {
    if (!(m in MODS)) return null;
    modifiers |= MODS[m];
  }
  if (!key) return null;
  if (key === "Space") return { key: " ", modifiers };
  if (NAMED.has(key) || key.length === 1) return { key, modifiers };
  return null;
}
