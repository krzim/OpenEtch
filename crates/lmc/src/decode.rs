//! Reading packets back: typed decode, a text disassembler, a motion trace and the light-only emission check.
//!
//! The board runs a list's packets in order and stops at the first `listEndOfList`. [`decode`] and [`trace`] follow
//! that; [`disassemble`] and [`emits_light_only`] also look past it, so a command hidden behind an end-of-list is
//! shown and counted, never skipped.

// Disassembly rules adapted from openlaser-rs @ 9d81a15 `crates/lmc/src/decode.rs` (MIT OR Apache-2.0, used here
// under MIT); command semantics from meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (MIT).
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

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::command::ListCommand;
use crate::packet::{PAD, Packet};
use crate::units::CENTER;
use crate::{LASER_ENABLE_BIT, op};

/// The words the board executes: every packet's slots in order, up to and including the first `listEndOfList`.
fn executed_words(packets: &[Packet]) -> Vec<[u16; 6]> {
    let mut out = Vec::new();
    for w in packets.iter().flat_map(Packet::words) {
        out.push(w);
        if w[0] == op::LIST_END_OF_LIST {
            break;
        }
    }
    out
}

/// One packet's commands up to and including its first `listEndOfList` (all 256 when there is none).
pub fn decode_packet(packet: &Packet) -> Vec<ListCommand> {
    executed_words(std::slice::from_ref(packet)).into_iter().map(ListCommand::from_words).collect()
}

/// A whole list as the board executes it: the packets' commands in order, up to and including the first
/// `listEndOfList`; the padding after it is dropped. `decode(&builder.finish())` equals the builder's commands
/// followed by `EndOfList`.
pub fn decode(packets: &[Packet]) -> Vec<ListCommand> {
    executed_words(packets).into_iter().map(ListCommand::from_words).collect()
}

/// One line per slot: `p<packet>:<slot>  <six hex words>  <command>`. Per packet: every slot up to the first
/// `listEndOfList`, plus any later slot that is not plain padding.
pub fn disassemble(packets: &[Packet]) -> String {
    let mut out = String::new();
    for (pi, packet) in packets.iter().enumerate() {
        let words = packet.words();
        let eol = words.iter().position(|w| w[0] == op::LIST_END_OF_LIST);
        let last_real = words.iter().rposition(|w| *w != PAD);
        let end = match (eol, last_real) {
            (Some(e), Some(l)) => e.max(l) + 1,
            (Some(e), None) => e + 1,
            (None, _) => words.len(),
        };
        for (si, w) in words.into_iter().enumerate().take(end) {
            let hex: Vec<String> = w.iter().map(|v| format!("{v:04x}")).collect();
            // Writing to a String cannot fail.
            let _ = writeln!(out, "p{pi}:{si:03}  {}  {}", hex.join(" "), ListCommand::from_words(w));
        }
    }
    out
}

/// True iff no slot of any packet (padding and anything after an end-of-list included) can make light.
///
/// Conservative allowlist: every slot must be `ReadyMark`, `JumpTo`, `JumpSpeed`, `JumpDelay`, `DelayTime`,
/// `EndOfList`, or `WritePort` without the laser-enable bit (bit 0). So `MarkTo`, `LaserOnPoint`, `FiberOpenMO`
/// (open or close), a port write with bit 0, and every other opcode (mark parameters, unknown or malformed slots)
/// make it false. Every [`crate::LightList`] output passes.
pub fn emits_light_only(packets: &[Packet]) -> bool {
    packets.iter().flat_map(Packet::words).all(|w| match ListCommand::from_words(w) {
        ListCommand::WritePort { bits } => bits & LASER_ENABLE_BIT == 0,
        ListCommand::ReadyMark
        | ListCommand::JumpTo { .. }
        | ListCommand::JumpSpeed { .. }
        | ListCommand::JumpDelay(_)
        | ListCommand::DelayTime { .. }
        | ListCommand::EndOfList => true,
        _ => false,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    /// `JumpTo`: laser off.
    Jump,
    /// `MarkTo` or `LaserOnPoint`: fires when the MO is open and port bit 0 is set ([`TraceSegment::emits`]).
    Mark,
}

/// One motion of a list, in galvo units, with the list state it ran under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraceSegment {
    /// Galvo (x, y) before the motion.
    pub from: (u16, u16),
    /// Galvo (x, y) after it (equal to `from` for `LaserOnPoint`).
    pub to: (u16, u16),
    pub kind: TraceKind,
    /// Master oscillator open (by `FiberOpenMO` in this list, or the [`TraceState`] it started from).
    pub mo_open: bool,
    /// Output port word in effect.
    pub port: u16,
}

impl TraceSegment {
    /// A mark with the MO open and the laser-enable bit set: this segment fires.
    pub fn emits(&self) -> bool {
        self.kind == TraceKind::Mark && self.mo_open && self.port & LASER_ENABLE_BIT != 0
    }
}

/// The list-level state [`trace_from`] tracks. Singles sent around the list (e.g. `Fiber_SetMo(1)`,
/// `WritePort(1)` in the machine's prelude) are not in the packets; pass them in as the start state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraceState {
    /// Galvo (x, y).
    pub position: (u16, u16),
    /// Master oscillator open.
    pub mo_open: bool,
    /// Output port word.
    pub port: u16,
}

impl Default for TraceState {
    /// Galvo at the field centre, MO closed, port 0.
    fn default() -> Self {
        TraceState { position: (CENTER, CENTER), mo_open: false, port: 0 }
    }
}

/// [`trace_from`] the default state (centre, MO closed, port 0).
pub fn trace(packets: &[Packet]) -> Vec<TraceSegment> {
    trace_from(packets, TraceState::default()).0
}

/// Walk the executed commands, returning every motion and the final state.
///
/// Reads raw opcodes, so a motion with unusual extra words still moves the trace. `JumpTo` and `MarkTo` give a
/// segment of their kind; `LaserOnPoint` gives a zero-length `Mark` at the current position.
pub fn trace_from(packets: &[Packet], start: TraceState) -> (Vec<TraceSegment>, TraceState) {
    let mut state = start;
    let mut out = Vec::new();
    for w in executed_words(packets) {
        let [opcode, a, b, ..] = w;
        let kind = match opcode {
            op::LIST_JUMP_TO => Some((TraceKind::Jump, (a, b))),
            op::LIST_MARK_TO => Some((TraceKind::Mark, (a, b))),
            op::LIST_LASER_ON_POINT => Some((TraceKind::Mark, state.position)),
            op::LIST_FIBER_OPEN_MO => {
                state.mo_open = a != 0;
                None
            }
            op::LIST_WRITE_PORT => {
                state.port = a;
                None
            }
            _ => None,
        };
        if let Some((kind, to)) = kind {
            out.push(TraceSegment { from: state.position, to, kind, mo_open: state.mo_open, port: state.port });
            state.position = to;
        }
    }
    (out, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LightList, MarkingList};

    fn marking() -> Vec<Packet> {
        let mut l = MarkingList::new();
        l.write_port(1).mark_speed(100).unwrap();
        l.jump_to(1000, 1000).fiber_open_mo().mark_to(2000, 1000).laser_on_point(5).fiber_close_mo().jump_to(0, 0);
        l.finish()
    }

    #[test]
    fn marking_list_is_not_light_only() {
        assert!(!emits_light_only(&marking()));
        let only_mo_close = {
            let mut l = MarkingList::new();
            l.fiber_close_mo();
            l.finish()
        };
        assert!(!emits_light_only(&only_mo_close));
        let laser_port = {
            let mut l = MarkingList::new();
            l.write_port(0x0101);
            l.finish()
        };
        assert!(!emits_light_only(&laser_port));
        let red_only = {
            let mut l = MarkingList::new();
            l.write_port(0x0100).jump_to(1, 1);
            l.finish()
        };
        assert!(emits_light_only(&red_only));
        assert!(emits_light_only(&[Packet::empty()]));
    }

    #[test]
    fn hidden_command_after_end_of_list_is_caught() {
        let words = [[op::LIST_END_OF_LIST, 0, 0, 0, 0, 0], [op::LIST_MARK_TO, 1, 1, 0, 0, 0]];
        let packets = [Packet::from_words(&words).unwrap()];
        assert!(!emits_light_only(&packets));
        assert_eq!(decode(&packets), vec![ListCommand::EndOfList]);
        assert!(disassemble(&packets).contains("p0:001"));
    }

    #[test]
    fn trace_tracks_position_mo_and_port() {
        let segs = trace(&marking());
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[0], TraceSegment { from: (CENTER, CENTER), to: (1000, 1000), kind: TraceKind::Jump, mo_open: false, port: 1 });
        assert_eq!(segs[1], TraceSegment { from: (1000, 1000), to: (2000, 1000), kind: TraceKind::Mark, mo_open: true, port: 1 });
        assert!(segs[1].emits());
        assert_eq!((segs[2].from, segs[2].to, segs[2].kind), ((2000, 1000), (2000, 1000), TraceKind::Mark));
        assert_eq!(segs[3], TraceSegment { from: (2000, 1000), to: (0, 0), kind: TraceKind::Jump, mo_open: false, port: 1 });
        assert!(!segs[3].emits());
    }

    #[test]
    fn trace_continues_across_packets_and_from_state() {
        let mut l = LightList::new();
        l.red_pointer_on();
        for i in 0..300u16 {
            l.jump_to(i, i);
        }
        let packets = l.finish();
        assert_eq!(packets.len(), 2);
        let start = TraceState { position: (7, 7), mo_open: true, port: 0 };
        let (segs, end) = trace_from(&packets, start);
        assert_eq!(segs.len(), 300);
        assert_eq!(segs[0].from, (7, 7));
        assert!(segs.iter().all(|s| s.kind == TraceKind::Jump && s.port == 0x0100 && !s.emits()));
        assert_eq!(end, TraceState { position: (299, 299), mo_open: true, port: 0x0100 });
    }

    #[test]
    fn decode_packet_and_disassemble() {
        let packets = marking();
        assert_eq!(decode_packet(&packets[0]), decode(&packets));
        let text = disassemble(&packets);
        assert!(text.starts_with("p0:000  8051 0000 0000 0000 0000 0000  ReadyMark\n"));
        assert!(text.contains("MarkTo x=2000 y=1000 dist=1000"));
        assert!(text.trim_end().ends_with("EndOfList"));
        assert_eq!(text.lines().count(), 10);
    }
}
