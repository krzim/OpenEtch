// Small building blocks shared by the files dialogs: a segmented choice, a number box that keeps its text while typed
// (parsed on submit, so a click on the dialog's button never sees a stale value), a length box in the display unit,
// and a hidden file picker.

import { forwardRef, useState, type ReactNode } from "react";
import { evalNumber } from "../editing.ts";
import { UnitTextField } from "../components/ui.tsx";

export function Segmented<T extends string>(p: { value: T; options: { value: T; label: string; title?: string; disabled?: boolean }[]; onChange: (v: T) => void; label: string }) {
  return (
    <div className="segmented" role="radiogroup" aria-label={p.label}>
      {p.options.map((o) => (
        <button
          key={o.value}
          role="radio"
          aria-checked={p.value === o.value}
          className={p.value === o.value ? "active" : ""}
          title={o.title}
          disabled={o.disabled}
          onClick={() => p.onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** Field row: label on the left, control on the right (the app's `.field` layout). */
export function Row(p: { label: string; children: ReactNode; title?: string }) {
  return (
    <div className="field" title={p.title}>
      <span className="field-label">{p.label}</span>
      <span className="field-input">{p.children}</span>
    </div>
  );
}

/** A number typed as text; `parse` reads it (null when empty or not a number). */
export function NumBox(p: { label: string; value: string; onChange: (v: string) => void; unit?: string; title?: string; placeholder?: string }) {
  return (
    <label className="field" title={p.title}>
      <span className="field-label">{p.label}</span>
      <span className="field-input">
        <input type="text" inputMode="decimal" value={p.value} placeholder={p.placeholder} onChange={(e) => p.onChange(e.target.value)} />
        {p.unit && <span className="unit">{p.unit}</span>}
      </span>
    </label>
  );
}

/** A length in mm shown in the display unit (Preferences). */
export function MmBox(p: { label: string; value: string; onChange: (v: string) => void; title?: string }) {
  return <UnitTextField label={p.label} value={p.value} onChange={p.onChange} unit="mm" title={p.title} />;
}

/** Parse a number box: null when empty, NaN when it is not a number. */
export function parseBox(text: string): number | null {
  const t = text.trim();
  if (!t) return null;
  const n = evalNumber(t);
  return n === null ? Number.NaN : n;
}

/** A useState for a number box, starting from a number. */
export function useBox(initial: number | null): [string, (v: string) => void] {
  return useState(initial === null ? "" : String(initial));
}

/** Hidden `<input type=file>`; call `.click()` on the ref. */
export const FilePick = forwardRef<HTMLInputElement, { accept: string; multiple?: boolean; onFiles: (f: File[]) => void }>(function FilePick(p, ref) {
  return (
    <input
      ref={ref}
      type="file"
      accept={p.accept}
      multiple={p.multiple}
      hidden
      onChange={(e) => {
        const f = Array.from(e.target.files ?? []);
        e.target.value = "";
        if (f.length) p.onFiles(f);
      }}
    />
  );
});
