// Small shared UI pieces: the modal dialog (an alias of Dialog.tsx), commit-on-blur fields, and the icon set. Plain
// CSS, no libraries.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { evalNumber } from "../editing.ts";
import { useT, useUnitConversion } from "../workspace/usePrefs.ts";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { holdDelay, numericInputProps, stepValue } from "./numericInput.ts";
import "./fields.css";
import { trimNumber } from "../workspace/units.ts";

/**
 * Centred modal with a title, body and footer: an alias of the Dialog primitive (Dialog.tsx), which adds the dialog
 * stack, the focus trap, sizes, the phone sheet and "Discard changes?". Escape cancels; with `onSubmit`, Enter in a
 * text field submits.
 */
export { Dialog as Modal } from "./Dialog.tsx";

/** Format a number for a field: up to `digits` decimals, no trailing zeros. */
export function fmt(v: number | null | undefined, digits = 3): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return "";
  const r = Number(v.toFixed(digits));
  return Object.is(r, -0) ? "0" : String(r);
}

/** A coarse pointer (or Density = Touch): number fields with `stepper` show −/+ buttons. */
function useTouchFields(): boolean {
  const l = useLayoutMode();
  return l.pointer === "coarse" || l.density === "touch";
}

/**
 * −/+ buttons beside a number field (coarse pointers). A tap steps once and commits; holding repeats (the field shows
 * the running value) and commits once on release, so a hold is one undo step.
 */
function Steppers(p: { label: string; value: number | null; step: number; min?: number; max?: number; disabled?: boolean; onShow: (v: number) => void; onCommit: (v: number) => void }) {
  const t = useT();
  const held = useRef<number | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const P = useRef(p);
  P.current = p;
  const stop = (commit: boolean) => {
    window.clearTimeout(timer.current);
    const v = held.current;
    held.current = null;
    if (commit && v !== null && v !== P.current.value) P.current.onCommit(v);
  };
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const start = (dir: 1 | -1) => {
    const q = P.current;
    held.current = stepValue(q.value, q.step, dir, q.min, q.max);
    q.onShow(held.current);
    let n = 0;
    const tick = () => {
      const r = P.current;
      if (held.current === null) return;
      held.current = stepValue(held.current, r.step, dir, r.min, r.max);
      r.onShow(held.current);
      timer.current = window.setTimeout(tick, holdDelay(++n));
    };
    timer.current = window.setTimeout(tick, holdDelay(0));
  };
  const btn = (dir: 1 | -1) => (
    <button
      type="button"
      className="field-step"
      tabIndex={-1}
      disabled={p.disabled}
      aria-label={t(dir < 0 ? "rb.decrease" : "rb.increase", { label: p.label })}
      onPointerDown={(e) => {
        e.preventDefault();
        e.currentTarget.setPointerCapture?.(e.pointerId);
        start(dir);
      }}
      onPointerUp={() => stop(true)}
      onPointerCancel={() => stop(false)}
      onLostPointerCapture={() => stop(true)}
      onClick={(e) => {
        // Keyboard or assistive activation (no pointer press): one step.
        if (e.detail !== 0) return;
        const q = P.current;
        q.onCommit(stepValue(q.value, q.step, dir, q.min, q.max));
      }}
    >
      {dir < 0 ? "−" : "+"}
    </button>
  );
  return (
    <>
      {btn(-1)}
      {btn(1)}
    </>
  );
}

/**
 * A number entry whose value stays in engine units (mm): with Preferences set to inches the field shows and takes
 * inches (workspace/usePrefs.ts) and hands the converted mm text back, like NumberField. For dialogs that keep the
 * text and parse it on submit.
 */
export function UnitTextField(props: { label: string; value: string; onChange: (v: string) => void; unit?: string; title?: string; signed?: boolean }) {
  const conv = useUnitConversion(props.unit);
  // What the user typed, kept while the parent's value is still the one that text produced.
  const [typed, setTyped] = useState<{ text: string; value: string } | null>(null);
  const converts = conv.unit !== props.unit;
  let shown = props.value;
  if (converts) {
    const n = evalNumber(props.value);
    shown = typed && typed.value === props.value ? typed.text : n === null ? props.value : trimNumber(conv.to(n), 4 + conv.extraDigits);
  }
  const change = (text: string) => {
    if (!converts) return props.onChange(text);
    const n = evalNumber(text);
    const value = n === null ? text : String(conv.from(n));
    setTyped({ text, value });
    props.onChange(value);
  };
  return (
    <label className="field" title={props.title}>
      <span className="field-label">{props.label}</span>
      <span className="field-input">
        <input type="text" {...numericInputProps(!!props.signed)} aria-label={props.label} value={shown} onChange={(e) => change(e.target.value)} />
        {conv.unit && <span className="unit">{conv.unit}</span>}
      </span>
    </label>
  );
}

/**
 * Numeric input that keeps local text while editing and commits on Enter or blur, only when the parsed value changed.
 * Simple maths works (`20/3`, `12.5+2*3`). `value` null renders empty; with `allowEmpty`, clearing the field commits
 * null. Out-of-range values are clamped and the field briefly says so.
 */
export function NumberField(props: {
  label: string;
  value: number | null;
  onCommit: (v: number | null) => void;
  unit?: string;
  min?: number;
  max?: number;
  step?: number;
  digits?: number;
  allowEmpty?: boolean;
  placeholder?: string;
  disabled?: boolean;
  /** Takes a sign (X/Y, rotation, offsets): the text keyboard on phones and tablets, which has "-" and "/". */
  signed?: boolean;
  /** −/+ buttons with press-and-hold on coarse pointers (power, speed, passes, frequency, size). */
  stepper?: boolean;
}) {
  // Lengths and speeds show in the display unit chosen in Preferences; the value committed is always the engine's.
  const conv = useUnitConversion(props.unit);
  const digits = (props.digits ?? 3) + conv.extraDigits;
  const value = props.value === null ? null : conv.to(props.value);
  const min = props.min === undefined ? undefined : conv.to(props.min);
  const max = props.max === undefined ? undefined : conv.to(props.max);
  const unit = conv.unit;
  const onCommit = (v: number | null) => props.onCommit(v === null ? null : conv.from(v));
  const [text, setText] = useState(fmt(value, digits));
  const [clamped, setClamped] = useState<string | null>(null);
  const editing = useRef(false);
  const touch = useTouchFields();
  useEffect(() => {
    if (!editing.current) setText(fmt(value, digits));
  }, [value, digits]);
  useEffect(() => {
    if (!clamped) return;
    const t = window.setTimeout(() => setClamped(null), 3000);
    return () => window.clearTimeout(t);
  }, [clamped]);
  // Only text the user typed is committed: a linked field (Lock aspect, Keep round) changed by another field while
  // this one had focus must not send its stale value back on blur.
  const dirty = useRef(false);
  const commit = () => {
    editing.current = false;
    if (!dirty.current) {
      setText(fmt(value, digits));
      return;
    }
    dirty.current = false;
    const t = text.trim();
    if (t === "") {
      if (props.allowEmpty && value !== null) onCommit(null);
      else setText(fmt(value, digits));
      return;
    }
    let v = evalNumber(t);
    if (v === null) {
      setText(fmt(value, digits));
      return;
    }
    const typed = v;
    if (min !== undefined) v = Math.max(min, v);
    if (max !== undefined) v = Math.min(max, v);
    setClamped(v !== typed ? `Limited to ${fmt(v, digits)}${unit ? ` ${unit}` : ""}` : null);
    setText(fmt(v, digits));
    if (value === null || Math.abs(v - value) > 1e-9) onCommit(v);
  };
  return (
    <label className="field">
      <span className="field-label">{props.label}</span>
      <span className="field-input">
        <input
          type="text"
          {...numericInputProps(!!props.signed)}
          aria-label={props.label}
          value={text}
          placeholder={props.placeholder}
          disabled={props.disabled}
          onFocus={() => {
            editing.current = true;
            dirty.current = false;
          }}
          onChange={(e) => {
            dirty.current = true;
            setText(e.target.value);
          }}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            if (e.key === "Escape") {
              editing.current = false;
              dirty.current = false;
              setText(fmt(value, digits));
              (e.target as HTMLInputElement).blur();
            }
          }}
        />
        {unit && <span className="unit">{unit}</span>}
        {props.stepper && touch && (
          <Steppers
            label={props.label}
            value={value}
            step={props.step ?? 1}
            min={min}
            max={max}
            disabled={props.disabled}
            onShow={(v) => setText(fmt(v, digits))}
            onCommit={(v) => {
              setText(fmt(v, digits));
              if (value === null || Math.abs(v - value) > 1e-9) onCommit(v);
            }}
          />
        )}
      </span>
      {clamped && (
        <span className="field-note" role="status">
          {clamped}
        </span>
      )}
    </label>
  );
}

/**
 * Two numbers edited together (X and Y): one commit, so one undo step, when Enter is pressed or focus leaves the pair.
 * Values commit rounded to micrometres. Maths works as in NumberField.
 */
export function PairField(props: {
  labels: [string, string];
  unit?: string;
  value: [number, number];
  onCommit: (v: [number, number]) => void;
  digits?: number;
  /** The values take a sign (positions): the text keyboard on touch, with "-" and "/". */
  signed?: boolean;
}) {
  const conv = useUnitConversion(props.unit);
  const digits = (props.digits ?? 3) + conv.extraDigits;
  const value: [number, number] = [conv.to(props.value[0]), conv.to(props.value[1])];
  const unit = conv.unit;
  const [text, setText] = useState<[string, string]>([fmt(value[0], digits), fmt(value[1], digits)]);
  const editing = useRef(false);
  useEffect(() => {
    if (!editing.current) setText([fmt(value[0], digits), fmt(value[1], digits)]);
  }, [value[0], value[1], digits]);
  const commit = () => {
    editing.current = false;
    const a = evalNumber(text[0]);
    const b = evalNumber(text[1]);
    if (a === null || b === null) {
      setText([fmt(value[0], digits), fmt(value[1], digits)]);
      return;
    }
    const q = 10 ** digits;
    const r = (v: number) => Math.round(v * q) / q;
    const next: [number, number] = [r(a), r(b)];
    setText([fmt(next[0], digits), fmt(next[1], digits)]);
    if (Math.abs(next[0] - value[0]) > 0.5 / q || Math.abs(next[1] - value[1]) > 0.5 / q) {
      // Back to engine units, rounded to micrometres.
      const mm = (v: number) => Math.round(conv.from(v) * 1000) / 1000;
      props.onCommit([mm(next[0]), mm(next[1])]);
    }
  };
  const box = (i: 0 | 1) => (
    <label className="field">
      <span className="field-label">{props.labels[i]}</span>
      <span className="field-input">
        <input
          type="text"
          {...numericInputProps(!!props.signed)}
          aria-label={props.labels[i]}
          value={text[i]}
          onFocus={() => (editing.current = true)}
          onChange={(e) => {
            const t = e.target.value;
            setText((p) => (i === 0 ? [t, p[1]] : [p[0], t]));
          }}
          onBlur={(e) => {
            // Moving between X and Y is still one edit.
            const to = e.relatedTarget as Node | null;
            if (to && e.currentTarget.closest(".pair-field")?.contains(to)) return;
            commit();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") {
              editing.current = false;
              setText([fmt(value[0], digits), fmt(value[1], digits)]);
              (e.target as HTMLInputElement).blur();
            }
          }}
        />
        {unit && <span className="unit">{unit}</span>}
      </span>
    </label>
  );
  return (
    <div className="field-grid pair-field">
      {box(0)}
      {box(1)}
    </div>
  );
}

/** Text input that commits on Enter or blur. */
export function TextField(props: { label: string; value: string; onCommit: (v: string) => void }) {
  const [text, setText] = useState(props.value);
  const editing = useRef(false);
  useEffect(() => {
    if (!editing.current) setText(props.value);
  }, [props.value]);
  const commit = () => {
    editing.current = false;
    const t = text.trim();
    if (t && t !== props.value) props.onCommit(t);
    else setText(props.value);
  };
  return (
    <label className="field">
      <span className="field-label">{props.label}</span>
      <span className="field-input">
        <input
          type="text"
          value={text}
          onFocus={() => (editing.current = true)}
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
        />
      </span>
    </label>
  );
}

/**
 * Colour picker. Browsers fire `input` continuously while the picker is dragged, so the commit is debounced to send one
 * command per pick rather than dozens.
 */
export function ColorField(props: { label: string; value: string; onCommit: (v: string) => void }) {
  const [local, setLocal] = useState(props.value);
  const timer = useRef<number | undefined>(undefined);
  const commitRef = useRef(props.onCommit);
  commitRef.current = props.onCommit;
  useEffect(() => setLocal(props.value), [props.value]);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return (
    <label className="field">
      <span className="field-label">{props.label}</span>
      <span className="field-input">
        <input
          type="color"
          value={local}
          onChange={(e) => {
            const v = e.target.value;
            setLocal(v);
            window.clearTimeout(timer.current);
            timer.current = window.setTimeout(() => v !== props.value && commitRef.current(v), 400);
          }}
        />
      </span>
    </label>
  );
}

/** Labelled checkbox row. */
export function CheckField(props: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="field field-check">
      <span className="field-label">{props.label}</span>
      <input type="checkbox" checked={props.checked} onChange={(e) => props.onChange(e.target.checked)} />
    </label>
  );
}

// The icon registry (docs/ux/ui-plan-2026-11.md 1.4): path data copied from Lucide (lucide-static 0.460.0, ISC) and,
// for CAD gaps, Tabler Icons (3.19.0, MIT); licence texts in src/icons/THIRD_PARTY_NOTICES.txt. 24x24, stroke 1.5,
// coloured by currentColor. Sizes: 20 px toolbar, 16 px menus and inspector, 24 px touch HUD.
// L = Lucide, T = Tabler, with the source icon's name in brackets.
const ICONS: Record<string, ReactNode> = {
  // L [box]
  stock: (
    <>
      <path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z" />
      <path d="m3.3 7 8.7 5 8.7-5" />
      <path d="M12 22V12" />
    </>
  ),
  // L [square]
  rect: <rect width="18" height="18" x="3" y="3" rx="2" />,
  // L [spline]
  path: (
    <>
      <circle cx="19" cy="5" r="2" />
      <circle cx="5" cy="19" r="2" />
      <path d="M5 17A12 12 0 0 1 17 5" />
    </>
  ),
  // L [image]
  image: (
    <>
      <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
      <circle cx="9" cy="9" r="2" />
      <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />
    </>
  ),
  // T [oval], turned on its side
  ellipse: <path d="M12 12m-6 0a6 9 0 1 0 12 0a6 9 0 1 0 -12 0" transform="rotate(90 12 12)" />,
  // L [circle-dot]
  circle: (
    <>
      <circle cx="12" cy="12" r="10" />
      <circle cx="12" cy="12" r="1" />
    </>
  ),
  // L [type]
  text: (
    <>
      <polyline points="4 7 4 4 20 4 20 7" />
      <line x1="9" x2="15" y1="20" y2="20" />
      <line x1="12" x2="12" y1="4" y2="20" />
    </>
  ),
  // L [qr-code]
  barcode: (
    <>
      <rect width="5" height="5" x="3" y="3" rx="1" />
      <rect width="5" height="5" x="16" y="3" rx="1" />
      <rect width="5" height="5" x="3" y="16" rx="1" />
      <path d="M21 16h-3a2 2 0 0 0-2 2v3" />
      <path d="M21 21v.01" />
      <path d="M12 7v3a2 2 0 0 1-2 2H7" />
      <path d="M3 12h.01" />
      <path d="M12 3h.01" />
      <path d="M12 16v.01" />
      <path d="M16 12h1" />
      <path d="M21 12v.01" />
      <path d="M12 21v-1" />
    </>
  ),
  // T [line]
  line: (
    <>
      <path d="M6 18m-2 0a2 2 0 1 0 4 0a2 2 0 1 0 -4 0" />
      <path d="M18 6m-2 0a2 2 0 1 0 4 0a2 2 0 1 0 -4 0" />
      <path d="M7.5 16.5l9 -9" />
    </>
  ),
  // L [pen-tool]
  pen: (
    <>
      <path d="M15.707 21.293a1 1 0 0 1-1.414 0l-1.586-1.586a1 1 0 0 1 0-1.414l5.586-5.586a1 1 0 0 1 1.414 0l1.586 1.586a1 1 0 0 1 0 1.414z" />
      <path d="m18 13-1.375-6.874a1 1 0 0 0-.746-.776L3.235 2.028a1 1 0 0 0-1.207 1.207L5.35 15.879a1 1 0 0 0 .776.746L13 18" />
      <path d="m2.3 2.3 7.286 7.286" />
      <circle cx="11" cy="11" r="2" />
    </>
  ),
  // L [pentagon]
  polygon: <path d="M10.83 2.38a2 2 0 0 1 2.34 0l8 5.74a2 2 0 0 1 .73 2.25l-3.04 9.26a2 2 0 0 1-1.9 1.37H7.04a2 2 0 0 1-1.9-1.37L2.1 10.37a2 2 0 0 1 .73-2.25z" />,
  // T [vector-bezier-2]
  node: (
    <>
      <path d="M3 3m0 1a1 1 0 0 1 1 -1h2a1 1 0 0 1 1 1v2a1 1 0 0 1 -1 1h-2a1 1 0 0 1 -1 -1z" />
      <path d="M17 17m0 1a1 1 0 0 1 1 -1h2a1 1 0 0 1 1 1v2a1 1 0 0 1 -1 1h-2a1 1 0 0 1 -1 -1z" />
      <path d="M7 5l7 0" />
      <path d="M10 19l7 0" />
      <path d="M9 19m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
      <path d="M15 5m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
      <path d="M7 5.5a5 6.5 0 0 1 5 6.5a5 6.5 0 0 0 5 6.5" />
    </>
  ),
  // L [scissors]
  trim: (
    <>
      <circle cx="6" cy="6" r="3" />
      <path d="M8.12 8.12 12 12" />
      <path d="M20 4 8.12 15.88" />
      <circle cx="6" cy="18" r="3" />
      <path d="M14.8 14.8 20 20" />
    </>
  ),
  // L [paint-bucket]
  fillRegion: (
    <>
      <path d="m19 11-8-8-8.6 8.6a2 2 0 0 0 0 2.8l5.2 5.2c.8.8 2 .8 2.8 0L19 11Z" />
      <path d="m5 2 5 5" />
      <path d="M2 13h15" />
      <path d="M22 20a2 2 0 1 1-4 0c0-1.6 1.7-2.4 2-4 .3 1.6 2 2.4 2 4Z" />
    </>
  ),
  // L [ruler]
  measure: (
    <>
      <path d="M21.3 15.3a2.4 2.4 0 0 1 0 3.4l-2.6 2.6a2.4 2.4 0 0 1-3.4 0L2.7 8.7a2.41 2.41 0 0 1 0-3.4l2.6-2.6a2.41 2.41 0 0 1 3.4 0Z" />
      <path d="m14.5 12.5 2-2" />
      <path d="m11.5 9.5 2-2" />
      <path d="m8.5 6.5 2-2" />
      <path d="m17.5 15.5 2-2" />
    </>
  ),
  // L [settings-2]
  modify: (
    <>
      <path d="M20 7h-9" />
      <path d="M14 17H5" />
      <circle cx="17" cy="17" r="3" />
      <circle cx="7" cy="7" r="3" />
    </>
  ),
  // L [layers]
  arrange: (
    <>
      <path d="m12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z" />
      <path d="m22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65" />
      <path d="m22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65" />
    </>
  ),
  // L [clipboard]
  edit: (
    <>
      <rect width="8" height="4" x="8" y="2" rx="1" ry="1" />
      <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" />
    </>
  ),
  // L [wrench]
  tools: <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />,
  // L [import]
  import: (
    <>
      <path d="M12 3v12" />
      <path d="m8 11 4 4 4-4" />
      <path d="M8 5H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-4" />
    </>
  ),
  // L [file-input]
  insert: (
    <>
      <path d="M4 22h14a2 2 0 0 0 2-2V7l-5-5H6a2 2 0 0 0-2 2v4" />
      <path d="M14 2v4a2 2 0 0 0 2 2h4" />
      <path d="M2 15h10" />
      <path d="m9 18 3-3-3-3" />
    </>
  ),
  // L [layout-template]
  place: (
    <>
      <rect width="18" height="7" x="3" y="3" rx="1" />
      <rect width="9" height="7" x="3" y="14" rx="1" />
      <rect width="5" height="7" x="16" y="14" rx="1" />
    </>
  ),
  // L [layout-grid]
  array: (
    <>
      <rect width="7" height="7" x="3" y="3" rx="1" />
      <rect width="7" height="7" x="14" y="3" rx="1" />
      <rect width="7" height="7" x="14" y="14" rx="1" />
      <rect width="7" height="7" x="3" y="14" rx="1" />
    </>
  ),
  // L [shapes]
  art: (
    <>
      <path d="M8.3 10a.7.7 0 0 1-.626-1.079L11.4 3a.7.7 0 0 1 1.198-.043L16.3 8.9a.7.7 0 0 1-.572 1.1Z" />
      <rect x="3" y="14" width="7" height="7" rx="1" />
      <circle cx="17.5" cy="17.5" r="3.5" />
    </>
  ),
  // L [square-plus]
  create: (
    <>
      <rect width="18" height="18" x="3" y="3" rx="2" />
      <path d="M8 12h8" />
      <path d="M12 8v8" />
    </>
  ),
  // L [trash-2]
  delete: (
    <>
      <path d="M3 6h18" />
      <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
      <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
      <line x1="10" x2="10" y1="11" y2="17" />
      <line x1="14" x2="14" y1="11" y2="17" />
    </>
  ),
  // L [route]
  preview: (
    <>
      <circle cx="6" cy="19" r="3" />
      <path d="M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15" />
      <circle cx="18" cy="5" r="3" />
    </>
  ),
  // L [scan]
  frame: (
    <>
      <path d="M3 7V5a2 2 0 0 1 2-2h2" />
      <path d="M17 3h2a2 2 0 0 1 2 2v2" />
      <path d="M21 17v2a2 2 0 0 1-2 2h-2" />
      <path d="M7 21H5a2 2 0 0 1-2-2v-2" />
    </>
  ),
  // T [frame-off]
  frameStop: (
    <>
      <path d="M4 7h3m4 0h9" />
      <path d="M4 17h13" />
      <path d="M7 7v13" />
      <path d="M17 4v9m0 4v3" />
      <path d="M3 3l18 18" />
    </>
  ),
  // L [play]
  run: <polygon points="6 3 20 12 6 21 6 3" />,
  play: <polygon points="6 3 20 12 6 21 6 3" />,
  // L [pause]
  pause: (
    <>
      <rect x="14" y="4" width="4" height="16" rx="1" />
      <rect x="6" y="4" width="4" height="16" rx="1" />
    </>
  ),
  // L [square]
  stop: <rect width="18" height="18" x="3" y="3" rx="2" />,
  // T [flame]
  laserTools: <path d="M12 12c2 -2.96 0 -7 -1 -8c0 3.038 -1.773 4.741 -3 6c-1.226 1.26 -2 3.24 -2 5a6 6 0 1 0 12 0c0 -1.532 -1.056 -3.94 -2 -5c-1.786 3 -2.791 3 -4 2z" />,
  // L [undo-2] / [redo-2]
  undo: (
    <>
      <path d="M9 14 4 9l5-5" />
      <path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11" />
    </>
  ),
  redo: (
    <>
      <path d="m15 14 5-5-5-5" />
      <path d="M20 9H9.5A5.5 5.5 0 0 0 4 14.5A5.5 5.5 0 0 0 9.5 20H13" />
    </>
  ),
  // L [x]
  close: (
    <>
      <path d="M18 6 6 18" />
      <path d="m6 6 12 12" />
    </>
  ),
  // L [eye] / [eye-off]
  eye: (
    <>
      <path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  eyeOff: (
    <>
      <path d="M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49" />
      <path d="M14.084 14.158a3 3 0 0 1-4.242-4.242" />
      <path d="M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143" />
      <path d="m2 2 20 20" />
    </>
  ),
  // L [crosshair] / [locate-off]
  laser: (
    <>
      <circle cx="12" cy="12" r="10" />
      <line x1="22" x2="18" y1="12" y2="12" />
      <line x1="6" x2="2" y1="12" y2="12" />
      <line x1="12" x2="12" y1="6" y2="2" />
      <line x1="12" x2="12" y1="22" y2="18" />
    </>
  ),
  noLaser: (
    <>
      <line x1="2" x2="5" y1="12" y2="12" />
      <line x1="19" x2="22" y1="12" y2="12" />
      <line x1="12" x2="12" y1="2" y2="5" />
      <line x1="12" x2="12" y1="19" y2="22" />
      <path d="M7.11 7.11C5.83 8.39 5 10.1 5 12c0 3.87 3.13 7 7 7 1.9 0 3.61-.83 4.89-2.11" />
      <path d="M18.71 13.96c.19-.63.29-1.29.29-1.96 0-3.87-3.13-7-7-7-.67 0-1.33.1-1.96.29" />
      <line x1="2" x2="22" y1="2" y2="22" />
    </>
  ),
  // L [panel-left] / [panel-right]
  panelLeft: (
    <>
      <rect width="18" height="18" x="3" y="3" rx="2" />
      <path d="M9 3v18" />
    </>
  ),
  panelRight: (
    <>
      <rect width="18" height="18" x="3" y="3" rx="2" />
      <path d="M15 3v18" />
    </>
  ),
  // L [plus] / [minus]
  plus: (
    <>
      <path d="M5 12h14" />
      <path d="M12 5v14" />
    </>
  ),
  minus: <path d="M5 12h14" />,
  // L [square-terminal]
  terminal: (
    <>
      <path d="m7 11 2-2-2-2" />
      <path d="M11 13h4" />
      <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
    </>
  ),
  // L [search]
  search: (
    <>
      <circle cx="11" cy="11" r="8" />
      <path d="m21 21-4.3-4.3" />
    </>
  ),
  // L [maximize] / [minimize]
  fit: (
    <>
      <path d="M8 3H5a2 2 0 0 0-2 2v3" />
      <path d="M21 8V5a2 2 0 0 0-2-2h-3" />
      <path d="M3 16v3a2 2 0 0 0 2 2h3" />
      <path d="M16 21h3a2 2 0 0 0 2-2v-3" />
    </>
  ),
  fullscreen: (
    <>
      <path d="M8 3H5a2 2 0 0 0-2 2v3" />
      <path d="M21 8V5a2 2 0 0 0-2-2h-3" />
      <path d="M3 16v3a2 2 0 0 0 2 2h3" />
      <path d="M16 21h3a2 2 0 0 0 2-2v-3" />
    </>
  ),
  fullscreenExit: (
    <>
      <path d="M8 3v3a2 2 0 0 1-2 2H3" />
      <path d="M21 8h-3a2 2 0 0 1-2-2V3" />
      <path d="M3 16h3a2 2 0 0 1 2 2v3" />
      <path d="M16 21v-3a2 2 0 0 1 2-2h3" />
    </>
  ),
  // L [chevron-right] / [chevron-down] / [chevron-up] / [chevron-left]
  chevron: <path d="m9 18 6-6-6-6" />,
  chevronDown: <path d="m6 9 6 6 6-6" />,
  chevronUp: <path d="m18 15-6-6-6 6" />,
  prev: <path d="m15 18-6-6 6-6" />,
  next: <path d="m9 18 6-6-6-6" />,
  // L [upload]
  upload: (
    <>
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <polyline points="17 8 12 3 7 8" />
      <line x1="12" x2="12" y1="3" y2="15" />
    </>
  ),
  // L [skip-back] / [skip-forward]
  first: (
    <>
      <polygon points="19 20 9 12 19 4 19 20" />
      <line x1="5" x2="5" y1="19" y2="5" />
    </>
  ),
  last: (
    <>
      <polygon points="5 4 15 12 5 20 5 4" />
      <line x1="19" x2="19" y1="5" y2="19" />
    </>
  ),
  // L [align-start-vertical] [align-center-vertical] [align-end-vertical]
  alignLeft: (
    <>
      <rect width="9" height="6" x="6" y="14" rx="2" />
      <rect width="16" height="6" x="6" y="4" rx="2" />
      <path d="M2 2v20" />
    </>
  ),
  alignHCenter: (
    <>
      <path d="M12 2v20" />
      <path d="M8 10H4a2 2 0 0 1-2-2V6c0-1.1.9-2 2-2h4" />
      <path d="M16 10h4a2 2 0 0 0 2-2V6a2 2 0 0 0-2-2h-4" />
      <path d="M8 20H7a2 2 0 0 1-2-2v-2c0-1.1.9-2 2-2h1" />
      <path d="M16 14h1a2 2 0 0 1 2 2v2a2 2 0 0 1-2 2h-1" />
    </>
  ),
  alignRight: (
    <>
      <rect width="16" height="6" x="2" y="4" rx="2" />
      <rect width="9" height="6" x="9" y="14" rx="2" />
      <path d="M22 22V2" />
    </>
  ),
  // L [align-start-horizontal] [align-center-horizontal] [align-end-horizontal]
  alignTop: (
    <>
      <rect width="6" height="16" x="4" y="6" rx="2" />
      <rect width="6" height="9" x="14" y="6" rx="2" />
      <path d="M22 2H2" />
    </>
  ),
  alignVCenter: (
    <>
      <path d="M2 12h20" />
      <path d="M10 16v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-4" />
      <path d="M10 8V4a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v4" />
      <path d="M20 16v1a2 2 0 0 1-2 2h-2a2 2 0 0 1-2-2v-1" />
      <path d="M14 8V7c0-1.1.9-2 2-2h2a2 2 0 0 1 2 2v1" />
    </>
  ),
  alignBottom: (
    <>
      <rect width="6" height="16" x="4" y="2" rx="2" />
      <rect width="6" height="9" x="14" y="9" rx="2" />
      <path d="M22 22H2" />
    </>
  ),
  // L [align-horizontal-space-around] [align-horizontal-distribute-center]
  distH: (
    <>
      <rect width="6" height="10" x="9" y="7" rx="2" />
      <path d="M4 22V2" />
      <path d="M20 22V2" />
    </>
  ),
  distHc: (
    <>
      <rect width="6" height="14" x="4" y="5" rx="2" />
      <rect width="6" height="10" x="14" y="7" rx="2" />
      <path d="M17 22v-5" />
      <path d="M17 7V2" />
      <path d="M7 22v-3" />
      <path d="M7 5V2" />
    </>
  ),
  // L [align-vertical-space-around] [align-vertical-distribute-center]
  distV: (
    <>
      <rect width="10" height="6" x="7" y="9" rx="2" />
      <path d="M22 20H2" />
      <path d="M22 4H2" />
    </>
  ),
  distVc: (
    <>
      <path d="M22 17h-3" />
      <path d="M22 7h-5" />
      <path d="M5 17H2" />
      <path d="M7 7H2" />
      <rect x="5" y="14" width="14" height="6" rx="2" />
      <rect x="7" y="4" width="10" height="6" rx="2" />
    </>
  ),
  // T [focus-centered]
  centerStock: (
    <>
      <path d="M12 12m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
      <path d="M4 8v-2a2 2 0 0 1 2 -2h2" />
      <path d="M4 16v2a2 2 0 0 0 2 2h2" />
      <path d="M16 4h2a2 2 0 0 1 2 2v2" />
      <path d="M16 20h2a2 2 0 0 0 2 -2v-2" />
    </>
  ),
  // L [focus]
  centerField: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M3 7V5a2 2 0 0 1 2-2h2" />
      <path d="M17 3h2a2 2 0 0 1 2 2v2" />
      <path d="M21 17v2a2 2 0 0 1-2 2h-2" />
      <path d="M7 21H5a2 2 0 0 1-2-2v-2" />
    </>
  ),
  // L [group] / [ungroup]
  group: (
    <>
      <path d="M3 7V5c0-1.1.9-2 2-2h2" />
      <path d="M17 3h2c1.1 0 2 .9 2 2v2" />
      <path d="M21 17v2c0 1.1-.9 2-2 2h-2" />
      <path d="M7 21H5c-1.1 0-2-.9-2-2v-2" />
      <rect width="7" height="5" x="7" y="7" rx="1" />
      <rect width="7" height="5" x="10" y="12" rx="1" />
    </>
  ),
  ungroup: (
    <>
      <rect width="8" height="6" x="5" y="4" rx="1" />
      <rect width="8" height="6" x="11" y="14" rx="1" />
    </>
  ),
  // L [flip-horizontal-2] / [flip-vertical-2]
  flipH: (
    <>
      <path d="m3 7 5 5-5 5V7" />
      <path d="m21 7-5 5 5 5V7" />
      <path d="M12 20v2" />
      <path d="M12 14v2" />
      <path d="M12 8v2" />
      <path d="M12 2v2" />
    </>
  ),
  flipV: (
    <>
      <path d="m17 3-5 5-5-5h10" />
      <path d="m17 21-5-5-5 5h10" />
      <path d="M4 12H2" />
      <path d="M10 12H8" />
      <path d="M16 12h-2" />
      <path d="M22 12h-2" />
    </>
  ),
  // L [magnet]
  magnet: (
    <>
      <path d="m6 15-4-4 6.75-6.77a7.79 7.79 0 0 1 11 11L13 22l-4-4 6.39-6.36a2.14 2.14 0 0 0-3-3L6 15" />
      <path d="m5 8 4 4" />
      <path d="m12 15 4 4" />
    </>
  ),
  // L [grid-3x3]
  gridSnap: (
    <>
      <rect width="18" height="18" x="3" y="3" rx="2" />
      <path d="M3 9h18" />
      <path d="M3 15h18" />
      <path d="M9 3v18" />
      <path d="M15 3v18" />
    </>
  ),
  // L [paint-bucket]
  filled: (
    <>
      <path d="m19 11-8-8-8.6 8.6a2 2 0 0 0 0 2.8l5.2 5.2c.8.8 2 .8 2.8 0L19 11Z" />
      <path d="m5 2 5 5" />
      <path d="M2 13h15" />
      <path d="M22 20a2 2 0 1 1-4 0c0-1.6 1.7-2.4 2-4 .3 1.6 2 2.4 2 4Z" />
    </>
  ),
  // L [ellipsis]
  more: (
    <>
      <circle cx="12" cy="12" r="1" />
      <circle cx="19" cy="12" r="1" />
      <circle cx="5" cy="12" r="1" />
    </>
  ),
  // L [menu]
  menu: (
    <>
      <line x1="4" x2="20" y1="12" y2="12" />
      <line x1="4" x2="20" y1="6" y2="6" />
      <line x1="4" x2="20" y1="18" y2="18" />
    </>
  ),
  // L [history]
  history: (
    <>
      <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
      <path d="M3 3v5h5" />
      <path d="M12 7v5l4 2" />
    </>
  ),
  // L [save]
  save: (
    <>
      <path d="M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z" />
      <path d="M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7" />
      <path d="M7 3v4a1 1 0 0 0 1 1h7" />
    </>
  ),
  // L [globe]
  web: (
    <>
      <circle cx="12" cy="12" r="10" />
      <path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20" />
      <path d="M2 12h20" />
    </>
  ),

  // ---- shell, touch HUD and row controls (formerly the separate ShellIcon / TouchIcon sets) ----
  // L [copy]
  copy: (
    <>
      <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
      <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
    </>
  ),
  // L [clipboard-paste]
  paste: (
    <>
      <path d="M15 2H9a1 1 0 0 0-1 1v2c0 .6.4 1 1 1h6c.6 0 1-.4 1-1V3c0-.6-.4-1-1-1Z" />
      <path d="M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2M16 4h2a2 2 0 0 1 2 2v2M11 14h10" />
      <path d="m17 10 4 4-4 4" />
    </>
  ),
  // L [copy-plus]
  duplicate: (
    <>
      <path d="M15 12v6M12 15h6" />
      <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
      <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
    </>
  ),
  // L [rotate-cw]
  rotate: (
    <>
      <path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" />
      <path d="M21 3v5h-5" />
    </>
  ),
  // L [arrow-left] / [arrow-right] / [arrow-up] / [arrow-down]
  arrowLeft: (
    <>
      <path d="m12 19-7-7 7-7" />
      <path d="M19 12H5" />
    </>
  ),
  arrowRight: (
    <>
      <path d="M5 12h14" />
      <path d="m12 5 7 7-7 7" />
    </>
  ),
  arrowUp: (
    <>
      <path d="m5 12 7-7 7 7" />
      <path d="M12 19V5" />
    </>
  ),
  arrowDown: (
    <>
      <path d="M12 5v14" />
      <path d="m19 12-7 7-7-7" />
    </>
  ),
  // L [grip-vertical]
  grip: (
    <>
      <circle cx="9" cy="12" r="1" />
      <circle cx="9" cy="5" r="1" />
      <circle cx="9" cy="19" r="1" />
      <circle cx="15" cy="12" r="1" />
      <circle cx="15" cy="5" r="1" />
      <circle cx="15" cy="19" r="1" />
    </>
  ),
  // L [pencil]
  pencil: (
    <>
      <path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z" />
      <path d="m15 5 4 4" />
    </>
  ),
  // L [zap]
  zap: (
    <path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z" />
  ),
  // L [layers]
  layers: (
    <>
      <path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z" />
      <path d="M22 17.65l-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65" />
      <path d="M22 12.65l-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65" />
    </>
  ),
  // L [mouse-pointer-2]
  pointer: <path d="M4.037 4.688a.495.495 0 0 1 .651-.651l16 6.5a.5.5 0 0 1-.063.947l-6.124 1.58a2 2 0 0 0-1.438 1.435l-1.579 6.126a.5.5 0 0 1-.947.063z" />,
  // L [pin]
  pin: (
    <>
      <path d="M12 17v5" />
      <path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" />
    </>
  ),
  // L [sliders-horizontal]
  sliders: <path d="M21 4h-7 M10 4H3 M21 12h-9 M8 12H3 M21 20h-5 M12 20H3 M14 2v4 M8 10v4 M16 18v4" />,
  // L [monitor]
  monitor: (
    <>
      <rect width="20" height="14" x="2" y="3" rx="2" />
      <path d="M8 21h8 M12 17v4" />
    </>
  ),
  // L [move]
  move: <path d="M12 2v20 M15 19l-3 3-3-3 M19 9l3 3-3 3 M2 12h20 M5 9l-3 3 3 3 M9 5l3-3 3 3" />,
  // L [scaling]
  scale: <path d="M21 3L9 15 M12 3H3v18h18v-9 M16 3h5v5 M14 15H9v-5" />,
  // L [type]
  type: <path d="M4 7V4h16v3 M9 20h6 M12 4v16" />,
  // L [tag]
  tag: (
    <>
      <path d="M12.586 2.586A2 2 0 0 0 11.172 2H4a2 2 0 0 0-2 2v7.172a2 2 0 0 0 .586 1.414l8.704 8.704a2.426 2.426 0 0 0 3.42 0l6.58-6.58a2.426 2.426 0 0 0 0-3.42z" />
      <path d="M7.5 7.5h.01" />
    </>
  ),
  // L [file]
  file: (
    <>
      <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z" />
      <path d="M14 2v4a2 2 0 0 0 2 2h4" />
    </>
  ),
  // L [sun] / [moon]; the theme switch's "System" is a monitor with a half-filled disc
  sun: <path d="M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z M12 2v2 M12 20v2 M4.93 4.93l1.41 1.41 M17.66 17.66l1.41 1.41 M2 12h2 M20 12h2 M6.34 17.66l-1.41 1.41 M19.07 4.93l-1.41 1.41" />,
  moon: <path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" />,
  themeSystem: <path d="M4 5h16v11H4z M9 20h6 M12 16v4 M12 7.5a3 3 0 0 0 0 6z" />,
  // L [settings]
  settings: (
    <>
      <path d="M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z" />
      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
    </>
  ),
  // L [lock] / [lock-open] (simplified)
  lock: <path d="M6 11h12v9H6z M8.5 11V8a3.5 3.5 0 0 1 7 0v3" />,
  unlock: <path d="M6 11h12v9H6z M8.5 11V8a3.5 3.5 0 0 1 6.8-1.2" />,
  // L [link]
  link: <path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1 M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" />,
  // W [warning] (Lucide triangle-alert)
  warning: <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3 M12 9v4 M12 17h.01" />,
  // L [check]
  check: <path d="M20 6 9 17l-5-5" />,
  // L [star] (font favourites; filled by CSS when on)
  star: <path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z" />,
  // L [book-open] (example projects)
  book: (
    <>
      <path d="M12 7v14" />
      <path d="M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z" />
    </>
  ),
  // L [layout-grid] (the contact sheet)
  grid: (
    <>
      <rect width="7" height="7" x="3" y="3" rx="1" />
      <rect width="7" height="7" x="14" y="3" rx="1" />
      <rect width="7" height="7" x="14" y="14" rx="1" />
      <rect width="7" height="7" x="3" y="14" rx="1" />
    </>
  ),
  // L [clipboard-pen-line] (fill in a template)
  fillIn: (
    <>
      <rect width="8" height="4" x="8" y="2" rx="1" />
      <path d="M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h5.5" />
      <path d="M16 4h2a2 2 0 0 1 1.73 1" />
      <path d="M8 18h1" />
      <path d="M21.378 12.626a1 1 0 0 0-3.004-3.004l-4.01 4.012a2 2 0 0 0-.506.854l-.837 2.87a.5.5 0 0 0 .62.62l2.87-.837a2 2 0 0 0 .854-.506z" />
    </>
  ),
};

/** Every registered icon name. */
export const ICON_NAMES: readonly string[] = Object.keys(ICONS);

/** Stroke icon, 24x24 viewBox, inherits `currentColor`. */
export function Icon({ name, size = 20 }: { name: keyof typeof ICONS | string; size?: number }) {
  // Preferences > Icon size scales every icon through --icon-scale (workspace/usePrefs.ts).
  const scaled = `calc(${size}px * var(--icon-scale, 1))`;
  return (
    <svg
      className="icon"
      width={size}
      height={size}
      style={{ width: scaled, height: scaled }}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {ICONS[name] ?? null}
    </svg>
  );
}
