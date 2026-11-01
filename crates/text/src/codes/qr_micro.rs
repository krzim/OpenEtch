//! Micro QR Code (ISO/IEC 18004:2015, versions M1-M4) and Rectangular Micro QR Code (rMQR, ISO/IEC 23941:2022, the 32
//! sizes R7x43 to R17x139). rxing (Apache-2.0) only decodes these; its version tables (error correction blocks,
//! character count bits, alignment columns, function areas) are used here so the encoder and the tests' decoder
//! agree. The rMQR function patterns follow Zint's `rmqr_setup_grid` (BSD-3-Clause, see ATTRIBUTION.md).
//!
//! Data is one segment in the most compact single mode (numeric, alphanumeric or bytes as UTF-8); the smallest
//! symbol that holds it is used. In M1 and M3 the 4-bit final data codeword is always left zero (the next size is
//! used when the data would need it), so readers that place those four bits either way agree.

// Module grids read most clearly with (x, y) indices.
#![allow(clippy::needless_range_loop)]

use rxing::qrcode::common::{ErrorCorrectionLevel, Mode, Version};

use super::Grid;

/// QR's GF(256) (polynomial 0x11D): the Reed-Solomon error correction codewords of `data` (`n` of them, generator
/// roots α^0 .. α^(n-1)).
pub(crate) fn rs_ec(data: &[u8], n: usize) -> Vec<u8> {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    for i in 0..255 {
        exp[i] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= 0x11D;
        }
    }
    for i in 255..512 {
        exp[i] = exp[i - 255];
    }
    let mul = |a: u8, b: u8| if a == 0 || b == 0 { 0 } else { exp[log[a as usize] as usize + log[b as usize] as usize] };
    // Generator, highest power first.
    let mut g = vec![1u8];
    for i in 0..n {
        let mut next = vec![0u8; g.len() + 1];
        for (j, c) in g.iter().enumerate() {
            next[j] ^= *c;
            next[j + 1] ^= mul(*c, exp[i]);
        }
        g = next;
    }
    let mut rem = vec![0u8; n];
    for d in data {
        let f = d ^ rem[0];
        rem.remove(0);
        rem.push(0);
        if f != 0 {
            for j in 0..n {
                rem[j] ^= mul(g[j + 1], f);
            }
        }
    }
    rem
}

/// QR's alphanumeric characters, in value order.
const ALNUM: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seg {
    Numeric,
    Alnum,
    Byte,
}

impl Seg {
    fn of(data: &str) -> Seg {
        if data.bytes().all(|b| b.is_ascii_digit()) {
            Seg::Numeric
        } else if data.bytes().all(|b| ALNUM.contains(&b)) {
            Seg::Alnum
        } else {
            Seg::Byte
        }
    }

    fn mode(self) -> Mode {
        match self {
            Seg::Numeric => Mode::NUMERIC,
            Seg::Alnum => Mode::ALPHANUMERIC,
            Seg::Byte => Mode::BYTE,
        }
    }

    /// The character count field's value.
    fn count(self, data: &str) -> usize {
        data.len()
    }
}

fn push(bits: &mut Vec<bool>, value: u32, n: u32) {
    for i in (0..n).rev() {
        bits.push((value >> i) & 1 == 1);
    }
}

/// The segment's data bits (without mode and count).
fn payload(seg: Seg, data: &str) -> Vec<bool> {
    let mut b = Vec::new();
    let d = data.as_bytes();
    match seg {
        Seg::Numeric => {
            for c in d.chunks(3) {
                let v = c.iter().fold(0u32, |a, x| a * 10 + u32::from(x - b'0'));
                push(&mut b, v, [0, 4, 7, 10][c.len()]);
            }
        }
        Seg::Alnum => {
            let val = |x: &u8| ALNUM.iter().position(|a| a == x).expect("alphanumeric") as u32;
            for c in d.chunks(2) {
                if c.len() == 2 {
                    push(&mut b, val(&c[0]) * 45 + val(&c[1]), 11);
                } else {
                    push(&mut b, val(&c[0]), 6);
                }
            }
        }
        Seg::Byte => d.iter().for_each(|x| push(&mut b, u32::from(*x), 8)),
    }
    b
}

/// Data bits padded to `codewords` bytes: terminator (as much as fits), zero bits to a byte boundary, then 0xEC and
/// 0x11 in turn.
fn padded(mut bits: Vec<bool>, terminator: usize, capacity_bits: usize, codewords: usize) -> Vec<u8> {
    let t = terminator.min(capacity_bits - bits.len());
    bits.extend(std::iter::repeat_n(false, t));
    while !bits.len().is_multiple_of(8) {
        bits.push(false);
    }
    let mut out: Vec<u8> = bits.chunks(8).map(|c| c.iter().fold(0u8, |a, b| (a << 1) | u8::from(*b))).collect();
    let mut pad = [0xEC, 0x11].into_iter().cycle();
    while out.len() < codewords {
        out.push(pad.next().expect("cycles"));
    }
    out
}

/// The 15-bit BCH(15,5) code of 5 data bits (generator 0x537), as QR and Micro QR format information use.
fn bch15(data5: u32) -> u32 {
    let mut v = data5 << 10;
    for i in (10..15).rev() {
        if v & (1 << i) != 0 {
            v ^= 0x537 << (i - 10);
        }
    }
    (data5 << 10) | v
}

/// The 18-bit BCH(18,6) code of 6 data bits (generator 0x1F25), as rMQR format information uses.
fn bch18(data6: u32) -> u32 {
    let mut v = data6 << 12;
    for i in (12..18).rev() {
        if v & (1 << i) != 0 {
            v ^= 0x1F25 << (i - 12);
        }
    }
    (data6 << 12) | v
}

fn new_grid(w: usize, h: usize) -> Grid {
    vec![vec![false; w]; h]
}

/// The 7 x 7 finder pattern with its top-left corner at (x0, y0).
fn finder(g: &mut Grid, x0: usize, y0: usize) {
    for dy in 0..7 {
        for dx in 0..7 {
            let d = (dx as i32 - 3).abs().max((dy as i32 - 3).abs());
            g[y0 + dy][x0 + dx] = d != 2;
        }
    }
}

/// Put `bits` into the modules the zigzag visits (columns pairs from `x_start` leftwards, upward first), skipping
/// `function` modules; modules left over stay light.
fn place(g: &mut Grid, function: &dyn Fn(usize, usize) -> bool, x_start: usize, bits: &[bool]) {
    let h = g.len();
    let mut it = bits.iter();
    let mut up = true;
    let mut x = x_start as i32;
    while x > 0 {
        for row in 0..h {
            let y = if up { h - 1 - row } else { row };
            for col in 0..2 {
                let xx = (x - col) as usize;
                if !function(xx, y) {
                    g[y][xx] = it.next().copied().unwrap_or(false);
                }
            }
        }
        up = !up;
        x -= 2;
    }
}

fn level_of(ec: Option<&str>, allowed: &[&str], default: &str) -> Result<String, String> {
    let l = ec.map(|e| e.trim().to_ascii_uppercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| default.into());
    if allowed.contains(&l.as_str()) {
        Ok(l)
    } else {
        Err(format!("error correction must be one of {}", allowed.join(", ")))
    }
}

/// Micro QR's (total, data, error correction) codewords for version 1-4 and level, `None` where it does not exist.
fn micro_capacity(version: u32, level: &str) -> Option<(usize, usize, usize, u32)> {
    // (total, data codewords, ec codewords, symbol number)
    match (version, level) {
        (1, "L") => Some((5, 3, 2, 0)),
        (2, "L") => Some((10, 5, 5, 1)),
        (2, "M") => Some((10, 4, 6, 2)),
        (3, "L") => Some((17, 11, 6, 3)),
        (3, "M") => Some((17, 9, 8, 4)),
        (4, "L") => Some((24, 16, 8, 5)),
        (4, "M") => Some((24, 14, 10, 6)),
        (4, "Q") => Some((24, 10, 14, 7)),
        _ => None,
    }
}

/// Encode `data` as a Micro QR Code: `grid[y][x]`, dark `true`. `ec` is L (default), M or Q (M1 has error detection
/// only and is used for L).
pub fn micro_qr(data: &str, ec: Option<&str>) -> Result<Grid, String> {
    let level = level_of(ec, &["L", "M", "Q"], "L")?;
    let seg = Seg::of(data);
    let body = payload(seg, data);
    for version in 1..=4u32 {
        let Some((total, dc, ecn, symbol)) = micro_capacity(version, &level) else { continue };
        let allowed = match seg {
            Seg::Numeric => true,
            Seg::Alnum => version >= 2,
            Seg::Byte => version >= 3,
        };
        if !allowed {
            continue;
        }
        let v = Version::Micro(version).map_err(|e| e.to_string())?;
        let count_bits = seg.mode().CharacterCountBits(v);
        let half = version % 2 == 1;
        // The 4-bit codeword of M1 / M3 stays zero.
        let capacity = dc * 8 - if half { 8 } else { 0 };
        let mut bits = Vec::new();
        push(&mut bits, [0, 1, 2][seg as usize], version - 1);
        if seg.count(data) >= 1 << count_bits {
            continue;
        }
        push(&mut bits, seg.count(data) as u32, count_bits);
        bits.extend(&body);
        if bits.len() > capacity {
            continue;
        }
        let full = if half { dc - 1 } else { dc };
        let mut cw = padded(bits, 2 * version as usize + 1, capacity, full);
        if half {
            cw.push(0);
        }
        let ecc = rs_ec(&cw, ecn);
        debug_assert_eq!(cw.len() + ecc.len(), total);
        let mut stream = Vec::with_capacity(total * 8);
        for (i, c) in cw.iter().enumerate() {
            let n = if half && i == dc - 1 { 4 } else { 8 };
            push(&mut stream, u32::from(*c), n);
        }
        ecc.iter().for_each(|c| push(&mut stream, u32::from(*c), 8));
        let dim = (9 + 2 * version) as usize;
        let fp = v.buildFunctionPattern().map_err(|e| e.to_string())?;
        let function = |x: usize, y: usize| fp.get(x as u32, y as u32);
        let mut data_grid = new_grid(dim, dim);
        place(&mut data_grid, &function, dim - 1, &stream);
        let masks: [fn(usize, usize) -> bool; 4] = [
            |_, y| y % 2 == 0,
            |x, y| (y / 2 + x / 3) % 2 == 0,
            |x, y| ((y * x) % 6) < 3,
            |x, y| (y + x + (y * x) % 3) % 2 == 0,
        ];
        let mut best: Option<(u32, Grid)> = None;
        for (m, mask) in masks.iter().enumerate() {
            let mut g = data_grid.clone();
            for (y, row) in g.iter_mut().enumerate() {
                for (x, cell) in row.iter_mut().enumerate() {
                    if !function(x, y) && mask(x, y) {
                        *cell = !*cell;
                    }
                }
            }
            // Function patterns: finder, separators, timing.
            finder(&mut g, 0, 0);
            for i in 0..8 {
                g[7][i] = false;
                g[i][7] = false;
            }
            for i in 8..dim {
                g[0][i] = i % 2 == 0;
                g[i][0] = i % 2 == 0;
            }
            let fmt = bch15((symbol << 2) | m as u32) ^ 0x4445;
            let cells: Vec<(usize, usize)> = (1..=8).map(|x| (x, 8)).chain((1..=7).rev().map(|y| (8, y))).collect();
            for (k, (x, y)) in cells.into_iter().enumerate() {
                g[y][x] = (fmt >> (14 - k)) & 1 == 1;
            }
            // ISO 18004 7.8.3.2: the darker the right and bottom edges, the better.
            let s1: u32 = (1..dim).map(|y| u32::from(g[y][dim - 1])).sum();
            let s2: u32 = (1..dim).map(|x| u32::from(g[dim - 1][x])).sum();
            let score = if s1 <= s2 { s1 * 16 + s2 } else { s2 * 16 + s1 };
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, g));
            }
        }
        return Ok(best.expect("four masks").1);
    }
    Err(format!("{data:?} is too long for Micro QR at error correction {level} (M4 holds up to 35 digits, 21 letters or 15 bytes at L)"))
}

/// rMQR's 32 sizes (height, width), version 1 first.
const RMQR_SIZES: [(usize, usize); 32] = [
    (7, 43), (7, 59), (7, 77), (7, 99), (7, 139),
    (9, 43), (9, 59), (9, 77), (9, 99), (9, 139),
    (11, 27), (11, 43), (11, 59), (11, 77), (11, 99), (11, 139),
    (13, 27), (13, 43), (13, 59), (13, 77), (13, 99), (13, 139),
    (15, 43), (15, 59), (15, 77), (15, 99), (15, 139),
    (17, 43), (17, 59), (17, 77), (17, 99), (17, 139),
];

/// Encode `data` as a Rectangular Micro QR Code (the smallest symbol by area that holds it): `grid[y][x]`. `ec` is
/// M (default) or H.
pub fn rmqr(data: &str, ec: Option<&str>) -> Result<Grid, String> {
    let level = level_of(ec, &["M", "H"], "M")?;
    let ecl = if level == "H" { ErrorCorrectionLevel::H } else { ErrorCorrectionLevel::M };
    let seg = Seg::of(data);
    let body = payload(seg, data);
    let mut order: Vec<usize> = (0..32).collect();
    order.sort_by_key(|&i| (RMQR_SIZES[i].0 * RMQR_SIZES[i].1, RMQR_SIZES[i].0));
    for i in order {
        let n = i as u32 + 1;
        let v = Version::rMQR(n).map_err(|e| e.to_string())?;
        let blocks = v.getECBlocksForLevel(ecl);
        let ecpb = blocks.getECCodewordsPerBlock() as usize;
        let sizes: Vec<usize> = blocks
            .getECBlocks()
            .iter()
            .flat_map(|b| std::iter::repeat_n(b.getDataCodewords() as usize, b.getCount() as usize))
            .collect();
        let dc: usize = sizes.iter().sum();
        let count_bits = seg.mode().CharacterCountBits(v);
        if seg.count(data) >= 1 << count_bits {
            continue;
        }
        let mut bits = Vec::new();
        push(&mut bits, [1, 2, 3][seg as usize], 3);
        push(&mut bits, seg.count(data) as u32, count_bits);
        bits.extend(&body);
        if bits.len() > dc * 8 {
            continue;
        }
        let cw = padded(bits, 3, dc * 8, dc);
        // Blocks, then interleaved: data codewords column by column, then error correction.
        let mut split = Vec::new();
        let mut at = 0;
        for s in &sizes {
            split.push(cw[at..at + s].to_vec());
            at += s;
        }
        let ecs: Vec<Vec<u8>> = split.iter().map(|b| rs_ec(b, ecpb)).collect();
        let mut stream = Vec::new();
        for k in 0..sizes.iter().copied().max().unwrap_or(0) {
            for b in &split {
                if let Some(c) = b.get(k) {
                    push(&mut stream, u32::from(*c), 8);
                }
            }
        }
        for k in 0..ecpb {
            for e in &ecs {
                push(&mut stream, u32::from(e[k]), 8);
            }
        }
        let (h, w) = RMQR_SIZES[i];
        let fp = v.buildFunctionPattern().map_err(|e| e.to_string())?;
        debug_assert_eq!((fp.width() as usize, fp.height() as usize), (w, h));
        let function = |x: usize, y: usize| fp.get(x as u32, y as u32);
        let mut g = new_grid(w, h);
        place(&mut g, &function, w - 2, &stream);
        // Data mask 4 is the only one rMQR uses.
        for (y, row) in g.iter_mut().enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                if !function(x, y) && (y / 2 + x / 3) % 2 == 0 {
                    *cell = !*cell;
                }
            }
        }
        rmqr_patterns(&mut g, v.getAlignmentPatternCenters());
        let f = bch18((u32::from(level == "H") << 5) | (n - 1));
        let (left, right) = (f ^ 0x1FAB2, f ^ 0x20A7B);
        let mut lc: Vec<(usize, usize)> = (1..=3).rev().map(|y| (11, y)).collect();
        for x in [10, 9, 8] {
            lc.extend((1..=5).rev().map(|y| (x, y)));
        }
        let mut rc: Vec<(usize, usize)> = (3..=5).map(|x| (w - x, h - 6)).collect();
        for x in 6..=8 {
            rc.extend((2..=6).map(|y| (w - x, h - y)));
        }
        for (k, (x, y)) in lc.into_iter().enumerate() {
            g[y][x] = (left >> (17 - k)) & 1 == 1;
        }
        for (k, (x, y)) in rc.into_iter().enumerate() {
            g[y][x] = (right >> (17 - k)) & 1 == 1;
        }
        return Ok(g);
    }
    Err(format!("{data:?} is too long for rMQR at error correction {level} (R17x139 holds up to 361 digits or 150 bytes at M)"))
}

/// rMQR's fixed patterns (Zint `rmqr_setup_grid`): edge timing, finder, finder sub-pattern, corner patterns,
/// separators, alignment patterns with their vertical timing lines.
fn rmqr_patterns(g: &mut Grid, alignment: &[u32]) {
    let (h, w) = (g.len(), g[0].len());
    for x in 0..w {
        g[0][x] = x % 2 == 0;
        g[h - 1][x] = x % 2 == 0;
    }
    for y in 0..h {
        g[y][0] = y % 2 == 0;
        g[y][w - 1] = y % 2 == 0;
    }
    finder(g, 0, 0);
    const SUB: [u8; 5] = [0x1F, 0x11, 0x15, 0x11, 0x1F];
    for (i, row) in SUB.iter().enumerate() {
        for j in 0..5 {
            g[h - 5 + i][w - 5 + j] = row & (0x10 >> j) != 0;
        }
    }
    // Corner finder patterns: bottom left and top right.
    g[h - 2][0] = true;
    g[h - 2][1] = false;
    g[h - 1][1] = true;
    g[0][w - 2] = true;
    g[1][w - 2] = false;
    g[1][w - 1] = true;
    for row in g.iter_mut().take(7) {
        row[7] = false;
    }
    if h > 7 {
        for x in 0..8 {
            g[7][x] = false;
        }
    }
    for p in alignment.iter().map(|p| *p as usize) {
        for (y, row) in g.iter_mut().enumerate() {
            row[p] = y % 2 == 0;
        }
        for y in [1, 2, h - 3, h - 2] {
            g[y][p - 1] = true;
            g[y][p + 1] = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rxing::common::BitMatrix;

    fn decode(g: &Grid) -> Result<String, String> {
        let (h, w) = (g.len(), g[0].len());
        let mut m = BitMatrix::new(w as u32, h as u32).unwrap();
        for (y, row) in g.iter().enumerate() {
            for (x, d) in row.iter().enumerate() {
                if *d {
                    m.set(x as u32, y as u32);
                }
            }
        }
        let r = rxing::qrcode::cpp_port::decoder::Decode(&m).map_err(|e| e.to_string())?;
        Ok(r.content().to_string())
    }

    #[test]
    fn rs_matches_a_known_qr_block() {
        // ISO 18004 Annex I: "01234567" at 1-M, data codewords and their 10 error correction codewords.
        let data = [0x10, 0x20, 0x0C, 0x56, 0x61, 0x80, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11];
        assert_eq!(rs_ec(&data, 10), [0xA5, 0x24, 0xD4, 0xC1, 0xED, 0x36, 0xC7, 0x87, 0x2C, 0x55]);
        assert_eq!(bch15(0b01000) ^ 0x5412, 0x77C4, "QR format information for M, mask 0");
        assert_eq!(bch18(1) ^ 0x1FAB2, 0x1E597);
        assert_eq!(bch18(1) ^ 0x20A7B, 0x2155E);
    }

    #[test]
    fn micro_qr_round_trips_through_a_reader() {
        for (data, ec, size) in [
            ("123", None, 11),
            ("12345", None, 13),
            ("01234567", None, 13),
            ("HELLO 1", None, 15),
            ("OpenEtch", None, 15),
            ("OPENETCH-2024", Some("M"), 17),
            ("123456789012345678901234567890", None, 17),
            ("Grüße", None, 15),
            ("ABC", Some("Q"), 17),
        ] {
            let g = micro_qr(data, ec).unwrap_or_else(|e| panic!("{data}: {e}"));
            assert_eq!((g.len(), g[0].len()), (size, size), "{data}");
            assert_eq!(decode(&g).unwrap_or_else(|e| panic!("{data}: {e}")), data);
            // Finder in the top-left corner, timing along the top.
            assert!(g[0][..7].iter().all(|d| *d) && !g[0][7] && g[0][8] && !g[0][9]);
        }
        assert!(micro_qr(&"9".repeat(36), None).is_err());
        assert!(micro_qr("x", Some("H")).is_err());
    }

    #[test]
    fn rmqr_round_trips_through_a_reader() {
        for (data, ec) in [
            ("123", None),
            ("RMQR TEST", None),
            ("https://openetch.example/rmqr", None),
            ("OpenEtch rectangular micro QR", Some("H")),
            (&"7".repeat(300)[..], None),
        ] {
            let g = rmqr(data, ec).unwrap_or_else(|e| panic!("{data}: {e}"));
            assert!(RMQR_SIZES.contains(&(g.len(), g[0].len())));
            assert_eq!(decode(&g).unwrap_or_else(|e| panic!("{data} ({}x{}): {e}", g.len(), g[0].len())), data);
        }
        // Every size: fill it to the brim and read it back.
        for n in 1..=32u32 {
            let v = Version::rMQR(n).unwrap();
            let dc: usize = v
                .getECBlocksForLevel(ErrorCorrectionLevel::M)
                .getECBlocks()
                .iter()
                .map(|b| (b.getCount() * b.getDataCodewords()) as usize)
                .sum();
            let digits = (dc * 8 - 3 - Mode::NUMERIC.CharacterCountBits(v) as usize) * 3 / 10;
            let data = "1".repeat(digits.min((1 << Mode::NUMERIC.CharacterCountBits(v)) - 1));
            let g = rmqr(&data, None).unwrap();
            assert_eq!(decode(&g).unwrap(), data, "version {n}");
        }
        assert!(rmqr(&"x".repeat(200), None).is_err());
        assert!(rmqr("x", Some("L")).is_err());
    }
}
