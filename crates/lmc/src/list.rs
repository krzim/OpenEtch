//! [`MarkingList`]: the unrestricted list builder used for jobs that fire the laser.
//!
//! Inputs are LMC words (galvo units, units/ms, [`DelayWord`]s, mark current, period, ns); convert from mm, %, kHz
//! and µs with [`crate::units`]. Values the board cannot use (a motion speed or Q-switch period of 0, a mark current
//! above 4095) are refused. Motions carry the distance word galvoplotter computes from the previous position.

// Command layouts and the distance word from meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (`list_jump`,
// `list_mark`, `list_*`; MIT); builder shape adapted from openlaser-rs @ 9d81a15 `crates/lmc/src/encode.rs`
// (MIT OR Apache-2.0, used here under MIT).
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

use crate::command::ListCommand;
use crate::packet::{Packet, packetize};
use crate::units::{CENTER, DelayWord, MARK_CURRENT_MAX, distance_word};
use crate::{LmcError, Result};

/// Builds a marking list. Starts with `ReadyMark`, as every galvoplotter and EzCad list does.
///
/// The galvo position used for distance words starts at the field centre ([`MarkingList::new`]) or where the
/// previous list left it ([`MarkingList::starting_at`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkingList {
    commands: Vec<ListCommand>,
    position: (u16, u16),
}

impl Default for MarkingList {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkingList {
    /// A list whose galvo starts at the field centre.
    pub fn new() -> Self {
        Self::starting_at(CENTER, CENTER)
    }

    /// A list whose galvo starts at `(x, y)` (e.g. where the previous list ended).
    pub fn starting_at(x: u16, y: u16) -> Self {
        MarkingList { commands: vec![ListCommand::ReadyMark], position: (x, y) }
    }

    /// Append any list command. A `JumpTo`/`MarkTo` moves the tracked position; an `Other` whose words fit a typed
    /// variant is stored as that variant.
    pub fn push(&mut self, command: ListCommand) -> &mut Self {
        let command = ListCommand::from_words(command.to_words());
        if let ListCommand::JumpTo { x, y, .. } | ListCommand::MarkTo { x, y, .. } = command {
            self.position = (x, y);
        }
        self.commands.push(command);
        self
    }

    /// Jump speed in galvo units per ms ([`crate::units::speed_to_units`]); 0 is refused.
    pub fn jump_speed(&mut self, units: u16) -> Result<&mut Self> {
        nonzero(units, "jump speed")?;
        Ok(self.push(ListCommand::JumpSpeed { units }))
    }

    /// Mark speed in galvo units per ms; 0 is refused.
    pub fn mark_speed(&mut self, units: u16) -> Result<&mut Self> {
        nonzero(units, "mark speed")?;
        Ok(self.push(ListCommand::MarkSpeed { units }))
    }

    /// Laser-on delay ([`crate::units::delay_word`]).
    pub fn laser_on_delay(&mut self, delay: DelayWord) -> &mut Self {
        self.push(ListCommand::LaserOnDelay(delay))
    }

    /// Laser-off delay.
    pub fn laser_off_delay(&mut self, delay: DelayWord) -> &mut Self {
        self.push(ListCommand::LaserOffDelay(delay))
    }

    /// Polygon (corner) delay in 10 µs units ([`crate::units::polygon_delay_units`]; the encoding is unverified on
    /// hardware).
    pub fn polygon_delay(&mut self, time_10us: u16) -> &mut Self {
        self.push(ListCommand::PolygonDelay { time_10us })
    }

    /// Settling delay after each jump.
    pub fn jump_delay(&mut self, delay: DelayWord) -> &mut Self {
        self.push(ListCommand::JumpDelay(delay))
    }

    /// Fiber mark current ([`crate::units::power_to_mark_current`]); above 4095 (100 %) is refused.
    pub fn mark_power(&mut self, current: u16) -> Result<&mut Self> {
        if current > MARK_CURRENT_MAX {
            return Err(LmcError::OutOfRange { what: "mark current", value: f64::from(current), allowed: "0..=4095" });
        }
        Ok(self.push(ListCommand::MarkCurrent { current }))
    }

    /// Q-switch period ([`crate::units::frequency_to_period`]); 0 is refused.
    pub fn q_switch_period(&mut self, period: u16) -> Result<&mut Self> {
        nonzero(period, "q-switch period")?;
        Ok(self.push(ListCommand::QSwitchPeriod { period }))
    }

    /// MOPA pulse width in ns (list op `0x8026`). Validate it against the source's table first
    /// ([`crate::units::pulse_width_word`]); 0 is refused.
    pub fn fiber_pulse_width(&mut self, ns: u16) -> Result<&mut Self> {
        nonzero(ns, "pulse width")?;
        Ok(self.push(ListCommand::FiberPulseWidth { ns }))
    }

    /// Open the fiber master oscillator: the following marks emit.
    pub fn fiber_open_mo(&mut self) -> &mut Self {
        self.push(ListCommand::FiberOpenMo { open: 1 })
    }

    /// Close the fiber master oscillator.
    pub fn fiber_close_mo(&mut self) -> &mut Self {
        self.push(ListCommand::FiberOpenMo { open: 0 })
    }

    /// Write the whole output port word (bit 0 laser enable, bit 8 red pointer).
    pub fn write_port(&mut self, bits: u16) -> &mut Self {
        self.push(ListCommand::WritePort { bits })
    }

    /// Wait `time_10us` × 10 µs ([`crate::units::delay_time_units`]).
    pub fn delay_time(&mut self, time_10us: u16) -> &mut Self {
        self.push(ListCommand::DelayTime { time_10us })
    }

    /// Fire in place for `time_10us` × 10 µs.
    pub fn laser_on_point(&mut self, time_10us: u16) -> &mut Self {
        self.push(ListCommand::LaserOnPoint { time_10us })
    }

    /// Move with the laser off to `(x, y)` galvo units.
    pub fn jump_to(&mut self, x: u16, y: u16) -> &mut Self {
        let distance = distance_word(self.position, (x, y));
        self.push(ListCommand::JumpTo { x, y, distance })
    }

    /// Move with the laser on to `(x, y)` galvo units.
    pub fn mark_to(&mut self, x: u16, y: u16) -> &mut Self {
        let distance = distance_word(self.position, (x, y));
        self.push(ListCommand::MarkTo { x, y, distance })
    }

    /// An explicit `listEndOfList`. The board stops at the first one; [`MarkingList::finish`] adds it if missing.
    pub fn end_of_list(&mut self) -> &mut Self {
        self.push(ListCommand::EndOfList)
    }

    /// Where the galvo is after the commands so far.
    pub fn position(&self) -> (u16, u16) {
        self.position
    }

    /// The commands so far (starting with `ReadyMark`).
    pub fn commands(&self) -> &[ListCommand] {
        &self.commands
    }

    /// The packets to send: commands in order, ending with `listEndOfList`, 256 per packet, the last one padded.
    pub fn finish(mut self) -> Vec<Packet> {
        if self.commands.last() != Some(&ListCommand::EndOfList) {
            self.commands.push(ListCommand::EndOfList);
        }
        let words: Vec<[u16; 6]> = self.commands.iter().map(ListCommand::to_words).collect();
        packetize(&words)
    }
}

fn nonzero(v: u16, what: &'static str) -> Result<()> {
    if v == 0 { Err(LmcError::Zero { what }) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::{delay_word, frequency_to_period, polygon_delay_units, power_to_mark_current};
    use crate::{SLOTS, decode, op};

    fn sample() -> MarkingList {
        let mut l = MarkingList::new();
        l.write_port(0x0001);
        l.jump_speed(2000).unwrap();
        l.fiber_pulse_width(200).unwrap();
        l.mark_power(power_to_mark_current(30.0).unwrap()).unwrap();
        l.q_switch_period(frequency_to_period(30.0).unwrap()).unwrap();
        l.mark_speed(276).unwrap();
        l.laser_on_delay(delay_word(100.0).unwrap());
        l.laser_off_delay(delay_word(120.0).unwrap());
        l.polygon_delay(polygon_delay_units(50.0).unwrap());
        l.jump_delay(delay_word(400.0).unwrap());
        l.fiber_open_mo().delay_time(800);
        l.jump_to(31384, 31384).mark_to(34152, 31384).mark_to(34152, 34152);
        l.fiber_close_mo();
        l
    }

    #[test]
    fn galvoplotter_words() {
        let l = sample();
        let words: Vec<[u16; 6]> = l.commands().iter().map(ListCommand::to_words).collect();
        assert_eq!(words[0], [op::LIST_READY_MARK, 0, 0, 0, 0, 0]);
        assert!(words.contains(&[op::LIST_MARK_CURRENT, 1228, 0, 0, 0, 0]));
        assert!(words.contains(&[op::LIST_Q_SWITCH_PERIOD, 667, 0, 0, 0, 0]));
        assert!(words.contains(&[op::LIST_FIBER_YLPM_PULSE_WIDTH, 200, 0, 0, 0, 0]));
        assert!(words.contains(&[op::LIST_JUMP_TO, 31384, 31384, 0, 1957, 0]));
        assert!(words.contains(&[op::LIST_MARK_TO, 34152, 31384, 0, 2768, 0]));
        assert_eq!(l.position(), (34152, 34152));
    }

    #[test]
    fn refuses_unusable_words() {
        let mut l = MarkingList::new();
        assert_eq!(l.mark_speed(0).err(), Some(LmcError::Zero { what: "mark speed" }));
        assert!(l.jump_speed(0).is_err());
        assert!(l.q_switch_period(0).is_err());
        assert!(l.fiber_pulse_width(0).is_err());
        assert!(l.mark_power(4096).is_err());
        assert!(l.mark_power(4095).is_ok());
    }

    #[test]
    fn finish_round_trips_through_decode() {
        let l = sample();
        let mut expected = l.commands().to_vec();
        expected.push(ListCommand::EndOfList);
        let packets = l.finish();
        assert_eq!(packets.len(), 1);
        assert_eq!(decode(&packets), expected);
    }

    #[test]
    fn long_list_spans_packets_and_round_trips() {
        let mut l = MarkingList::new();
        for i in 0..600u16 {
            l.mark_to(i * 50, 65535 - i * 50);
        }
        let mut expected = l.commands().to_vec();
        expected.push(ListCommand::EndOfList);
        let packets = l.finish();
        // 1 ReadyMark + 600 marks + EndOfList = 602 commands.
        assert_eq!(packets.len(), 3);
        assert!(packets.iter().all(|p| p.as_bytes().len() == 3072));
        assert_eq!(packets[0].real_commands(), SLOTS);
        assert_eq!(packets[2].real_commands(), 602 - 2 * SLOTS - 1);
        assert_eq!(decode(&packets), expected);
    }

    #[test]
    fn explicit_end_of_list_is_not_doubled() {
        let mut l = MarkingList::new();
        l.end_of_list();
        let packets = l.finish();
        assert_eq!(decode(&packets), vec![ListCommand::ReadyMark, ListCommand::EndOfList]);
        assert_eq!(packets[0].real_commands(), 1);
    }

    #[test]
    fn push_normalises_other() {
        let mut l = MarkingList::new();
        l.push(ListCommand::Other { words: [op::LIST_JUMP_TO, 5, 6, 0, 0, 0] });
        assert_eq!(l.commands()[1], ListCommand::JumpTo { x: 5, y: 6, distance: 0 });
        assert_eq!(l.position(), (5, 6));
    }
}
