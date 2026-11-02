//! Unit conversions between mm / mm/s / kHz / % / µs / ns and LMC words.
//!
//! Rounding follows galvoplotter's Python: `round()` is ties-to-even (`f64::round_ties_even`) and `int()` truncates.
//! Every function refuses a value it cannot represent ([`LmcError`]) instead of clamping or wrapping. The one
//! exception is [`distance_word`], a motion hint that galvoplotter itself saturates at `0xFFFF`.

// Ported from openlaser-rs @ 9d81a15 `crates/types/src/galvo.rs` (MIT OR Apache-2.0, used here under MIT), whose
// formulas come from meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (`_convert_speed`, `_convert_frequency`,
// `_convert_power`, `list_jump`, `list_*_delay`; MIT). Changes: one error type, speed and power refuse instead of
// saturating, no `_om` compatibility variants. The delay sign rule (`>= 0` is positive) and the polygon delay
// encoding follow meerk40t `meerk40t/balormk/controller.py` (`list_laser_on_delay`, `list_polygon_delay`; MIT,
// checked against `main` on 2026-10-31), whose own LICENSE notice is the third copyright line below.
//
// MIT License
//
// Copyright (c) 2023 MeerK40t
// Copyright (c) 2026 Luiz Bueno and the OpenLaser contributors
// Copyright (c) 2021 meerk40t
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

use crate::calibration::Calibration;
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

/// How field-frame mm map onto the two galvo axes: the field size plus the profile's axis swap and mirrors.
///
/// This is the one definition of the transform: `plan` (the burn) and `machine` (red-light framing) both go through
/// [`field_to_galvo`] / [`galvo_to_field`], so the frame shows exactly where the job will mark.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FieldTransform {
    /// Width of the galvo field (along field X), mm; also its height unless `field_height_mm` is set.
    pub field_mm: f64,
    /// Height of a non-square field (along field Y), mm (LightBurn's separate field Width and Height); `None` for a
    /// square field. Each field axis maps its own size onto the full galvo word range. Left out of JSON when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_height_mm: Option<f64>,
    /// Exchange the axes: field X drives the galvo Y word and field Y the galvo X word. Applied after negation.
    pub swap_xy: bool,
    /// Mirror field X (in the field frame, before any swap).
    pub negate_x: bool,
    /// Mirror field Y (in the field frame, before any swap).
    pub negate_y: bool,
    /// Lens corrections (field offset and angle, scale, bulge, skew, trapezoid), applied in field mm after the field
    /// bound check and before negate and swap. Neutral by default (and then left out of JSON).
    #[serde(default, skip_serializing_if = "Calibration::is_neutral")]
    pub calibration: Calibration,
}

impl FieldTransform {
    /// A transform without lens corrections.
    pub fn plain(field_mm: f64, swap_xy: bool, negate_x: bool, negate_y: bool) -> Self {
        FieldTransform { field_mm, field_height_mm: None, swap_xy, negate_x, negate_y, calibration: Calibration::default() }
    }

    /// The field height, mm: `field_height_mm`, else the square `field_mm`.
    pub fn height_mm(&self) -> f64 {
        self.field_height_mm.unwrap_or(self.field_mm)
    }

    /// The field size speeds are converted with: the larger side, so a speed is never exceeded along either axis
    /// of a non-square field (the shorter axis then runs a little slower than asked).
    pub fn speed_field_mm(&self) -> f64 {
        self.field_mm.max(self.height_mm())
    }
}

/// Field-frame mm → galvo `(x, y)` words.
///
/// Order: refuse a coordinate that is not finite or not strictly inside `±field_mm / 2`; apply the lens
/// [`Calibration`] (a corrected point must still encode, else it is refused); negate in the **field**
/// frame (`negate_x` flips field x, `negate_y` flips field y); then swap (`swap_xy`); then [`mm_to_galvo`] per
/// axis. The bound is strict on both sides (`-field/2` alone would encode) so the accepted square is the same
/// whatever the negate settings.
pub fn field_to_galvo(x_mm: f64, y_mm: f64, t: &FieldTransform) -> Result<(u16, u16)> {
    let f = field(t.field_mm)?;
    let fh = field(t.height_mm())?;
    for (v, size) in [(x_mm, f), (y_mm, fh)] {
        finite(v, "coordinate")?;
        if v.abs() >= size / 2.0 {
            return Err(LmcError::OutOfRange { what: "coordinate", value: v, allowed: "|coordinate| < field/2" });
        }
    }
    let (cx, cy) = t.calibration.apply(x_mm, y_mm, f);
    let x = if t.negate_x { -cx } else { cx };
    let y = if t.negate_y { -cy } else { cy };
    // Each field axis keeps its own size through the swap: field X always scales by the width.
    let (wx, wy) = (mm_to_galvo(x, f)?, mm_to_galvo(y, fh)?);
    Ok(if t.swap_xy { (wy, wx) } else { (wx, wy) })
}

/// Galvo `(x, y)` words → field-frame mm: the inverse of [`field_to_galvo`] (unswap, then un-negate), up to half a
/// galvo unit.
pub fn galvo_to_field(gx: u16, gy: u16, t: &FieldTransform) -> (f64, f64) {
    let (wx, wy) = if t.swap_xy { (gy, gx) } else { (gx, gy) };
    let (x, y) = (galvo_to_mm(wx, t.field_mm), galvo_to_mm(wy, t.height_mm()));
    let (x, y) = (if t.negate_x { -x } else { x }, if t.negate_y { -y } else { y });
    t.calibration.invert(x, y, t.field_mm)
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

/// Accepted range of a motion time scale: how many times longer a board takes to move than distance / commanded
/// speed (`machine::MachineProfile::motion_time_scale`, `plan::Target::motion_time_scale`; time estimates only).
pub const MOTION_TIME_SCALE_RANGE: std::ops::RangeInclusive<f64> = 0.5..=2.0;

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

/// A delay as the board takes it: magnitude in µs plus a sign word (`0x0000` for >= 0, `0x8000` for < 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DelayWord {
    /// |delay| in whole µs.
    pub magnitude: u16,
    /// [`DelayWord::POSITIVE`] or [`DelayWord::NEGATIVE`].
    pub sign: u16,
}

impl DelayWord {
    /// Sign word of a zero or positive delay.
    pub const POSITIVE: u16 = 0x0000;
    /// Sign word of a negative delay.
    pub const NEGATIVE: u16 = 0x8000;

    /// The delay in µs (the magnitude, negated when the sign word is [`Self::NEGATIVE`]).
    pub fn micros(self) -> f64 {
        let m = f64::from(self.magnitude);
        if self.sign == Self::NEGATIVE { -m } else { m }
    }
}

/// µs → [`DelayWord`]: `(trunc(|d|), if trunc(d) >= 0 { 0x0000 } else { 0x8000 })`.
///
/// The sign follows meerk40t `balormk` (`0x0000 if delay >= 0`), so 0 → (0, 0x0000). galvoplotter @ fc35136 uses
/// `delay > 0` and sends a zero delay as "-0" (0x8000). The sign is taken after truncation, so -0.4 → (0, 0x0000)
/// and no "-0" is ever sent. Refused when `trunc(|d|) > 65535`.
pub fn delay_word(us: f64) -> Result<DelayWord> {
    let v = finite(us, "delay")?;
    let magnitude = whole_word(v.abs().trunc(), "delay", us, "|delay| < 65536 µs")?;
    let sign = if v.trunc() < 0.0 { DelayWord::NEGATIVE } else { DelayWord::POSITIVE };
    Ok(DelayWord { magnitude, sign })
}

/// µs → `listPolygonDelay` word: one unsigned word in 10 µs units, `trunc(us / 10)`, with no sign word.
///
/// **UNVERIFIED on hardware.** This is meerk40t `balormk/controller.py`'s encoding
/// (`int(min(65535, max(0, delay / 10.0)))`, checked against meerk40t `main` on 2026-10-31). galvoplotter @ fc35136
/// disagrees: it writes `abs(delay)` in µs plus a sign word, like the other laser delays. If galvoplotter is right,
/// a 100 µs corner delay sent this way runs as 10 µs (and the old encoding would have run it as 1000 µs). Confirm with
/// an EzCad USB capture.
///
/// A negative delay becomes 0, as meerk40t clamps it (a negative corner wait means none). Unlike meerk40t, a delay
/// above 655359 µs is refused rather than capped, as everywhere in this crate.
pub fn polygon_delay_units(us: f64) -> Result<u16> {
    let v = finite(us, "polygon delay")?;
    whole_word((v.max(0.0) / 10.0).trunc(), "polygon delay", us, "<= 655359 µs")
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
    fn delays_use_meerk40t_sign_word() {
        assert_eq!(delay_word(0.0), Ok(DelayWord { magnitude: 0, sign: 0 }));
        assert_eq!(delay_word(-0.0), Ok(DelayWord { magnitude: 0, sign: 0 }));
        assert_eq!(delay_word(-0.4), Ok(DelayWord { magnitude: 0, sign: 0 }));
        assert_eq!(delay_word(-1.0), Ok(DelayWord { magnitude: 1, sign: 0x8000 }));
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
    fn polygon_delay_is_one_word_of_10us() {
        assert_eq!(polygon_delay_units(100.0), Ok(10));
        assert_eq!(polygon_delay_units(109.9), Ok(10));
        assert_eq!(polygon_delay_units(0.0), Ok(0));
        assert_eq!(polygon_delay_units(-50.0), Ok(0));
        assert_eq!(polygon_delay_units(655_359.0), Ok(65535));
        assert!(polygon_delay_units(655_360.0).is_err());
        assert!(polygon_delay_units(f64::NAN).is_err());
    }

    fn transforms(field_mm: f64) -> impl Iterator<Item = FieldTransform> {
        (0..8u8).map(move |b| FieldTransform::plain(field_mm, b & 1 != 0, b & 2 != 0, b & 4 != 0))
    }

    #[test]
    fn field_transform_negates_in_field_frame_then_swaps() {
        let w = |v: f64| mm_to_galvo(v, 110.0).unwrap();
        let unit = 110.0 / 65535.0;
        for t in transforms(110.0) {
            // Field (10, 5): negate in the field frame first, then swap.
            let x = if t.negate_x { -10.0 } else { 10.0 };
            let y = if t.negate_y { -5.0 } else { 5.0 };
            let expected = if t.swap_xy { (w(y), w(x)) } else { (w(x), w(y)) };
            assert_eq!(field_to_galvo(10.0, 5.0, &t), Ok(expected), "{t:?}");
            let (bx, by) = galvo_to_field(expected.0, expected.1, &t);
            assert!((bx - 10.0).abs() < unit && (by - 5.0).abs() < unit, "{t:?}");
        }
        // The case the review found: swap + negate_x sends field (10, 5) to galvo axes (5, -10).
        let t = FieldTransform::plain(110.0, true, true, false);
        assert_eq!(field_to_galvo(10.0, 5.0, &t), Ok((w(5.0), w(-10.0))));
    }

    #[test]
    fn field_transform_bound_is_strict_for_every_setting() {
        for t in transforms(110.0) {
            assert!(field_to_galvo(54.99, -54.99, &t).is_ok(), "{t:?}");
            for (x, y) in [(55.0, 0.0), (-55.0, 0.0), (0.0, 55.0), (0.0, -55.0), (1e9, 0.0)] {
                assert!(matches!(field_to_galvo(x, y, &t), Err(LmcError::OutOfRange { .. })), "{t:?} ({x}, {y})");
            }
            assert!(matches!(field_to_galvo(f64::NAN, 0.0, &t), Err(LmcError::NonFinite { .. })));
            assert!(matches!(field_to_galvo(0.0, f64::INFINITY, &t), Err(LmcError::NonFinite { .. })));
        }
        let bad = FieldTransform::plain(0.0, false, false, false);
        assert!(field_to_galvo(0.0, 0.0, &bad).is_err());
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

    #[test]
    fn calibration_is_applied_before_negate_and_swap_and_inverted() {
        let calibration = Calibration { offset_x_mm: 1.0, scale_y: 1.01, angle_deg: 0.5, ..Calibration::default() };
        for b in 0..8u8 {
            let plain = FieldTransform::plain(110.0, b & 1 != 0, b & 2 != 0, b & 4 != 0);
            let t = FieldTransform { calibration, ..plain };
            let (cx, cy) = calibration.apply(10.0, 5.0, 110.0);
            assert_eq!(field_to_galvo(10.0, 5.0, &t), field_to_galvo(cx, cy, &plain), "{b:03b}");
            let (gx, gy) = field_to_galvo(10.0, 5.0, &t).unwrap();
            let (x, y) = galvo_to_field(gx, gy, &t);
            assert!((x - 10.0).abs() < 0.01 && (y - 5.0).abs() < 0.01, "{b:03b}");
        }
        // A point the correction pushes past the field edge is refused, never clamped.
        let t = FieldTransform { calibration: Calibration { offset_x_mm: 2.0, ..Calibration::default() }, ..FieldTransform::plain(110.0, false, false, false) };
        assert!(field_to_galvo(54.0, 0.0, &t).is_err());
        let json = serde_json::to_value(FieldTransform::plain(110.0, false, false, false)).unwrap();
        assert!(json.get("calibration").is_none());
    }
}

#[cfg(test)]
mod non_square_tests {
    use super::*;

    #[test]
    fn non_square_field_scales_each_axis_by_its_own_size() {
        let t = FieldTransform { field_height_mm: Some(100.0), ..FieldTransform::plain(200.0, false, false, false) };
        assert_eq!(t.height_mm(), 100.0);
        assert_eq!(t.speed_field_mm(), 200.0);
        // Half way to each edge is a quarter of the word range from the centre on both axes.
        let (gx, gy) = field_to_galvo(50.0, 25.0, &t).unwrap();
        assert_eq!(gx, mm_to_galvo(50.0, 200.0).unwrap());
        assert_eq!(gy, mm_to_galvo(25.0, 100.0).unwrap());
        assert_eq!(gx, gy, "the same fraction of each axis");
        // Y is bounded by the height, X by the width.
        assert!(field_to_galvo(90.0, 0.0, &t).is_ok());
        assert!(field_to_galvo(0.0, 50.0, &t).is_err());
        // Swap keeps each field axis with its own size; the inverse undoes it.
        for bits in 0..8u8 {
            let t = FieldTransform { field_height_mm: Some(100.0), ..FieldTransform::plain(200.0, bits & 1 != 0, bits & 2 != 0, bits & 4 != 0) };
            let w = field_to_galvo(-61.3, 12.7, &t).unwrap();
            let (x, y) = galvo_to_field(w.0, w.1, &t);
            assert!((x + 61.3).abs() < 200.0 / 65535.0 && (y - 12.7).abs() < 100.0 / 65535.0, "{bits}: {x} {y}");
        }
        // A square transform is unchanged and leaves the height out of JSON.
        let sq = FieldTransform::plain(110.0, false, false, false);
        assert_eq!(sq.height_mm(), 110.0);
        assert!(!serde_json::to_string(&sq).unwrap().contains("field_height_mm"));
    }
}
