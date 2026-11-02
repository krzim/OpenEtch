// Number entry on touch keyboards (docs/ux/ui-plan-2026-11.md 1.2 "Number entry"): fields that take a sign (X/Y,
// rotation, offsets) ask for the text keyboard, because iOS's decimal pad has no minus key and no "/" for `20/3`;
// unsigned fields keep the decimal pad. Steppers (−/+ with press-and-hold) on coarse pointers. Pure; tested in
// numericInput.test.ts.

export type NumericInputMode = "decimal" | "text";

/** The `inputMode` of a number field: "text" when it takes a sign (or maths), "decimal" otherwise. */
export function numericInputMode(signed: boolean): NumericInputMode {
  return signed ? "text" : "decimal";
}

/** The input attributes of a number field. */
export function numericInputProps(signed: boolean): { inputMode: NumericInputMode; enterKeyHint: "done"; autoComplete: "off"; spellCheck: false } {
  return { inputMode: numericInputMode(signed), enterKeyHint: "done", autoComplete: "off", spellCheck: false };
}

/**
 * One stepper press: `value` moved by `step` in direction `dir`, rounded to the step's decimals (no 0.30000000000000004)
 * and clamped to [min, max]. A null value starts from `min` (or 0).
 */
export function stepValue(value: number | null, step: number, dir: 1 | -1, min?: number, max?: number): number {
  const s = step > 0 && Number.isFinite(step) ? step : 1;
  const base = value ?? min ?? 0;
  const decimals = Math.min(10, Math.max(0, (String(s).split(".")[1] ?? "").length, (String(base).split(".")[1] ?? "").length));
  let v = Number((base + dir * s).toFixed(decimals));
  if (min !== undefined) v = Math.max(min, v);
  if (max !== undefined) v = Math.min(max, v);
  return v;
}

/** Press-and-hold timing: the first repeat after `HOLD_DELAY_MS`, then every `HOLD_REPEAT_MS`, faster after 10. */
export const HOLD_DELAY_MS = 400;
export const HOLD_REPEAT_MS = 80;

/** The delay before repeat number `n` (0 = the first repeat). */
export function holdDelay(n: number): number {
  if (n <= 0) return HOLD_DELAY_MS;
  return n > 10 ? HOLD_REPEAT_MS / 2 : HOLD_REPEAT_MS;
}
