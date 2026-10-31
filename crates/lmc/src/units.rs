//! Unit conversions between mm / mm/s / kHz / % / µs / ns and LMC words.
//!
//! Rounding follows galvoplotter's Python: `round()` is ties-to-even (`f64::round_ties_even`) and `int()` truncates.
//! Every function refuses a value it cannot represent ([`LmcError`]) instead of clamping or wrapping. The one
//! exception is [`distance_word`], a motion hint that galvoplotter itself saturates at `0xFFFF`.

// Ported from openlaser-rs @ 9d81a15 `crates/types/src/galvo.rs` (MIT OR Apache-2.0, used here under MIT), whose
// formulas come from meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (`_convert_speed`, `_convert_frequency`,
// `_convert_power`, `list_jump`, `list_*_delay`; MIT). Changes: one error type, speed and power refuse instead of
// saturating, no `_om` compatibility variants.
//
// MIT License
//
// Copyright (c) 2023 MeerK40t
// Copyright (c) 2026 Luiz Bueno and the OpenLaser contributors
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use serde::{Deserialize, Serialize};

use crate::{LmcError, Result};

/// Galvo word of the field centre.
pub const CENTER: u16 = 0x8000;

/// Mark current word for 100 % power.
pub const MARK_CURRENT_MAX: u16 = 4095;

/// Pulse widths (ns) a JPT M7 MOPA source accepts.
pub const JPT_M7_PULSE_WIDTHS_NS: [u16; 20] = [2, 4, 6, 9, 13, 20, 30, 45, 55, 60, 80, 100, 150, 200, 250, 300, 350, 400, 450, 500];

const WORD_MAX: f64 = 65535.0;

/// The one f64 → u16 narrowing of this crate. `w` must be a whole number in 0..=65535; anything else is refused.
/// `value` is what the error reports (the caller's input, not `w`).
fn whole_word(w: f64, what: &'static str, value: f64, allowed: &'static str) -> Result<u16> {
    if !w.is_finite() {
        return Err(LmcError::NonFinite { what });
    }
    if w.fract() != 0.0 || !(0.0..=WORD_MAX).contains(&w) {
        return Err(LmcError::OutOfRange { what, value, allowed });
    }
    // `w` is finite, whole and inside 0..=65535 (checked above), so the cast is exact; -0.0 becomes 0.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(w as u16)
}

fn finite(v: f64, what: &'static str) -> Result<f64> {
    if v.is_finite() { Ok(v) } else { Err(LmcError::NonFinite { what }) }
}

fn field(field_mm: f64) -> Result<f64> {
    let f = finite(field_mm, "field size")?;
    if f <= 0.0 {
        return Err(LmcError::OutOfRange { what: "field size", value: f, allowed: "> 0 mm" });
    }
    Ok(f)
}

/// Field-centred mm → galvo word: `round_ties_even(32768 + v × 65535 / field)`.
///
/// The encodable span is `-field/2 ..< +field/2`: `+field/2` would be 65536 and is refused, never clamped.
pub fn mm_to_galvo(v_mm: f64, field_mm: f64) -> Result<u16> {
    let v = finite(v_mm, "coordinate")?;
    let f = field(field_mm)?;
    let g = (32768.0 + v * (WORD_MAX / f)).round_ties_even();
    whole_word(g, "coordinate", v_mm, "-field/2 ..< +field/2")
}

/// Galvo word → field-centred mm (the inverse of [`mm_to_galvo`], up to rounding).
pub fn galvo_to_mm(w: u16, field_mm: f64) -> f64 {
    (f64::from(w) - 32768.0) * field_mm / WORD_MAX
}

/// mm/s → galvo units per ms: `trunc(v × (65535 / field) / 1000)`.
///
/// 0 is a legal word here (the list builders refuse a motion speed of 0). Above 65535 is refused (galvoplotter
/// saturates).
pub fn speed_to_units(mm_s: f64, field_mm: f64) -> Result<u16> {
    let v = finite(mm_s, "speed")?;
    let f = field(field_mm)?;
    if v < 0.0 {
        return Err(LmcError::OutOfRange { what: "speed", value: mm_s, allowed: ">= 0 mm/s" });
    }
    let w = (v * (WORD_MAX / f) / 1000.0).trunc();
    whole_word(w, "speed", mm_s, "<= 65535 galvo units/ms")
}

/// Galvo units per ms → mm/s (display).
pub fn units_to_speed(units: u16, field_mm: f64) -> f64 {
    f64::from(units) * field_mm * 1000.0 / WORD_MAX
}

/// Fiber Q-switch frequency (kHz) → period word: `round_ties_even(20000 / kHz)`.
///
/// Refused when the frequency is not positive, or the period is 0 (above 40 MHz) or above 65535 (below ~0.305 kHz).
pub fn frequency_to_period(khz: f64) -> Result<u16> {
    let f = finite(khz, "frequency")?;
    if f <= 0.0 {
        return Err(LmcError::OutOfRange { what: "frequency", value: khz, allowed: "> 0 kHz" });
    }
    let p = (20000.0 / f).round_ties_even();
    if p < 1.0 {
        return Err(LmcError::OutOfRange { what: "frequency", value: khz, allowed: "period >= 1 (<= 40000 kHz)" });
    }
    whole_word(p, "frequency", khz, "period <= 65535 (>= 0.31 kHz)")
}

/// Period word → kHz (display). A period of 0 has no frequency.
pub fn period_to_frequency(period: u16) -> Option<f64> {
    (period != 0).then(|| 20000.0 / f64::from(period))
}

/// Power percent → fiber mark current: `round_ties_even(pct × 4095 / 100)` (30 % → 1228). Only 0..=100 % is accepted.
pub fn power_to_mark_current(pct: f64) -> Result<u16> {
    let p = finite(pct, "power")?;
    if !(0.0..=100.0).contains(&p) {
        return Err(LmcError::OutOfRange { what: "power", value: pct, allowed: "0..=100 %" });
    }
    whole_word((p * 4095.0 / 100.0).round_ties_even(), "power", pct, "0..=100 %")
}

/// Mark current → power percent (display).
pub fn mark_current_to_power(current: u16) -> f64 {
    f64::from(current) * 100.0 / 4095.0
}

/// A delay as the board takes it: magnitude in µs plus a sign word (`0x0000` for > 0, `0x8000` otherwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DelayWord {
    /// |delay| in whole µs.
    pub magnitude: u16,
    /// [`DelayWord::POSITIVE`] or [`DelayWord::NEGATIVE`].
    pub sign: u16,
}

impl DelayWord {
    /// Sign word of a positive delay.
    pub const POSITIVE: u16 = 0x0000;
    /// Sign word of a zero or negative delay.
    pub const NEGATIVE: u16 = 0x8000;

    /// The delay in µs (the magnitude, negated when the sign word is [`Self::NEGATIVE`]).
    pub fn micros(self) -> f64 {
        let m = f64::from(self.magnitude);
        if self.sign == Self::NEGATIVE { -m } else { m }
    }
}

/// µs → [`DelayWord`] as galvoplotter writes it: `(trunc(|d|), if d > 0 { 0x0000 } else { 0x8000 })`.
///
/// So 0 → (0, 0x8000) and 0.4 → (0, 0x0000). Refused when `trunc(|d|) > 65535`.
pub fn delay_word(us: f64) -> Result<DelayWord> {
    let v = finite(us, "delay")?;
    let magnitude = whole_word(v.abs().trunc(), "delay", us, "|delay| < 65536 µs")?;
    let sign = if v > 0.0 { DelayWord::POSITIVE } else { DelayWord::NEGATIVE };
    Ok(DelayWord { magnitude, sign })
}

/// µs → `listDelayTime` / `listLaserOnPoint` word (10 µs units): `round_ties_even(us / 10)`. Negative is refused.
pub fn delay_time_units(us: f64) -> Result<u16> {
    let v = finite(us, "delay time")?;
    if v < 0.0 {
        return Err(LmcError::OutOfRange { what: "delay time", value: us, allowed: ">= 0 µs" });
    }
    whole_word((v / 10.0).round_ties_even(), "delay time", us, "<= 655350 µs")
}

/// Distance word of a jump or mark from `from` to `to`: `min(trunc(hypot(dx, dy)), 65535)` in galvo units.
///
/// Computed exactly in integers (`isqrt` of `dx² + dy²`), which equals galvoplotter's
/// `int(abs(complex(x, y) - complex(last_x, last_y)))` for every pair of 16-bit points.
pub fn distance_word(from: (u16, u16), to: (u16, u16)) -> u16 {
    let dx = u64::from(from.0.abs_diff(to.0));
    let dy = u64::from(from.1.abs_diff(to.1));
    u16::try_from((dx * dx + dy * dy).isqrt()).unwrap_or(u16::MAX)
}

/// What [`nearest_pulse_width`] decided for a requested pulse width.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PulseDecision {
    /// The request is a table entry.
    Exact(u16),
    /// Within 0.5 ns of a table entry (float noise, e.g. 199.8 → 200).
    Snapped { requested: f64, to: u16 },
    /// Further than 0.5 ns from every entry, or not finite, or the table is empty (`nearest` 0).
    Refused { requested: f64, nearest: u16 },
}

impl PulseDecision {
    /// The pulse width word to send: `Some` for `Exact` and `Snapped`, `None` for `Refused`.
    pub fn word(&self) -> Option<u16> {
        match *self {
            PulseDecision::Exact(w) | PulseDecision::Snapped { to: w, .. } => Some(w),
            PulseDecision::Refused { .. } => None,
        }
    }
}

/// Nearest entry of `table` to `ns`: ascending scan, the first minimum wins ties. |Δ| <= 0.5 ns snaps; further, or
/// a non-finite request, is refused.
pub fn nearest_pulse_width(ns: f64, table: &[u16]) -> PulseDecision {
    let mut best: Option<(u16, f64)> = None;
    for &t in table {
        let key = (f64::from(t) - ns).abs();
        if best.is_none_or(|(_, k)| key < k) {
            best = Some((t, key));
        }
    }
    let Some((nearest, distance)) = best else {
        return PulseDecision::Refused { requested: ns, nearest: 0 };
    };
    if !ns.is_finite() {
        PulseDecision::Refused { requested: ns, nearest }
    } else if ns == f64::from(nearest) {
        PulseDecision::Exact(nearest)
    } else if distance <= 0.5 {
        PulseDecision::Snapped { requested: ns, to: nearest }
    } else {
        PulseDecision::Refused { requested: ns, nearest }
    }
}

/// [`nearest_pulse_width`] as a `Result`: the word to send, or [`LmcError::PulseWidth`].
pub fn pulse_width_word(ns: f64, table: &[u16]) -> Result<u16> {
    match nearest_pulse_width(ns, table) {
        PulseDecision::Exact(w) | PulseDecision::Snapped { to: w, .. } => Ok(w),
        PulseDecision::Refused { requested, nearest } => Err(LmcError::PulseWidth { requested, nearest }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 200 mm lens with EzCad's typical 1.1834 field scale.
    const F: f64 = 200.0 * 1.1834;

    #[test]
    fn golden_values() {
        assert_eq!(power_to_mark_current(30.0), Ok(1228));
        assert_eq!(power_to_mark_current(70.0), Ok(2866));
        assert_eq!(power_to_mark_current(50.0), Ok(2048));
        assert_eq!(power_to_mark_current(100.0), Ok(MARK_CURRENT_MAX));
        assert_eq!(frequency_to_period(30.0), Ok(667));
        assert_eq!(frequency_to_period(45.0), Ok(444));
        assert_eq!(frequency_to_period(4000.0), Ok(5));
        assert_eq!(speed_to_units(1000.0, F), Ok(276));
        assert_eq!(speed_to_units(3000.0, F), Ok(830));
        assert_eq!(mm_to_galvo(0.0, F), Ok(CENTER));
        assert_eq!(mm_to_galvo(0.0, 110.0), Ok(0x8000));
        assert_eq!(mm_to_galvo(5.0, F), Ok(34152));
        assert_eq!(mm_to_galvo(-5.0, F), Ok(31384));
        assert_eq!(mm_to_galvo(25.0, 200.0), Ok(40960));
        assert_eq!(distance_word((0x8000, 0x8000), (31384, 31384)), 1957);
        assert_eq!(distance_word((31384, 31384), (34152, 31384)), 2768);
        assert_eq!(distance_word((0, 0), (0xFFFF, 0xFFFF)), 0xFFFF);
    }

    #[test]
    fn field_edges() {
        assert_eq!(mm_to_galvo(-F / 2.0, F), Ok(0));
        assert!(matches!(mm_to_galvo(F / 2.0, F), Err(LmcError::OutOfRange { .. })));
        assert!(mm_to_galvo(F / 2.0 - 0.01, F).is_ok());
        assert!(matches!(mm_to_galvo(-F / 2.0 - 0.01, F), Err(LmcError::OutOfRange { .. })));
        assert!(matches!(mm_to_galvo(f64::NAN, F), Err(LmcError::NonFinite { .. })));
        assert!(matches!(mm_to_galvo(1.0, 0.0), Err(LmcError::OutOfRange { .. })));
        assert!(matches!(mm_to_galvo(1.0, -10.0), Err(LmcError::OutOfRange { .. })));
        assert!((galvo_to_mm(mm_to_galvo(12.5, F).unwrap(), F) - 12.5).abs() < F / 65535.0);
    }

    #[test]
    fn frequency_and_power_edges() {
        assert!(frequency_to_period(0.0).is_err());
        assert!(frequency_to_period(-1.0).is_err());
        assert!(frequency_to_period(0.3).is_err()); // period 66667
        assert!(frequency_to_period(1e6).is_err()); // period 0
        assert!(frequency_to_period(f64::INFINITY).is_err());
        assert_eq!(period_to_frequency(667).map(|k| (k * 100.0).round()), Some(2999.0));
        assert_eq!(period_to_frequency(0), None);
        assert!(power_to_mark_current(-0.001).is_err());
        assert!(power_to_mark_current(100.01).is_err());
        assert!(power_to_mark_current(f64::NAN).is_err());
        assert_eq!(power_to_mark_current(0.0), Ok(0));
        assert!((mark_current_to_power(1228) - 29.988).abs() < 0.001);
    }

    #[test]
    fn speed_edges() {
        assert_eq!(speed_to_units(3.6, F), Ok(0));
        assert_eq!(speed_to_units(3.62, F), Ok(1));
        assert!(speed_to_units(1e7, F).is_err());
        assert!(speed_to_units(-1.0, F).is_err());
        assert!(speed_to_units(f64::NAN, F).is_err());
        assert!((units_to_speed(276, F) - 276.0 * F * 1000.0 / 65535.0).abs() < 1e-12);
    }

    #[test]
    fn delays_keep_galvoplotter_sign_word() {
        assert_eq!(delay_word(0.0), Ok(DelayWord { magnitude: 0, sign: 0x8000 }));
        assert_eq!(delay_word(0.4), Ok(DelayWord { magnitude: 0, sign: 0 }));
        assert_eq!(delay_word(100.0), Ok(DelayWord { magnitude: 100, sign: 0 }));
        assert_eq!(delay_word(-8.9), Ok(DelayWord { magnitude: 8, sign: 0x8000 }));
        assert_eq!(delay_word(65535.0), Ok(DelayWord { magnitude: 65535, sign: 0 }));
        assert!(delay_word(65536.0).is_err());
        assert!(delay_word(f64::INFINITY).is_err());
        assert_eq!(delay_word(-8.9).map(DelayWord::micros), Ok(-8.0));
        assert_eq!(delay_time_units(800.0), Ok(80));
        assert!(delay_time_units(-1.0).is_err());
        assert!(delay_time_units(700_000.0).is_err());
    }

    #[test]
    fn pulse_width_decisions() {
        let t = &JPT_M7_PULSE_WIDTHS_NS;
        assert_eq!(nearest_pulse_width(200.0, t), PulseDecision::Exact(200));
        assert_eq!(nearest_pulse_width(199.8, t), PulseDecision::Snapped { requested: 199.8, to: 200 });
        assert_eq!(nearest_pulse_width(170.0, t), PulseDecision::Refused { requested: 170.0, nearest: 150 });
        assert_eq!(nearest_pulse_width(175.0, t), PulseDecision::Refused { requested: 175.0, nearest: 150 });
        assert!(matches!(nearest_pulse_width(f64::NAN, t), PulseDecision::Refused { nearest: 2, .. }));
        assert_eq!(nearest_pulse_width(5.0, &[]), PulseDecision::Refused { requested: 5.0, nearest: 0 });
        assert_eq!(pulse_width_word(2.0, t), Ok(2));
        assert_eq!(pulse_width_word(171.0, t), Err(LmcError::PulseWidth { requested: 171.0, nearest: 150 }));
    }

    #[test]
    fn distance_word_matches_float_hypot() {
        for (a, b) in [((0u16, 0u16), (3u16, 4u16)), ((1000, 1000), (1003, 1004)), ((0, 0), (30, 1)), ((5, 7), (5, 7))] {
            let dx = f64::from(b.0) - f64::from(a.0);
            let dy = f64::from(b.1) - f64::from(a.1);
            assert_eq!(f64::from(distance_word(a, b)), dx.hypot(dy).trunc());
        }
    }
}
