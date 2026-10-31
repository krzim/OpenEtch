//! Typed list commands, with a lossless mapping to and from their six wire words.
//!
//! Parameter layouts follow galvoplotter's `list_*` writers: a motion is `[op, x, y, angle, distance, 0]` with angle
//! always 0; a delay is `[op, magnitude µs, sign word, 0, 0, 0]`; everything else takes one value in the first
//! parameter. A slot whose words do not fit its variant exactly (an unknown opcode, or a non-zero word where the
//! layout has none) decodes to [`ListCommand::Other`], so `from_words(w).to_words() == w` for every `w`.

// Parameter layouts from meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (MIT); variant set and decode rules
// adapted from openlaser-rs @ 9d81a15 `crates/lmc/src/decode.rs` (MIT OR Apache-2.0, used here under MIT).
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

use crate::op;
use crate::units::DelayWord;

/// One list command. Positions are galvo words, speeds galvo units per ms, delays µs, times 10 µs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ListCommand {
    /// `0x8051`: starts a list.
    ReadyMark,
    /// `0x8001`: move with the laser off. `distance` is the hint word ([`crate::units::distance_word`]).
    JumpTo { x: u16, y: u16, distance: u16 },
    /// `0x8005`: move with the laser on (emits when the MO is open and port bit 0 is set).
    MarkTo { x: u16, y: u16, distance: u16 },
    /// `0x8003`: fire in place for `time_10us` × 10 µs.
    LaserOnPoint { time_10us: u16 },
    /// `0x8004`: wait `time_10us` × 10 µs.
    DelayTime { time_10us: u16 },
    /// `0x8006`.
    JumpSpeed { units: u16 },
    /// `0x800c`.
    MarkSpeed { units: u16 },
    /// `0x8007`.
    LaserOnDelay(DelayWord),
    /// `0x8008`.
    LaserOffDelay(DelayWord),
    /// `0x800d`.
    JumpDelay(DelayWord),
    /// `0x800f`.
    PolygonDelay(DelayWord),
    /// `0x8011`: the whole output port word.
    WritePort { bits: u16 },
    /// `0x8012`: fiber power, 0..=4095.
    MarkCurrent { current: u16 },
    /// `0x801b`: 20000 / kHz.
    QSwitchPeriod { period: u16 },
    /// `0x8021`: 1 opens the master oscillator, 0 closes it.
    FiberOpenMo { open: u16 },
    /// `0x8026`: MOPA pulse width in ns.
    FiberPulseWidth { ns: u16 },
    /// `0x8002`: end of list (and the padding of unused slots).
    EndOfList,
    /// Any other slot, kept word for word.
    Other { words: [u16; 6] },
}

impl ListCommand {
    /// The six wire words.
    pub fn to_words(&self) -> [u16; 6] {
        use ListCommand::*;
        let one = |opcode: u16, v: u16| [opcode, v, 0, 0, 0, 0];
        let delay = |opcode: u16, d: DelayWord| [opcode, d.magnitude, d.sign, 0, 0, 0];
        match *self {
            ReadyMark => one(op::LIST_READY_MARK, 0),
            JumpTo { x, y, distance } => [op::LIST_JUMP_TO, x, y, 0, distance, 0],
            MarkTo { x, y, distance } => [op::LIST_MARK_TO, x, y, 0, distance, 0],
            LaserOnPoint { time_10us } => one(op::LIST_LASER_ON_POINT, time_10us),
            DelayTime { time_10us } => one(op::LIST_DELAY_TIME, time_10us),
            JumpSpeed { units } => one(op::LIST_JUMP_SPEED, units),
            MarkSpeed { units } => one(op::LIST_MARK_SPEED, units),
            LaserOnDelay(d) => delay(op::LIST_LASER_ON_DELAY, d),
            LaserOffDelay(d) => delay(op::LIST_LASER_OFF_DELAY, d),
            JumpDelay(d) => delay(op::LIST_JUMP_DELAY, d),
            PolygonDelay(d) => delay(op::LIST_POLYGON_DELAY, d),
            WritePort { bits } => one(op::LIST_WRITE_PORT, bits),
            MarkCurrent { current } => one(op::LIST_MARK_CURRENT, current),
            QSwitchPeriod { period } => one(op::LIST_Q_SWITCH_PERIOD, period),
            FiberOpenMo { open } => one(op::LIST_FIBER_OPEN_MO, open),
            FiberPulseWidth { ns } => one(op::LIST_FIBER_YLPM_PULSE_WIDTH, ns),
            EndOfList => one(op::LIST_END_OF_LIST, 0),
            Other { words } => words,
        }
    }

    /// Decode six wire words. Lossless: anything that does not fit a variant exactly is [`ListCommand::Other`].
    pub fn from_words(words: [u16; 6]) -> Self {
        use ListCommand::*;
        let [opcode, a, b, c, d, e] = words;
        let only_a = b == 0 && c == 0 && d == 0 && e == 0;
        let only_ab = c == 0 && d == 0 && e == 0;
        let motion = c == 0 && e == 0;
        let dw = DelayWord { magnitude: a, sign: b };
        let decoded = match opcode {
            op::LIST_READY_MARK if a == 0 && only_a => Some(ReadyMark),
            op::LIST_END_OF_LIST if a == 0 && only_a => Some(EndOfList),
            op::LIST_JUMP_TO if motion => Some(JumpTo { x: a, y: b, distance: d }),
            op::LIST_MARK_TO if motion => Some(MarkTo { x: a, y: b, distance: d }),
            op::LIST_LASER_ON_POINT if only_a => Some(LaserOnPoint { time_10us: a }),
            op::LIST_DELAY_TIME if only_a => Some(DelayTime { time_10us: a }),
            op::LIST_JUMP_SPEED if only_a => Some(JumpSpeed { units: a }),
            op::LIST_MARK_SPEED if only_a => Some(MarkSpeed { units: a }),
            op::LIST_LASER_ON_DELAY if only_ab => Some(LaserOnDelay(dw)),
            op::LIST_LASER_OFF_DELAY if only_ab => Some(LaserOffDelay(dw)),
            op::LIST_JUMP_DELAY if only_ab => Some(JumpDelay(dw)),
            op::LIST_POLYGON_DELAY if only_ab => Some(PolygonDelay(dw)),
            op::LIST_WRITE_PORT if only_a => Some(WritePort { bits: a }),
            op::LIST_MARK_CURRENT if only_a => Some(MarkCurrent { current: a }),
            op::LIST_Q_SWITCH_PERIOD if only_a => Some(QSwitchPeriod { period: a }),
            op::LIST_FIBER_OPEN_MO if only_a => Some(FiberOpenMo { open: a }),
            op::LIST_FIBER_YLPM_PULSE_WIDTH if only_a => Some(FiberPulseWidth { ns: a }),
            _ => None,
        };
        decoded.unwrap_or(Other { words })
    }

    /// The opcode word.
    pub fn opcode(&self) -> u16 {
        self.to_words()[0]
    }
}

/// One disassembly line, e.g. `JumpTo x=32768 y=32768 dist=0` or `WritePort 0x0100`.
impl fmt::Display for ListCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ListCommand::*;
        let delay = |f: &mut fmt::Formatter<'_>, name: &str, d: DelayWord| write!(f, "{name} {} us", d.micros());
        match *self {
            ReadyMark => f.write_str("ReadyMark"),
            JumpTo { x, y, distance } => write!(f, "JumpTo x={x} y={y} dist={distance}"),
            MarkTo { x, y, distance } => write!(f, "MarkTo x={x} y={y} dist={distance}"),
            LaserOnPoint { time_10us } => write!(f, "LaserOnPoint {time_10us}x10us"),
            DelayTime { time_10us } => write!(f, "DelayTime {time_10us}x10us"),
            JumpSpeed { units } => write!(f, "JumpSpeed {units}"),
            MarkSpeed { units } => write!(f, "MarkSpeed {units}"),
            LaserOnDelay(d) => delay(f, "LaserOnDelay", d),
            LaserOffDelay(d) => delay(f, "LaserOffDelay", d),
            JumpDelay(d) => delay(f, "JumpDelay", d),
            PolygonDelay(d) => delay(f, "PolygonDelay", d),
            WritePort { bits } => write!(f, "WritePort {bits:#06x}"),
            MarkCurrent { current } => write!(f, "MarkCurrent {current}"),
            QSwitchPeriod { period } => write!(f, "QSwitchPeriod {period}"),
            FiberOpenMo { open } => write!(f, "FiberOpenMO {open}"),
            FiberPulseWidth { ns } => write!(f, "FiberPulseWidth {ns}ns"),
            EndOfList => f.write_str("EndOfList"),
            Other { words: [opcode, params @ ..] } => {
                write!(f, "{} {opcode:#06x} {params:?}", op::list_name(opcode))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_galvoplotter() {
        assert_eq!(ListCommand::JumpTo { x: 1, y: 2, distance: 3 }.to_words(), [0x8001, 1, 2, 0, 3, 0]);
        assert_eq!(ListCommand::MarkTo { x: 1, y: 2, distance: 3 }.to_words(), [0x8005, 1, 2, 0, 3, 0]);
        let d = DelayWord { magnitude: 100, sign: 0x8000 };
        assert_eq!(ListCommand::PolygonDelay(d).to_words(), [0x800f, 100, 0x8000, 0, 0, 0]);
        assert_eq!(ListCommand::FiberPulseWidth { ns: 200 }.to_words(), [0x8026, 200, 0, 0, 0, 0]);
        assert_eq!(ListCommand::EndOfList.to_words(), [0x8002, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn decode_is_lossless() {
        let samples = [
            [0x8001, 10, 20, 0, 5, 0],
            [0x8001, 10, 20, 7, 5, 0], // non-zero angle: Other
            [0x8011, 0x0101, 0, 0, 0, 0],
            [0x8011, 1, 1, 0, 0, 0], // stray word: Other
            [0x8007, 50, 0, 0, 0, 0],
            [0x8050, 1, 2, 3, 4, 5],
            [0x8002, 0, 0, 0, 0, 0],
            [0x8051, 0, 0, 0, 0, 0],
            [0x0000, 0, 0, 0, 0, 0],
        ];
        for w in samples {
            assert_eq!(ListCommand::from_words(w).to_words(), w, "{w:04x?}");
        }
        assert!(matches!(ListCommand::from_words([0x8001, 10, 20, 7, 5, 0]), ListCommand::Other { .. }));
        assert!(matches!(ListCommand::from_words([0x8011, 1, 1, 0, 0, 0]), ListCommand::Other { .. }));
        assert_eq!(ListCommand::from_words([0x8021, 1, 0, 0, 0, 0]), ListCommand::FiberOpenMo { open: 1 });
    }

    #[test]
    fn display() {
        assert_eq!(ListCommand::WritePort { bits: 0x100 }.to_string(), "WritePort 0x0100");
        assert_eq!(ListCommand::LaserOnDelay(DelayWord { magnitude: 8, sign: 0x8000 }).to_string(), "LaserOnDelay -8 us");
        assert_eq!(ListCommand::Other { words: [0x8050, 1, 0, 0, 0, 0] }.to_string(), "listJptSetParam 0x8050 [1, 0, 0, 0, 0]");
    }
}
