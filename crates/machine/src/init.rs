//! [`InitSettings`]: the board-level values sent once at connect, and the init command sequence.

// Sequence and default values from meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (`__init__` defaults and
// `init_laser`), MIT. Written from scratch in Rust.
//
// MIT License
//
// Copyright (c) 2023 MeerK40t
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

use corfile::{COR_TABLE_BLANK, COR_TABLE_LOAD, CorTable};
use lmc::Single;
use serde::{Deserialize, Serialize};

/// Board settings sent by [`crate::Controller::init`]. Defaults are galvoplotter @ fc35136's; none of them is
/// B6-specific or verified on the B6 yet. Where the init sequence around them differs from galvoplotter's is listed
/// under "Deviations from the reference sequences" on [`crate::Controller`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitSettings {
    /// `SetControlMode` (galvoplotter `control_mode = 0`).
    pub control_mode: u16,
    /// `SetLaserMode` (`laser_mode = 1`).
    pub laser_mode: u16,
    /// `SetDelayMode` (`delay_mode = 1`).
    pub delay_mode: u16,
    /// `SetTiming` (`timing_mode = 1`).
    pub timing_mode: u16,
    /// `SetStandby` parameters (`standby_p1 = 2000`, `standby_p2 = 20`).
    pub standby: (u16, u16),
    /// `SetFirstPulseKiller` (`first_pulse_killer = 200`).
    pub first_pulse_killer: u16,
    /// `SetPwmHalfPeriod` (`pwm_half_period = 125`).
    pub pwm_half_period: u16,
    /// `SetPwmPulseWidth` (`pwm_pulse_width = 125`).
    pub pwm_pulse_width: u16,
    /// `SetFpkParam2`: max voltage, min voltage, t1, t2 (`0xFFB, 1, 409, 100`).
    pub fpk_param_2: (u16, u16, u16, u16),
    /// `SetFlyRes` (`0, 99, 1000, 25`).
    pub fly_res: (u16, u16, u16, u16),
    /// `WriteAnalogPort1` (`0x7FF`).
    pub analog_port_1: u16,
}

impl Default for InitSettings {
    fn default() -> Self {
        InitSettings {
            control_mode: 0,
            laser_mode: 1,
            delay_mode: 1,
            timing_mode: 1,
            standby: (2000, 20),
            first_pulse_killer: 200,
            pwm_half_period: 125,
            pwm_pulse_width: 125,
            fpk_param_2: (0xFFB, 1, 409, 100),
            fly_res: (0, 99, 1000, 25),
            analog_port_1: 0x7FF,
        }
    }
}

impl InitSettings {
    /// The commands after `GetSerialNo`, `GetVersion`, `Reset` and the correction upload, in galvoplotter's
    /// `init_laser` order (fiber source, so `Fiber_SetMo(0)` is included). `EnableLaser` enables the source
    /// interface; it does not fire, and the MO is closed right after.
    pub fn configuration(&self) -> Vec<Single> {
        let (fpk1, fpk2, fpk3, fpk4) = self.fpk_param_2;
        let (r1, r2, r3, r4) = self.fly_res;
        vec![
            Single::enable_laser(),
            Single::set_control_mode(self.control_mode),
            Single::set_laser_mode(self.laser_mode),
            Single::set_delay_mode(self.delay_mode),
            Single::set_timing(self.timing_mode),
            Single::set_standby(self.standby.0, self.standby.1),
            Single::set_first_pulse_killer(self.first_pulse_killer),
            Single::set_pwm_half_period(self.pwm_half_period),
            Single::set_pwm_pulse_width(self.pwm_pulse_width),
            Single::fiber_set_mo(false),
            Single::set_fpk_param_2(fpk1, fpk2, fpk3, fpk4),
            Single::set_fly_res(r1, r2, r3, r4),
            Single::enable_z(),
            Single::write_analog_port_1(self.analog_port_1),
            Single::enable_z(),
        ]
    }
}

/// The correction upload: `WriteCorTable(1)` then one `WriteCorLine` per node, or `WriteCorTable(0)` alone when
/// there is no table (galvoplotter's blank table).
pub(crate) fn correction_upload(cor: Option<&CorTable>) -> Vec<Single> {
    let Some(table) = cor else {
        return vec![Single::write_cor_table(COR_TABLE_BLANK != 0)];
    };
    let mut out = Vec::with_capacity(corfile::NODE_COUNT + 1);
    out.push(Single::write_cor_table(COR_TABLE_LOAD != 0));
    out.extend(table.upload_lines().iter().map(|l| Single::write_cor_line(l.dx, l.dy, l.first)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_uses_galvoplotter_defaults() {
        let names: Vec<String> = InitSettings::default().configuration().iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            [
                "EnableLaser",
                "SetControlMode",
                "SetLaserMode(1)",
                "SetDelayMode(1)",
                "SetTiming(1)",
                "SetStandby(2000,20)",
                "SetFirstPulseKiller(200)",
                "SetPwmHalfPeriod(125)",
                "SetPwmPulseWidth(125)",
                "Fiber_SetMo",
                "SetFpkParam2(4091,1,409,100)",
                "SetFlyRes(0,99,1000,25)",
                "EnableZ",
                "WriteAnalogPort1(2047)",
                "EnableZ",
            ]
        );
    }

    #[test]
    fn blank_and_identity_upload() {
        assert_eq!(correction_upload(None), [Single::write_cor_table(false)]);
        let up = correction_upload(Some(&CorTable::identity(None)));
        assert_eq!(up.len(), 1 + corfile::NODE_COUNT);
        assert_eq!(up[0], Single::write_cor_table(true));
        assert_eq!(up[1], Single::write_cor_line(0, 0, true));
        assert_eq!(up[2], Single::write_cor_line(0, 0, false));
    }
}
