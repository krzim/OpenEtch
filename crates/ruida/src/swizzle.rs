//! Byte scrambling ("swizzle"): every byte on the wire and in an `.rd` file is scrambled with a one-byte magic key.
//!
//! Scramble one byte `b` with key `m`: swap bit 7 and bit 0, XOR with `m`, add 1 (mod 256). Unscramble runs the steps
//! backwards. Both are bijections on `0..=255` for every key. Most controllers use [`DEFAULT_MAGIC`] (`0x88`); the
//! RDC634XG uses `0x11`, and MeerK40t also lists `0x38`.
//!
//! The same scrambling is used over UDP, over USB serial and in `.rd` files.

// Facts and algorithm from meerk40t/meerk40t @ d511944 `meerk40t/ruida/rdjob.py` (`swizzle_byte`, `unswizzle_byte`,
// `determine_magic_via_histogram`, `magic_keys`) and `test/test_ruida.py` (expected keys), re-written in Rust.
//
// MIT License
//
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

/// The usual key (RDC644x family, LightBurn and RDWorks default).
pub const DEFAULT_MAGIC: u8 = 0x88;
/// The key MeerK40t uses for the RDC634XG.
pub const MAGIC_634XG: u8 = 0x11;
/// A third key MeerK40t lists.
pub const MAGIC_ALT: u8 = 0x38;

/// Swap bit 7 and bit 0.
const fn swap_ends(b: u8) -> u8 {
    (b & 0x7E) | ((b & 0x80) >> 7) | ((b & 0x01) << 7)
}

/// Scramble one byte.
pub const fn scramble_byte(b: u8, magic: u8) -> u8 {
    (swap_ends(b) ^ magic).wrapping_add(1)
}

/// Unscramble one byte.
pub const fn unscramble_byte(b: u8, magic: u8) -> u8 {
    swap_ends(b.wrapping_sub(1) ^ magic)
}

/// Both lookup tables for one key.
#[derive(Clone, PartialEq, Eq)]
pub struct Swizzle {
    magic: u8,
    fwd: [u8; 256],
    inv: [u8; 256],
}

impl std::fmt::Debug for Swizzle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Swizzle(0x{:02X})", self.magic)
    }
}

impl Default for Swizzle {
    fn default() -> Self {
        Swizzle::new(DEFAULT_MAGIC)
    }
}

impl Swizzle {
    pub fn new(magic: u8) -> Self {
        let mut fwd = [0u8; 256];
        let mut inv = [0u8; 256];
        for b in 0..=255u8 {
            let s = scramble_byte(b, magic);
            fwd[usize::from(b)] = s;
            inv[usize::from(s)] = b;
        }
        Swizzle { magic, fwd, inv }
    }

    pub fn magic(&self) -> u8 {
        self.magic
    }

    pub fn scramble(&self, data: &[u8]) -> Vec<u8> {
        data.iter().map(|&b| self.fwd[usize::from(b)]).collect()
    }

    pub fn unscramble(&self, data: &[u8]) -> Vec<u8> {
        data.iter().map(|&b| self.inv[usize::from(b)]).collect()
    }

    pub fn scramble_in_place(&self, data: &mut [u8]) {
        for b in data {
            *b = self.fwd[usize::from(*b)];
        }
    }

    pub fn unscramble_in_place(&self, data: &mut [u8]) {
        for b in data {
            *b = self.inv[usize::from(*b)];
        }
    }
}

/// The key implied by a scrambled `GetMemory(0x02FE)` (card ID request, `DA 00 05 7E`), which LightBurn and RDWorks
/// send first: MeerK40t's emulator switches keys on it. `None` when the 4 bytes are not that request under any key.
pub fn magic_from_card_id_request(scrambled: &[u8]) -> Option<u8> {
    const REQ: [u8; 4] = [0xDA, 0x00, 0x05, 0x7E];
    if scrambled.len() != 4 {
        return None;
    }
    (0..=255u8).find(|&m| REQ.iter().zip(scrambled).all(|(&r, &s)| scramble_byte(r, m) == s))
}

/// MeerK40t's histogram guess: about half of an `.rd` file is the byte 0, which scrambles to `magic + 1`, so the most
/// frequent byte minus one is most likely the key (runs count 5x). Only a guess: [`crate::rd::detect_magic`] checks it.
pub fn guess_magic_by_histogram(scrambled: &[u8]) -> Option<u8> {
    if scrambled.is_empty() {
        return None;
    }
    let mut histogram = [0u64; 256];
    let mut prev: Option<u8> = None;
    for &d in scrambled {
        histogram[usize::from(d)] += if prev == Some(d) { 5 } else { 1 };
        prev = Some(d);
    }
    let mut best = 0u8;
    for b in 1..=255u8 {
        if histogram[usize::from(b)] > histogram[usize::from(best)] {
            best = b;
        }
    }
    Some(best.wrapping_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bijective_for_every_key() {
        for m in 0..=255u8 {
            let s = Swizzle::new(m);
            let mut seen = [false; 256];
            for b in 0..=255u8 {
                let x = scramble_byte(b, m);
                assert!(!seen[usize::from(x)], "key {m:#04x} maps two bytes to {x:#04x}");
                seen[usize::from(x)] = true;
                assert_eq!(unscramble_byte(x, m), b);
                assert_eq!(s.fwd[usize::from(b)], x);
                assert_eq!(s.inv[usize::from(x)], b);
            }
        }
    }

    #[test]
    fn known_wire_bytes_for_0x88() {
        // ACK 0xCC and ERR 0xCD are the 0xC6 / 0x46 replies seen on the wire; 0 scrambles to magic + 1.
        assert_eq!(scramble_byte(0xCC, 0x88), 0xC6);
        assert_eq!(scramble_byte(0xCD, 0x88), 0x46);
        assert_eq!(scramble_byte(0xCF, 0x88), 0x48);
        assert_eq!(scramble_byte(0x00, 0x88), 0x89);
    }

    #[test]
    fn meerk40t_magic_key_vectors() {
        // test/test_ruida.py: scrambled DA 00 05 7E under each key.
        let cases: [(&[u8], u8); 8] = [
            (b"\xd4\x89\r\xf7", 0x88),
            (b"\xd9\x84\x08\xfe", 0x83),
            (b"i4\xb8N", 0x33),
            (b"I\x14\x98n", 0x13),
            (b"z#\xa7]", 0x22),
            (b"K\x12\x96p", 0x11),
            (b"-x\xf4\n", 0x77),
            (b"\xb6\xefk\x91", 0xEE),
        ];
        for (bytes, magic) in cases {
            assert_eq!(magic_from_card_id_request(bytes), Some(magic));
            assert_eq!(Swizzle::new(magic).scramble(&[0xDA, 0x00, 0x05, 0x7E]), bytes);
        }
        assert_eq!(magic_from_card_id_request(b"abc"), None);
    }

    #[test]
    fn in_place_matches_copying() {
        let s = Swizzle::new(0x11);
        let data: Vec<u8> = (0..=255u8).collect();
        let mut buf = data.clone();
        s.scramble_in_place(&mut buf);
        assert_eq!(buf, s.scramble(&data));
        s.unscramble_in_place(&mut buf);
        assert_eq!(buf, data);
    }

    #[test]
    fn histogram_guess_on_zero_heavy_data() {
        let mut plain = vec![0u8; 100];
        plain.extend([0x88, 0x01, 0x02, 0xA8, 0x05]);
        for m in [0x88, 0x11, 0x38] {
            assert_eq!(guess_magic_by_histogram(&Swizzle::new(m).scramble(&plain)), Some(m));
        }
        assert_eq!(guess_magic_by_histogram(&[]), None);
    }
}
