//! [`LightList`]: the red-pointer framing list builder, which cannot express emission.
//!
//! # Invariant
//!
//! Every list a `LightList` produces contains only `ReadyMark`, `JumpSpeed`, `JumpDelay`, `JumpTo`, `DelayTime`,
//! `WritePort(0x0000)`, `WritePort(RED_POINTER_BIT)` and `EndOfList`. It cannot hold `MarkTo`, `LaserOnPoint`,
//! `FiberOpenMO`, `MarkCurrent` or any port write with the laser-enable bit (bit 0) set, so [`crate::emits_light_only`]
//! is true for every output. This is enforced by the type, not by convention:
//!
//! - the command vector is private and only the methods below push to it, each with a fixed opcode;
//! - the only port writes are the constants `0x0000` and [`RED_POINTER_BIT`] (`0x0100`), with no bits argument;
//! - there is no conversion from [`crate::MarkingList`] or from arbitrary [`ListCommand`]s.
//!
//! A port write replaces the whole port word, so turning the pointer on or off also clears the laser-enable bit.

// Lighting sequence facts (ReadyMark, then the port with the light pin, bit 8 by default, and jumps) from
// meerk40t/galvoplotter @ fc35136 `galvo/controller.py` `lighting_configuration` (MIT); design follows
// openlaser-rs @ 9d81a15 `crates/lmc/src/encode.rs` `LightingList` (MIT OR Apache-2.0, used here under MIT).
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
use crate::units::{CENTER, DelayWord, distance_word};
use crate::{LmcError, Result};

/// Port bit 8: the red aiming pointer on the B6 (galvoplotter's default `light_pin`).
pub const RED_POINTER_BIT: u16 = 1 << 8;

// The invariant depends on these two being different bits.
const _: () = assert!(RED_POINTER_BIT & crate::LASER_ENABLE_BIT == 0);

/// Builds a red-pointer framing list. See the module docs for the no-emission invariant.
///
/// Starts with `ReadyMark`; the red pointer is off until [`LightList::red_pointer_on`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightList {
    commands: Vec<ListCommand>,
    position: (u16, u16),
}

impl Default for LightList {
    fn default() -> Self {
        Self::new()
    }
}

impl LightList {
    /// A list whose galvo starts at the field centre.
    pub fn new() -> Self {
        Self::starting_at(CENTER, CENTER)
    }

    /// A list whose galvo starts at `(x, y)` (only used for distance words).
    pub fn starting_at(x: u16, y: u16) -> Self {
        LightList { commands: vec![ListCommand::ReadyMark], position: (x, y) }
    }

    /// Jump speed in galvo units per ms; 0 is refused.
    pub fn jump_speed(&mut self, units: u16) -> Result<&mut Self> {
        if units == 0 {
            return Err(LmcError::Zero { what: "jump speed" });
        }
        self.commands.push(ListCommand::JumpSpeed { units });
        Ok(self)
    }

    /// Settling time after each jump. Not emitting; kept so framed corners stay sharp.
    pub fn jump_delay(&mut self, delay: DelayWord) -> &mut Self {
        self.commands.push(ListCommand::JumpDelay(delay));
        self
    }

    /// Move to `(x, y)` galvo units (laser off; the dot is visible while the pointer is on).
    pub fn jump_to(&mut self, x: u16, y: u16) -> &mut Self {
        let distance = distance_word(self.position, (x, y));
        self.commands.push(ListCommand::JumpTo { x, y, distance });
        self.position = (x, y);
        self
    }

    /// Wait `time_10us` × 10 µs.
    pub fn delay_time(&mut self, time_10us: u16) -> &mut Self {
        self.commands.push(ListCommand::DelayTime { time_10us });
        self
    }

    /// Port word = [`RED_POINTER_BIT`] only (laser-enable bit cleared).
    pub fn red_pointer_on(&mut self) -> &mut Self {
        self.commands.push(ListCommand::WritePort { bits: RED_POINTER_BIT });
        self
    }

    /// Port word = 0 (pointer and laser-enable both off).
    pub fn red_pointer_off(&mut self) -> &mut Self {
        self.commands.push(ListCommand::WritePort { bits: 0 });
        self
    }

    /// Where the galvo is after the commands so far.
    pub fn position(&self) -> (u16, u16) {
        self.position
    }

    /// Read-only view of the commands so far.
    pub fn commands(&self) -> &[ListCommand] {
        &self.commands
    }

    /// The packets to send: commands in order, then `listEndOfList`, 256 per packet, the last one padded.
    pub fn finish(mut self) -> Vec<Packet> {
        self.commands.push(ListCommand::EndOfList);
        let words: Vec<[u16; 6]> = self.commands.iter().map(ListCommand::to_words).collect();
        packetize(&words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::delay_word;
    use crate::{decode, emits_light_only};

    fn frame() -> LightList {
        let mut l = LightList::new();
        l.jump_speed(4000).unwrap();
        l.jump_delay(delay_word(100.0).unwrap());
        l.red_pointer_on();
        for _ in 0..200 {
            l.jump_to(30000, 30000).jump_to(35000, 30000).jump_to(35000, 35000).jump_to(30000, 35000);
        }
        l.delay_time(10).red_pointer_off();
        l
    }

    #[test]
    fn light_list_is_light_only_and_round_trips() {
        let l = frame();
        let mut expected = l.commands().to_vec();
        expected.push(ListCommand::EndOfList);
        let packets = l.finish();
        assert!(packets.len() > 1);
        assert!(emits_light_only(&packets));
        assert_eq!(decode(&packets), expected);
    }

    #[test]
    fn only_allowed_commands_appear() {
        let packets = frame().finish();
        for c in decode(&packets) {
            let ok = matches!(
                c,
                ListCommand::ReadyMark
                    | ListCommand::JumpSpeed { .. }
                    | ListCommand::JumpDelay(_)
                    | ListCommand::JumpTo { .. }
                    | ListCommand::DelayTime { .. }
                    | ListCommand::WritePort { bits: 0 | RED_POINTER_BIT }
                    | ListCommand::EndOfList
            );
            assert!(ok, "{c}");
        }
    }

    #[test]
    fn zero_jump_speed_refused() {
        assert!(LightList::new().jump_speed(0).is_err());
    }
}
