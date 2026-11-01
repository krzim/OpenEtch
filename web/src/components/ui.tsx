// Small shared UI pieces: modal dialog, commit-on-blur fields, and the toolbar icon set. Plain CSS, no libraries.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { evalNumber } from "../editing.ts";

/**
 * Centered modal with a title, body and footer. Escape or a backdrop tap cancels; with `onSubmit`, Enter in a text
 * field submits.
 */
export function Modal(props: { title: string; onClose: () => void; children: ReactNode; footer?: ReactNode; wide?: boolean; onSubmit?: () => void }) {
  const { onClose } = props;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="modal-backdrop" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        className={`modal${props.wide ? " modal-wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-label={props.title}
        onKeyDown={(e) => {
          const t = e.target as HTMLElement;
          if (e.key === "Enter" && props.onSubmit && t.tagName === "INPUT" && (t as HTMLInputElement).type !== "checkbox") {
            e.preventDefault();
            props.onSubmit();
          }
        }}
      >
        <div className="modal-head">
          <span>{props.title}</span>
          <button className="icon-btn" onClick={onClose} aria-label="Close">
            <Icon name="close" />
          </button>
        </div>
        <div className="modal-body">{props.children}</div>
        {props.footer && <div className="modal-foot">{props.footer}</div>}
      </div>
    </div>
  );
}

/** Format a number for a field: up to `digits` decimals, no trailing zeros. */
export function fmt(v: number | null | undefined, digits = 3): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return "";
  const r = Number(v.toFixed(digits));
  return Object.is(r, -0) ? "0" : String(r);
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
}) {
  const { value, digits = 3 } = props;
  const [text, setText] = useState(fmt(value, digits));
  const [clamped, setClamped] = useState<string | null>(null);
  const editing = useRef(false);
  useEffect(() => {
    if (!editing.current) setText(fmt(value, digits));
  }, [value, digits]);
  useEffect(() => {
    if (!clamped) return;
    const t = window.setTimeout(() => setClamped(null), 3000);
    return () => window.clearTimeout(t);
  }, [clamped]);
  const commit = () => {
    editing.current = false;
    const t = text.trim();
    if (t === "") {
      if (props.allowEmpty && value !== null) props.onCommit(null);
      else setText(fmt(value, digits));
      return;
    }
    let v = evalNumber(t);
    if (v === null) {
      setText(fmt(value, digits));
      return;
    }
    const typed = v;
    if (props.min !== undefined) v = Math.max(props.min, v);
    if (props.max !== undefined) v = Math.min(props.max, v);
    setClamped(v !== typed ? `Limited to ${fmt(v, digits)}${props.unit ? ` ${props.unit}` : ""}` : null);
    setText(fmt(v, digits));
    if (value === null || Math.abs(v - value) > 1e-9) props.onCommit(v);
  };
  return (
    <label className="field">
      <span className="field-label">{props.label}</span>
      <span className="field-input">
        <input
          type="text"
          inputMode="decimal"
          value={text}
          placeholder={props.placeholder}
          disabled={props.disabled}
          onFocus={() => (editing.current = true)}
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            if (e.key === "Escape") {
              editing.current = false;
              setText(fmt(value, digits));
              (e.target as HTMLInputElement).blur();
            }
          }}
        />
        {props.unit && <span className="unit">{props.unit}</span>}
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
export function PairField(props: { labels: [string, string]; unit?: string; value: [number, number]; onCommit: (v: [number, number]) => void; digits?: number }) {
  const { value, digits = 3 } = props;
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
    const r = (v: number) => Math.round(v * 1000) / 1000;
    const next: [number, number] = [r(a), r(b)];
    setText([fmt(next[0], digits), fmt(next[1], digits)]);
    if (Math.abs(next[0] - value[0]) > 5e-4 || Math.abs(next[1] - value[1]) > 5e-4) props.onCommit(next);
  };
  const box = (i: 0 | 1) => (
    <label className="field">
      <span className="field-label">{props.labels[i]}</span>
      <span className="field-input">
        <input
          type="text"
          inputMode="decimal"
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
        {props.unit && <span className="unit">{props.unit}</span>}
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

const ICONS: Record<string, ReactNode> = {
  stock: <path d="M3 8l9-4 9 4v8l-9 4-9-4z M3 8l9 4 9-4 M12 12v8" />,
  rect: <rect x="4" y="6" width="16" height="12" rx="1" />,
  ellipse: <ellipse cx="12" cy="12" rx="9" ry="6" />,
  import: <path d="M12 3v12 M7 10l5 5 5-5 M4 17v3h16v-3" />,
  place: <path d="M4 4h16v16H4z M9 9h6v6H9z M12 4v5 M12 15v5 M4 12h5 M15 12h5" />,
  array: <path d="M4 4h5v5H4z M15 4h5v5h-5z M4 15h5v5H4z M15 15h5v5h-5z" />,
  delete: <path d="M5 7h14 M9 7V4h6v3 M7 7l1 13h8l1-13" />,
  preview: <path d="M3 17l5-6 4 4 4-7 5 9 M3 20h18" />,
  frame: <path d="M4 9V4h5 M15 4h5v5 M20 15v5h-5 M9 20H4v-5" />,
  frameStop: <path d="M4 9V4h5 M15 4h5v5 M20 15v5h-5 M9 20H4v-5 M9.5 9.5h5v5h-5z" />,
  run: <path d="M7 4l12 8-12 8z" />,
  stop: <rect x="6" y="6" width="12" height="12" rx="1" />,
  undo: <path d="M9 14L4 9l5-5 M4 9h11a5 5 0 010 10h-3" />,
  redo: <path d="M15 14l5-5-5-5 M20 9H9a5 5 0 000 10h3" />,
  close: <path d="M6 6l12 12 M18 6L6 18" />,
  eye: <path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z M12 9a3 3 0 100 6 3 3 0 000-6z" />,
  eyeOff: <path d="M3 3l18 18 M10.6 5.1A10 10 0 0112 5c6 0 10 7 10 7a17 17 0 01-3 3.7 M6.6 6.6C3.8 8.5 2 12 2 12s4 7 10 7a9.6 9.6 0 004.4-1" />,
  laser: <path d="M12 2v6 M12 16v6 M2 12h6 M16 12h6 M12 9a3 3 0 100 6 3 3 0 000-6z" />,
  noLaser: <path d="M12 9a3 3 0 100 6 3 3 0 000-6z M4 4l16 16" />,
  panelLeft: <path d="M3 4h18v16H3z M9 4v16" />,
  panelRight: <path d="M3 4h18v16H3z M15 4v16" />,
  plus: <path d="M12 5v14 M5 12h14" />,
  terminal: <path d="M4 5h16v14H4z M7 9l3 3-3 3 M12 15h5" />,
  fit: <path d="M4 9V4h5 M15 4h5v5 M20 15v5h-5 M9 20H4v-5 M9 9h6v6H9z" />,
  chevron: <path d="M9 6l6 6-6 6" />,
  chevronDown: <path d="M6 9l6 6 6-6" />,
  upload: <path d="M12 16V4 M7 9l5-5 5 5 M4 17v3h16v-3" />,
  first: <path d="M6 5v14 M18 6l-8 6 8 6z" />,
  prev: <path d="M15 6l-6 6 6 6" />,
  next: <path d="M9 6l6 6-6 6" />,
  last: <path d="M18 5v14 M6 6l8 6-8 6z" />,
  play: <path d="M8 5l11 7-11 7z" />,
  pause: <path d="M8 5v14 M16 5v14" />,
  alignLeft: <path d="M4 3v18 M7 7h10v4H7z M7 14h6v4H7z" />,
  alignHCenter: <path d="M12 3v18 M6 7h12v4H6z M8 14h8v4H8z" />,
  alignRight: <path d="M20 3v18 M7 7h10v4H7z M11 14h6v4h-6z" />,
  alignTop: <path d="M3 4h18 M7 7v10h4V7z M14 7v6h4V7z" />,
  alignVCenter: <path d="M3 12h18 M7 6v12h4V6z M14 8v8h4V8z" />,
  alignBottom: <path d="M3 20h18 M7 7v10h4V7z M14 11v6h4v-6z" />,
  distH: <path d="M3 4v16 M21 4v16 M9 8h6v8H9z" />,
  distV: <path d="M4 3h16 M4 21h16 M8 9h8v6H8z" />,
  centerStock: <path d="M3 6h18v12H3z M12 9v6 M9 12h6" />,
  centerField: <path d="M4 4h16v16H4z M12 8v8 M8 12h8 M4 4l3 3 M20 4l-3 3 M4 20l3-3 M20 20l-3-3" />,
  group: <path d="M3 3h4 M3 3v4 M21 3h-4 M21 3v4 M3 21h4 M3 21v-4 M21 21h-4 M21 21v-4 M7 7h5v5H7z M12 12h5v5h-5z" />,
  ungroup: <path d="M4 4h6v6H4z M14 14h6v6h-6z M14 4l6 6 M20 4l-6 6" />,
  flipH: <path d="M12 3v18 M9 6L3 18h6z M15 6l6 12h-6z" />,
  flipV: <path d="M3 12h18 M6 9L18 3v6z M6 15l12 6v-6z" />,
  magnet: <path d="M6 4v8a6 6 0 0012 0V4 M6 4h4v8a2 2 0 004 0V4h4 M6 8h4 M14 8h4" />,
  gridSnap: <path d="M4 4h16v16H4z M4 9.3h16 M4 14.6h16 M9.3 4v16 M14.6 4v16" />,
  filled: <path d="M4 6h10v10H4z M10 10h10v10H10z M4 6l10 10" />,
  more: <path d="M5 12h.01 M12 12h.01 M19 12h.01" />,
};

/** Stroke icon, 24x24 viewBox, inherits `currentColor`. */
export function Icon({ name, size = 20 }: { name: keyof typeof ICONS | string; size?: number }) {
  return (
    <svg
      className="icon"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {ICONS[name] ?? null}
    </svg>
  );
}
