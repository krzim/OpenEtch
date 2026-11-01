//! Single (immediate) commands and their 8-byte replies.
//!
//! A single command is six u16 words sent little-endian in one 12-byte bulk write. Every one except `WriteCorLine`
//! is answered by an 8-byte reply (four u16 words); word 3 is the board's [`StatusWord`].

// Ported from openlaser-rs @ 9d81a15 `crates/lmc/src/packet.rs` (MIT OR Apache-2.0, used here under MIT); command
// parameter layouts and status bits from meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (MIT).
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

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{LmcError, Result, op};

/// Bytes in a single-command reply.
pub const REPLY_LEN: usize = 8;

/// One single command: opcode then five parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Single(pub [u16; 6]);

impl Single {
    /// Any single command: opcode and its five parameters.
    pub const fn new(opcode: u16, params: [u16; 5]) -> Self {
        let [a, b, c, d, e] = params;
        Single([opcode, a, b, c, d, e])
    }

    const fn op(opcode: u16) -> Self {
        Single::new(opcode, [0; 5])
    }

    const fn op1(opcode: u16, a: u16) -> Self {
        Single::new(opcode, [a, 0, 0, 0, 0])
    }

    /// The opcode word.
    pub const fn opcode(&self) -> u16 {
        self.0[0]
    }

    /// The 12 bytes on the wire (six little-endian words).
    pub fn to_bytes(&self) -> [u8; 12] {
        let mut out = [0u8; 12];
        for (pair, word) in out.as_chunks_mut::<2>().0.iter_mut().zip(self.0) {
            *pair = word.to_le_bytes();
        }
        out
    }

    /// The inverse of [`Single::to_bytes`].
    pub fn from_bytes(bytes: [u8; 12]) -> Self {
        let mut words = [0u16; 6];
        for (word, pair) in words.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *word = u16::from_le_bytes(*pair);
        }
        Single(words)
    }

    /// Whether the board answers with an 8-byte reply that must be read (all but `WriteCorLine`).
    pub fn reads_reply(&self) -> bool {
        self.opcode() != op::WRITE_COR_LINE
    }

    /// galvoplotter's name for the opcode.
    pub fn name(&self) -> &'static str {
        op::single_name(self.opcode())
    }

    /// Firmware version; the reply's word 3 is the status word (galvoplotter polls status this way).
    pub const fn get_version() -> Self {
        Self::op(op::GET_VERSION)
    }
    pub const fn get_serial_no() -> Self {
        Self::op(op::GET_SERIAL_NO)
    }
    pub const fn reset() -> Self {
        Self::op(op::RESET)
    }
    /// Init: enable the laser source interface (does not fire).
    pub const fn enable_laser() -> Self {
        Self::op(op::ENABLE_LASER)
    }
    /// Disable the laser source interface.
    pub const fn disable_laser() -> Self {
        Self::op(op::DISABLE_LASER)
    }
    /// Write the whole output port word (bit 0 laser enable, bit 8 red pointer on the B6).
    pub const fn write_port(bits: u16) -> Self {
        Self::op1(op::WRITE_PORT, bits)
    }
    /// Analog output 1 (init writes `0x07FF`).
    pub const fn write_analog_port_1(value: u16) -> Self {
        Self::op1(op::WRITE_ANALOG_PORT_1, value)
    }
    /// Start executing the queued list packets.
    pub const fn execute_list() -> Self {
        Self::op(op::EXECUTE_LIST)
    }
    /// Abort the running list.
    pub const fn stop_execute() -> Self {
        Self::op(op::STOP_EXECUTE)
    }
    /// Pause the running list: galvoplotter's and meerk40t's `pause()` send exactly this (`StopList`, 0x0020) and
    /// resume with [`Single::restart_list`]. That the B6 holds its place and keeps the MO state is unverified on
    /// hardware.
    pub const fn stop_list() -> Self {
        Self::op(op::STOP_LIST)
    }
    /// Clear the board's list buffer before a new list.
    pub const fn reset_list() -> Self {
        Self::op(op::RESET_LIST)
    }
    /// Resume a list paused with [`Single::stop_list`] (galvoplotter / meerk40t `resume()`: `RestartList`, 0x0013).
    pub const fn restart_list() -> Self {
        Self::op(op::RESTART_LIST)
    }
    /// Sent after each list packet (galvoplotter always sends 0).
    pub const fn set_end_of_list() -> Self {
        Self::op1(op::SET_END_OF_LIST, 0)
    }
    pub const fn get_list_status() -> Self {
        Self::op(op::GET_LIST_STATUS)
    }
    /// Begin (`true`) a lens correction table upload; the rows follow as [`Single::write_cor_line`].
    pub const fn write_cor_table(enable: bool) -> Self {
        Self::op1(op::WRITE_COR_TABLE, enable as u16)
    }
    /// One correction row. `dx`/`dy` are the raw words the `corfile` crate produces; `first` is true only for the
    /// table's first row (galvoplotter's `non_first` = 0). This is the one single command without a reply.
    pub const fn write_cor_line(dx: u16, dy: u16, first: bool) -> Self {
        Single::new(op::WRITE_COR_LINE, [dx, dy, !first as u16, 0, 0])
    }
    /// Move the galvos immediately (laser off). `distance` is the motion hint ([`crate::units::distance_word`]),
    /// galvoplotter's default 0 is accepted.
    pub const fn goto_xy(x: u16, y: u16, distance: u16) -> Self {
        Single::new(op::GOTO_XY, [x, y, 0, distance, 0])
    }
    pub const fn set_control_mode(mode: u16) -> Self {
        Self::op1(op::SET_CONTROL_MODE, mode)
    }
    pub const fn set_laser_mode(mode: u16) -> Self {
        Self::op1(op::SET_LASER_MODE, mode)
    }
    pub const fn set_delay_mode(mode: u16) -> Self {
        Self::op1(op::SET_DELAY_MODE, mode)
    }
    pub const fn set_timing(timing: u16) -> Self {
        Self::op1(op::SET_TIMING, timing)
    }
    pub const fn set_standby(standby_1: u16, standby_2: u16) -> Self {
        Single::new(op::SET_STANDBY, [standby_1, standby_2, 0, 0, 0])
    }
    pub const fn set_first_pulse_killer(fpk: u16) -> Self {
        Self::op1(op::SET_FIRST_PULSE_KILLER, fpk)
    }
    pub const fn set_pwm_half_period(half_period: u16) -> Self {
        Self::op1(op::SET_PWM_HALF_PERIOD, half_period)
    }
    pub const fn set_pwm_pulse_width(pulse_width: u16) -> Self {
        Self::op1(op::SET_PWM_PULSE_WIDTH, pulse_width)
    }
    pub const fn set_fpk_param_2(p1: u16, p2: u16, p3: u16, p4: u16) -> Self {
        Single::new(op::SET_FPK_PARAM_2, [p1, p2, p3, p4, 0])
    }
    pub const fn set_fly_res(r1: u16, r2: u16, r3: u16, r4: u16) -> Self {
        Single::new(op::SET_FLY_RES, [r1, r2, r3, r4, 0])
    }
    /// Open (`true`) or close the fiber master oscillator. Opening it arms emission for the next marks.
    pub const fn fiber_set_mo(open: bool) -> Self {
        Self::op1(op::FIBER_SET_MO, open as u16)
    }
    pub const fn enable_z() -> Self {
        Self::op(op::ENABLE_Z)
    }
    pub const fn read_port() -> Self {
        Self::op(op::READ_PORT)
    }
    pub const fn get_position_xy() -> Self {
        Self::op(op::GET_POSITION_XY)
    }
    /// Move the extension axis (rotary or Z) to `position`; `invert` reverses its direction. meerk40t
    /// `balormk/controller.py` `move_axis_to(position, invert)`; the unit (motor steps) and range are unverified on
    /// hardware. The board reports [`StatusWord::AXIS`] while the axis moves.
    pub const fn move_axis_to(position: u16, invert: bool) -> Self {
        Single::new(op::MOVE_AXIS_TO, [position, invert as u16, 0, 0, 0])
    }
    /// Home the extension axis (meerk40t `axis_go_origin()`, no parameters).
    pub const fn axis_go_origin() -> Self {
        Self::op(op::AXIS_GO_ORIGIN)
    }
    /// Read the extension axis position (meerk40t `get_axis_pos(index=0)`).
    pub const fn get_axis_pos(index: u16) -> Self {
        Self::op1(op::GET_AXIS_POS, index)
    }
    /// Extension axis motion parameters (galvoplotter `set_axis_motion_param(p0..p3)`; meanings unverified).
    pub const fn set_axis_motion_param(p0: u16, p1: u16, p2: u16, p3: u16) -> Self {
        Single::new(op::SET_AXIS_MOTION_PARAM, [p0, p1, p2, p3, 0])
    }
    /// Extension axis homing parameters (galvoplotter `set_axis_origin_param(p0..p3)`; meanings unverified).
    pub const fn set_axis_origin_param(p0: u16, p1: u16, p2: u16, p3: u16) -> Self {
        Single::new(op::SET_AXIS_ORIGIN_PARAM, [p0, p1, p2, p3, 0])
    }
    /// The board's mark counter (galvoplotter `get_mark_count`; reply layout undecoded).
    pub const fn get_mark_count() -> Self {
        Self::op(op::GET_MARK_COUNT)
    }
    /// Mark time of the last list (galvoplotter always sends 3).
    pub const fn get_mark_time() -> Self {
        Self::op1(op::GET_MARK_TIME, 3)
    }
}

/// e.g. `Fiber_SetMo(0)`, `WritePort(1)`, `ResetList`: the name, then the parameters up to the last non-zero one.
impl fmt::Display for Single {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [_, params @ ..] = self.0;
        let used = params.iter().rposition(|p| *p != 0).map_or(0, |i| i + 1);
        f.write_str(self.name())?;
        if used > 0 {
            let shown: Vec<String> = params[..used].iter().map(u16::to_string).collect();
            write!(f, "({})", shown.join(","))?;
        }
        Ok(())
    }
}

/// Parse an 8-byte reply into its four little-endian words.
pub fn parse_reply(bytes: &[u8]) -> Result<[u16; 4]> {
    let array: [u8; REPLY_LEN] = bytes.try_into().map_err(|_| LmcError::ReplyLength(bytes.len()))?;
    let [a0, a1, b0, b1, c0, c1, d0, d1] = array;
    Ok([u16::from_le_bytes([a0, a1]), u16::from_le_bytes([b0, b1]), u16::from_le_bytes([c0, c1]), u16::from_le_bytes([d0, d1])])
}

/// The board status: word 3 of a reply. Unknown bits are kept (one board idles at `0x0220`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StatusWord(pub u16);

impl StatusWord {
    /// A list is executing.
    pub const BUSY: u16 = 0x04;
    /// The board can accept another list packet.
    pub const READY: u16 = 0x20;
    /// The rotary/extension axis is moving.
    pub const AXIS: u16 = 0x40;

    /// Word 3 of a parsed reply.
    pub fn from_reply(reply: [u16; 4]) -> Self {
        StatusWord(reply[3])
    }

    /// Word 3 of an 8-byte reply.
    pub fn from_reply_bytes(bytes: &[u8]) -> Result<Self> {
        parse_reply(bytes).map(Self::from_reply)
    }

    pub fn busy(self) -> bool {
        self.0 & Self::BUSY != 0
    }

    pub fn ready(self) -> bool {
        self.0 & Self::READY != 0
    }

    pub fn axis(self) -> bool {
        self.0 & Self::AXIS != 0
    }

    /// Ready and not busy: the last list has finished (galvoplotter's `wait_finished` condition).
    pub fn idle(self) -> bool {
        self.ready() && !self.busy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_six_le_words() {
        let s = Single::goto_xy(0x1234, 0x8000, 7);
        assert_eq!(s.to_bytes(), [0x0d, 0x00, 0x34, 0x12, 0x00, 0x80, 0, 0, 7, 0, 0, 0]);
        assert_eq!(Single::from_bytes(s.to_bytes()), s);
        assert_eq!(Single::get_version().to_bytes(), [7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn only_write_cor_line_has_no_reply() {
        assert!(!Single::write_cor_line(1, 2, true).reads_reply());
        assert!(Single::write_cor_table(true).reads_reply());
        assert!(Single::get_version().reads_reply());
        assert_eq!(Single::write_cor_line(1, 2, true).0, [op::WRITE_COR_LINE, 1, 2, 0, 0, 0]);
        assert_eq!(Single::write_cor_line(1, 2, false).0, [op::WRITE_COR_LINE, 1, 2, 1, 0, 0]);
    }

    #[test]
    fn constructors_use_galvoplotter_layouts() {
        assert_eq!(Single::fiber_set_mo(true).0, [op::FIBER_SET_MO, 1, 0, 0, 0, 0]);
        assert_eq!(Single::set_standby(2000, 20).0, [op::SET_STANDBY, 2000, 20, 0, 0, 0]);
        assert_eq!(Single::set_fpk_param_2(0x0ffb, 1, 409, 100).0, [op::SET_FPK_PARAM_2, 0x0ffb, 1, 409, 100, 0]);
        assert_eq!(Single::set_fly_res(0, 99, 1000, 25).0, [op::SET_FLY_RES, 0, 99, 1000, 25, 0]);
        assert_eq!(Single::write_analog_port_1(0x07ff).0, [op::WRITE_ANALOG_PORT_1, 0x07ff, 0, 0, 0, 0]);
        assert_eq!(Single::get_mark_time().to_string(), "GetMarkTime(3)");
        assert_eq!(Single::reset_list().to_string(), "ResetList");
    }

    #[test]
    fn status_from_reply() {
        let st = StatusWord::from_reply_bytes(&[1, 0, 2, 0, 3, 0, 0x24, 0x02]).unwrap();
        assert_eq!(st, StatusWord(0x0224));
        assert!(st.busy() && st.ready() && !st.axis() && !st.idle());
        assert!(StatusWord(0x0220).idle());
        assert!(StatusWord(0x40).axis());
        assert_eq!(parse_reply(&[0; 7]), Err(LmcError::ReplyLength(7)));
    }
}
