//! List packets: exactly 3072 bytes = 256 list commands of 12 bytes.
//!
//! galvoplotter starts every packet as 256 copies of `listEndOfList` (`0x8002`, zero parameters) and overwrites
//! slots from the front, so a partly filled packet is padded with `0x8002` no-ops. We do the same. A finished list
//! also carries an explicit `listEndOfList` as its last real command (the builders add it).

// Ported from openlaser-rs @ 9d81a15 `crates/lmc/src/packet.rs` (MIT OR Apache-2.0, used here under MIT); padding
// from meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (`nop`, `empty`, `_list_write`; MIT).
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

use crate::{LmcError, Result, op};

/// Bytes in one list packet.
pub const PACKET_LEN: usize = 3072;
/// List commands in one packet.
pub const SLOTS: usize = 256;
/// Bytes in one list command (and in one single command).
pub const COMMAND_LEN: usize = 12;

/// The padding slot: `listEndOfList` with zero parameters.
pub(crate) const PAD: [u16; 6] = [op::LIST_END_OF_LIST, 0, 0, 0, 0, 0];

/// One 3072-byte list packet, exactly as written to the bulk endpoint.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Packet(Box<[u8; PACKET_LEN]>);

impl Packet {
    /// Up to 256 commands (as six words each), padded with `listEndOfList`.
    pub fn from_words(words: &[[u16; 6]]) -> Result<Self> {
        if words.len() > SLOTS {
            return Err(LmcError::TooManyCommands(words.len()));
        }
        Ok(Packet::fill(words))
    }

    /// An all-padding packet (galvoplotter's empty list; what an abort sends).
    pub fn empty() -> Self {
        Packet::fill(&[])
    }

    /// The first 256 of `words`, then padding.
    fn fill(words: &[[u16; 6]]) -> Self {
        let mut bytes = Box::new([0u8; PACKET_LEN]);
        let words = words.iter().copied().chain(std::iter::repeat(PAD));
        for (slot, word) in bytes.as_chunks_mut::<COMMAND_LEN>().0.iter_mut().zip(words) {
            for (pair, value) in slot.as_chunks_mut::<2>().0.iter_mut().zip(word) {
                *pair = value.to_le_bytes();
            }
        }
        Packet(bytes)
    }

    /// Wraps received or stored bytes; refuses anything but exactly 3072.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let array: [u8; PACKET_LEN] = bytes.try_into().map_err(|_| LmcError::PacketLength(bytes.len()))?;
        Ok(Packet(Box::new(array)))
    }

    pub fn as_bytes(&self) -> &[u8; PACKET_LEN] {
        &self.0
    }

    /// Slot `i` (0..256) as six words.
    pub fn slot(&self, i: usize) -> Option<[u16; 6]> {
        let bytes = self.0.as_chunks::<COMMAND_LEN>().0.get(i)?;
        let mut out = [0u16; 6];
        for (value, pair) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *value = u16::from_le_bytes(*pair);
        }
        Some(out)
    }

    /// All 256 slots as words, padding included.
    pub fn words(&self) -> Vec<[u16; 6]> {
        (0..SLOTS).filter_map(|i| self.slot(i)).collect()
    }

    /// Slots before the first `listEndOfList` (256 when there is none).
    pub fn real_commands(&self) -> usize {
        self.words().iter().position(|w| w[0] == op::LIST_END_OF_LIST).unwrap_or(SLOTS)
    }
}

impl fmt::Debug for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Packet").field("real_commands", &self.real_commands()).finish_non_exhaustive()
    }
}

/// Split a list into packets of 256 commands; the last one is padded.
pub(crate) fn packetize(words: &[[u16; 6]]) -> Vec<Packet> {
    words.chunks(SLOTS).map(Packet::fill).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_packet_is_padded_with_end_of_list() {
        let p = Packet::from_words(&[[op::LIST_JUMP_TO, 1, 2, 0, 3, 0]]).unwrap();
        assert_eq!(p.as_bytes().len(), PACKET_LEN);
        assert_eq!(p.slot(0), Some([op::LIST_JUMP_TO, 1, 2, 0, 3, 0]));
        assert!((1..SLOTS).all(|i| p.slot(i) == Some(PAD)));
        assert_eq!(&p.as_bytes()[12..24], &[0x02, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(p.real_commands(), 1);
        assert_eq!(p.slot(SLOTS), None);
    }

    #[test]
    fn sizes_are_enforced() {
        assert_eq!(Packet::from_words(&vec![PAD; SLOTS + 1]), Err(LmcError::TooManyCommands(SLOTS + 1)));
        assert!(Packet::from_words(&vec![[op::LIST_DELAY_TIME, 1, 0, 0, 0, 0]; SLOTS]).is_ok());
        assert_eq!(Packet::from_bytes(&[0; 3071]), Err(LmcError::PacketLength(3071)));
        let p = Packet::from_words(&[[op::LIST_DELAY_TIME, 5, 0, 0, 0, 0]]).unwrap();
        assert_eq!(Packet::from_bytes(p.as_bytes()).unwrap(), p);
        assert_eq!(Packet::empty(), Packet::from_words(&[]).unwrap());
    }

    #[test]
    fn packetize_splits_at_256() {
        let words = vec![[op::LIST_DELAY_TIME, 1, 0, 0, 0, 0]; 300];
        let packets = packetize(&words);
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].real_commands(), SLOTS);
        assert_eq!(packets[1].real_commands(), 44);
    }
}
