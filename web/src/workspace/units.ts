// Display units (LightBurn's Units setting): lengths in mm or inches, speeds in mm/s, mm/min or in/s. Conversion
// happens only in the UI, on the way into and out of fields; every command still carries millimetres and mm/s.

import type { LengthUnit, SpeedUnit } from "./prefs.ts";

export const MM_PER_IN = 25.4;

export function lengthToDisplay(mm: number, u: LengthUnit): number {
  return u === "in" ? mm / MM_PER_IN : mm;
}

export function lengthFromDisplay(v: number, u: LengthUnit): number {
  return u === "in" ? v * MM_PER_IN : v;
}

export function speedToDisplay(mmPerS: number, u: SpeedUnit): number {
  return u === "mm/min" ? mmPerS * 60 : u === "in/s" ? mmPerS / MM_PER_IN : mmPerS;
}

export function speedFromDisplay(v: number, u: SpeedUnit): number {
  return u === "mm/min" ? v / 60 : u === "in/s" ? v * MM_PER_IN : v;
}

/** Decimals worth showing for a length in `u` (inches need one more for the same precision). */
export function lengthDigits(u: LengthUnit, mmDigits = 3): number {
  return u === "in" ? mmDigits + 1 : mmDigits;
}

/** How a field labelled with an engine unit is shown: its display unit and the two conversions. */
export interface UnitConversion {
  unit: string | undefined;
  to: (v: number) => number;
  from: (v: number) => number;
  /** Extra decimals over the field's own. */
  extraDigits: number;
}

const IDENTITY = (v: number) => v;

/** Conversion for a field whose engine unit is `unit` ("mm" and "mm/s" convert; anything else is shown as is). */
export function unitConversion(unit: string | undefined, prefs: { units: LengthUnit; speedUnit: SpeedUnit }): UnitConversion {
  if (unit === "mm" && prefs.units === "in") {
    return { unit: "in", to: (v) => lengthToDisplay(v, "in"), from: (v) => lengthFromDisplay(v, "in"), extraDigits: 1 };
  }
  if (unit === "mm/s" && prefs.speedUnit !== "mm/s") {
    const su = prefs.speedUnit;
    return { unit: su, to: (v) => speedToDisplay(v, su), from: (v) => speedFromDisplay(v, su), extraDigits: su === "in/s" ? 1 : 0 };
  }
  return { unit, to: IDENTITY, from: IDENTITY, extraDigits: 0 };
}

/** "12.35" with at most `digits` decimals, no trailing zeros, no "-0". */
export function trimNumber(v: number, digits: number): string {
  if (!Number.isFinite(v)) return "";
  const r = Number(v.toFixed(digits));
  return Object.is(r, -0) ? "0" : String(r);
}

/** "12.35 mm" or "0.4862 in". */
export function formatLength(mm: number, u: LengthUnit, digits = 2): string {
  return `${trimNumber(lengthToDisplay(mm, u), lengthDigits(u, digits))} ${u}`;
}

/** "1000 mm/s", "60000 mm/min", "39.37 in/s". */
export function formatSpeed(mmPerS: number, u: SpeedUnit): string {
  return `${trimNumber(speedToDisplay(mmPerS, u), u === "in/s" ? 2 : 1)} ${u}`;
}

/** Fixed-width coordinate text for the status bar: "12.350". */
export function coord(mm: number, u: LengthUnit): string {
  return lengthToDisplay(mm, u).toFixed(u === "in" ? 4 : 3);
}

/** Zoom as a percentage: 100 % shows real size at 96 CSS px per inch. */
export function zoomPercent(pxPerMm: number): number {
  return (pxPerMm / (96 / MM_PER_IN)) * 100;
}
