//! [`JobPlan`] -> LMC list packets.
//!
//! The command order follows meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (`marking_configuration`,
//! `set`, `mark`, `goto`, `initial_configuration`) as also used by openlaser-rs @ 9d81a15
//! `crates/lmc/src/encode.rs` (`encode_section`). Only the order of protocol commands is taken from them; the code
//! is new and built on this workspace's `lmc::MarkingList`. Both projects are MIT licensed:
//!
//! MIT License
//!
//! Copyright (c) 2023 MeerK40t
//! Copyright (c) 2026 Luiz Bueno and the OpenLaser contributors
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.
//!
//! # List layout
//!
//! galvoplotter enters marking with the singles `WritePort(laser bit)` and `Fiber_SetMo(1)`, then queues
//! `ReadyMark`, `DelayTime(open-MO delay)`, `WritePort`, then `set()`. It leaves with `Fiber_SetMo(0)` and
//! `WritePort(0)` once the board is idle. Neither galvoplotter nor meerk40t uses the in-list `FiberOpenMO` (0x8021)
//! when marking, and its behaviour on the B6 is unverified, so this list never contains it: the master oscillator
//! is opened and closed only by `machine::Controller::run`'s single commands. The full order on the wire is:
//!
//! - `machine` prelude (singles): `ResetList`, `WritePort(laser enable)`, `Fiber_SetMo(1)`;
//! - this list:
//!
//! 1. `ReadyMark`
//! 2. `DelayTime(open_mo_delay_us)`, `WritePort(LASER_ENABLE_BIT)`: galvoplotter's order (the MO settle wait, then
//!    the port again, in-list).
//! 3. Per pass, in galvoplotter's `set()` order: `JumpSpeed`, `FiberPulseWidth` (0x8026, only when the pass sets
//!    one), `MarkCurrent`, `QSwitchPeriod`, `MarkSpeed`, `LaserOnDelay`, `LaserOffDelay`, `PolygonDelay` (one
//!    10 µs word, meerk40t's encoding, unverified on hardware: see `lmc::units::polygon_delay_units`).
//! 4. Per polyline: `JumpDelay` (only when it changes; LightBurn's rule, min..max by distance up to the limit),
//!    `JumpTo(start)` (skipped when already there), `MarkCurrent` when the polyline's power differs from the one in
//!    force (grayscale image runs, [`crate::PassPlan::powers`]), one `MarkTo` per further point (skipped when it
//!    rounds to the galvo's current word, as galvoplotter does), then `DelayTime(end_delay_us)` when that is >= 10 µs.
//!    The mid-list `MarkCurrent` is the same list command the pass header sends, which the list executes in order
//!    (as meerk40t's raster output changes power between moves); that it takes effect at that point of the list on
//!    the B6 is unverified on hardware.
//! 5. `WritePort(0)`, `EndOfList`.
//!
//! - `machine` postlude (singles), once the board is idle: `Fiber_SetMo(0)`, `WritePort(0)`, `GetMarkTime(3)`.

use doc::GalvoTiming;
use lmc::units::{delay_time_units, delay_word, frequency_to_period, polygon_delay_units, power_to_mark_current, pulse_width_word, speed_to_units};
use lmc::{DelayWord, LASER_ENABLE_BIT, LmcError, MarkingList, Packet};

use crate::job::{JobPlan, PassSettings};
use crate::{PlanError, Result, Target};

/// One pass's settings as board words.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PassWords {
    jump_speed: u16,
    mark_speed: u16,
    mark_current: u16,
    q_period: u16,
    pulse_width: Option<u16>,
    laser_on: DelayWord,
    laser_off: DelayWord,
    polygon: u16,
    end_delay: u16,
}

impl PassWords {
    /// Convert, refusing anything that cannot be sent as asked (the `lmc` conversions never clamp).
    pub(crate) fn new(s: &PassSettings, t: &Target) -> std::result::Result<Self, LmcError> {
        let tm = &s.timing;
        let nonzero = |w: u16, what: &'static str| if w == 0 { Err(LmcError::Zero { what }) } else { Ok(w) };
        // Every jump delay this timing can produce converts if both ends do.
        delay_word(tm.min_jump_delay_us)?;
        delay_word(tm.max_jump_delay_us)?;
        Ok(PassWords {
            jump_speed: nonzero(speed_to_units(tm.jump_speed_mm_s, t.field_mm)?, "jump speed")?,
            mark_speed: nonzero(speed_to_units(s.speed_mm_s, t.field_mm)?, "mark speed")?,
            mark_current: power_to_mark_current(s.power_pct)?,
            q_period: frequency_to_period(s.frequency_khz)?,
            pulse_width: s.pulse_width_ns.map(|ns| pulse_width_word(f64::from(ns), &t.pulse_widths_ns)).transpose()?,
            laser_on: delay_word(tm.laser_on_delay_us)?,
            laser_off: delay_word(tm.laser_off_delay_us)?,
            polygon: polygon_delay_units(tm.polygon_delay_us)?,
            end_delay: delay_time_units(tm.end_delay_us.max(0.0))?,
        })
    }
}

/// Jump delay in µs for a jump of `distance_mm`: LightBurn's rule, `min + (max - min) × d / limit`, `max` from the
/// limit on (and always `max` when the limit is not positive).
pub fn jump_delay_us(t: &GalvoTiming, distance_mm: f64) -> f64 {
    if t.jump_distance_limit_mm <= 0.0 || distance_mm >= t.jump_distance_limit_mm {
        t.max_jump_delay_us
    } else {
        t.min_jump_delay_us + (t.max_jump_delay_us - t.min_jump_delay_us) * distance_mm / t.jump_distance_limit_mm
    }
}

/// Encode `plan` for `target` as one marking list (see the module docs for the layout).
///
/// Refused: a plan with nothing to mark, a point the target cannot encode, and settings that do not convert.
/// The plan is not trusted to be in-limits: power above `target.max_power_pct` and pulse widths not in the table
/// are checked again here.
pub fn encode(plan: &JobPlan, target: &Target) -> Result<Vec<Packet>> {
    target.validate()?;
    let open_mo = delay_time_units(target.open_mo_delay_us)?;
    let mut list = MarkingList::new();
    list.delay_time(open_mo).write_port(LASER_ENABLE_BIT);

    let mut marks = 0usize;
    let mut pos_mm = [0.0, 0.0];
    let mut jump_delay: Option<DelayWord> = None;
    for pass in &plan.passes {
        let s = &pass.settings;
        if s.power_pct > target.max_power_pct {
            return Err(PlanError::PowerTooHigh { layer: pass.layer_id.clone(), power_pct: s.power_pct, max_pct: target.max_power_pct });
        }
        let w = PassWords::new(s, target).map_err(|e| PlanError::Encode { layer: pass.layer_id.clone(), source: e })?;
        list.jump_speed(w.jump_speed)?;
        if let Some(ns) = w.pulse_width {
            list.fiber_pulse_width(ns)?;
        }
        list.mark_power(w.mark_current)?;
        list.q_switch_period(w.q_period)?;
        list.mark_speed(w.mark_speed)?;
        list.laser_on_delay(w.laser_on).laser_off_delay(w.laser_off).polygon_delay(w.polygon);

        let mut current = w.mark_current;
        for (i, poly) in pass.polylines.iter().enumerate() {
            let Some((&first, rest)) = poly.split_first() else { continue };
            // A polyline's own power (grayscale image runs) is checked against the limit like the pass's.
            let power = pass.power_of(i);
            let mc = if power == s.power_pct {
                w.mark_current
            } else if power.is_finite() && power >= 0.0 && power <= target.max_power_pct {
                power_to_mark_current(power).map_err(|e| PlanError::Encode { layer: pass.layer_id.clone(), source: e })?
            } else {
                return Err(PlanError::PowerTooHigh { layer: pass.layer_id.clone(), power_pct: power, max_pct: target.max_power_pct });
            };
            let start = to_galvo(target, first, &pass.layer_id)?;
            if start != list.position() {
                let d = jump_delay_word(&s.timing, crate::geom::dist(pos_mm, first)).map_err(|e| PlanError::Encode { layer: pass.layer_id.clone(), source: e })?;
                if jump_delay != Some(d) {
                    list.jump_delay(d);
                    jump_delay = Some(d);
                }
                list.jump_to(start.0, start.1);
            }
            if mc != current {
                list.mark_power(mc)?;
                current = mc;
            }
            pos_mm = first;
            for &p in rest {
                let to = to_galvo(target, p, &pass.layer_id)?;
                pos_mm = p;
                if to == list.position() {
                    continue;
                }
                list.mark_to(to.0, to.1);
                marks += 1;
            }
            if w.end_delay > 0 {
                list.delay_time(w.end_delay);
            }
        }
    }
    if marks == 0 {
        return Err(PlanError::Empty);
    }
    list.write_port(0).end_of_list();
    Ok(list.finish())
}

fn jump_delay_word(t: &GalvoTiming, distance_mm: f64) -> std::result::Result<DelayWord, LmcError> {
    delay_word(jump_delay_us(t, distance_mm))
}

fn to_galvo(target: &Target, p: [f64; 2], layer: &str) -> Result<(u16, u16)> {
    target.to_galvo(p).map_err(|_| PlanError::PointOutOfField { layer: layer.into(), point: p, field_mm: target.field_mm })
}
