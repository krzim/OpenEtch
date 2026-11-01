//! Han Xin Code (Chinese Sensible Code, ISO/IEC 20830:2021): versions 1-84 (23 x 23 to 189 x 189 modules), error
//! correction levels 1-4. Ported from Zint's `hanxin.c` (BSD-3-Clause, Copyright (C) 2009-2026 Robin Stuart; its
//! mode optimisation adapted from Project Nayuki, MIT), so OpenEtch draws the same symbols as Zint, including its
//! mask choice. See ATTRIBUTION.md.
//!
//! The data is ISO/IEC 8859-1 when every character fits, else GB 18030 (the Chinese modes: Region One / Two,
//! 2-byte and 4-byte), as Zint does without an ECI. The modes (numeric, text, binary and the Chinese ones) are chosen
//! for the shortest bit stream. Without an explicit level the error correction is raised while the data still fits
//! the smallest version.

use rxing::common::CharacterSet;

use super::Grid;
use super::hanxin_tables as t;
use super::rs::Field;

const N: usize = 0; // numeric
const T: usize = 1; // text (sub-modes 1 and 2)
const B: usize = 2; // binary
const R1: usize = 3; // Common Chinese Region One
const R2: usize = 4; // Common Chinese Region Two
const D: usize = 5; // GB 18030 2-byte region
const F: usize = 6; // GB 18030 4-byte region
const MODES: usize = 7;

/// Whether `c` is in ISO/IEC 8859-1 as Zint takes it (without the C1 controls 0x80-0x9F).
pub fn latin1(c: char) -> bool {
    let u = u32::from(c);
    u < 0x80 || (0xA0..=0xFF).contains(&u)
}

/// The code units of `data`: ISO/IEC 8859-1 bytes when every character is one, else GB 18030 ([`gb18030_units`]).
pub fn units(data: &str) -> Vec<u32> {
    if data.chars().all(latin1) { data.chars().map(u32::from).collect() } else { gb18030_units(data) }
}

/// `data` in GB 18030: a 2-byte character is one unit `first << 8 | second`, a 4-byte one two such units, ASCII one
/// byte.
pub fn gb18030_units(data: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4];
    for c in data.chars() {
        let b = CharacterSet::GB18030.encode(c.encode_utf8(&mut buf)).unwrap_or_default();
        match b.len() {
            2 => out.push(u32::from(b[0]) << 8 | u32::from(b[1])),
            4 => out.extend([u32::from(b[0]) << 8 | u32::from(b[1]), u32::from(b[2]) << 8 | u32::from(b[3])]),
            _ => out.extend(b.iter().map(|x| u32::from(*x))),
        }
    }
    out
}

fn digit(u: u32) -> bool {
    (u32::from(b'0')..=u32::from(b'9')).contains(&u)
}
fn upper(u: u32) -> bool {
    (u32::from(b'A')..=u32::from(b'Z')).contains(&u)
}
fn lower(u: u32) -> bool {
    (u32::from(b'a')..=u32::from(b'z')).contains(&u)
}

/// Text sub-mode of a character: 1 for digits and letters, 2 otherwise.
fn submode(u: u32) -> u8 {
    if digit(u) || upper(u) || lower(u) { 1 } else { 2 }
}

/// Text 1 sub-mode value (Table 3).
fn text1(u: u32) -> Option<u32> {
    if digit(u) {
        Some(u - u32::from(b'0'))
    } else if upper(u) {
        Some(u - u32::from(b'A') + 10)
    } else if lower(u) {
        Some(u - u32::from(b'a') + 36)
    } else {
        None
    }
}

/// Text 2 sub-mode value (Table 4).
fn text2(u: u32) -> Option<u32> {
    match u {
        0..=27 => Some(u),
        0x20..=0x2F => Some(u - 0x20 + 28),
        0x3A..=0x40 => Some(u - 0x3A + 44),
        0x5B..=0x60 => Some(u - 0x5B + 51),
        0x7B..=0x7F => Some(u - 0x7B + 57),
        _ => None,
    }
}

fn region1(u: u32) -> bool {
    let (hi, lo) = (u >> 8, u & 0xFF);
    ((0xB0..=0xD7).contains(&hi) || (0xA1..=0xA3).contains(&hi)) && (0xA1..=0xFE).contains(&lo)
        || (0xA8A1..=0xA8C0).contains(&u)
}

fn region2(u: u32) -> bool {
    (0xD8..=0xF7).contains(&(u >> 8)) && (0xA1..=0xFE).contains(&(u & 0xFF))
}

fn double_byte(u: u32) -> bool {
    let lo = u & 0xFF;
    (0x81..=0xFE).contains(&(u >> 8)) && ((0x40..=0x7E).contains(&lo) || (0x80..=0xFE).contains(&lo))
}

fn four_byte(u: u32, v: u32) -> bool {
    (0x81..=0xFE).contains(&(u >> 8))
        && (0x30..=0x39).contains(&(u & 0xFF))
        && (0x81..=0xFE).contains(&(v >> 8))
        && (0x30..=0x39).contains(&(v & 0xFF))
}

/// The cheapest mode for every unit (Zint's `hx_define_modes`, costs in sixths of a bit).
fn define_modes(dd: &[u32]) -> Vec<usize> {
    const M: u32 = 6;
    let head = [4 * M, 4 * M, (4 + 13) * M, 4 * M, 4 * M, 4 * M, 0];
    let switch: [[u32; MODES]; MODES] = [
        [0, (10 + 4) * M, (10 + 4 + 13) * M, (10 + 4) * M, (10 + 4) * M, (10 + 4) * M, 10 * M],
        [(6 + 4) * M, 0, (6 + 4 + 13) * M, (6 + 4) * M, (6 + 4) * M, (6 + 4) * M, 6 * M],
        [4 * M, 4 * M, 0, 4 * M, 4 * M, 4 * M, 0],
        [(12 + 4) * M, (12 + 4) * M, (12 + 4 + 13) * M, 0, 12 * M, (12 + 4) * M, 12 * M],
        [(12 + 4) * M, (12 + 4) * M, (12 + 4 + 13) * M, 12 * M, 0, (12 + 4) * M, 12 * M],
        [(15 + 4) * M, (15 + 4) * M, (15 + 4 + 13) * M, (15 + 4) * M, (15 + 4) * M, 0, 15 * M],
        [4 * M, 4 * M, (4 + 13) * M, 4 * M, 4 * M, 4 * M, 0],
    ];
    let eod = [10 * M, 6 * M, 0, 12 * M, 12 * M, 15 * M, 0];
    let len = dd.len();
    let (mut numeric_end, mut numeric_cost, mut text_submode, mut fourbyte_end) = (0usize, 0u32, 1u8, 0usize);
    // modes[i][j]: 1 + the mode unit i is in when the encoding of units 0..=i ends in mode j (0: impossible).
    let mut modes = vec![[0u8; MODES]; len];
    let mut prev = head;
    for i in 0..len {
        let mut cur = [0u32; MODES];
        let u = dd[i];
        let (t1, t2);
        let in_numeric = i < numeric_end || {
            let n = (i..len.min(i + 3)).take_while(|&k| digit(dd[k])).count();
            numeric_end = if n == 0 { 0 } else { i + n };
            numeric_cost = [0, 60, 30, 20][n];
            n > 0
        };
        if in_numeric {
            cur[N] = prev[N] + numeric_cost;
            modes[i][N] = N as u8 + 1;
            (t1, t2) = (true, false);
        } else {
            (t1, t2) = (text1(u).is_some(), text2(u).is_some());
        }
        if t1 || t2 {
            if (text_submode == 1 && t2) || (text_submode == 2 && t1) {
                cur[T] = prev[T] + 72;
                text_submode = if t2 { 2 } else { 1 };
            } else {
                cur[T] = prev[T] + 36;
            }
            modes[i][T] = T as u8 + 1;
        } else {
            text_submode = 1;
        }
        cur[B] = prev[B] + if u > 0xFF { 96 } else { 48 };
        modes[i][B] = B as u8 + 1;
        let in_fourbyte = i < fourbyte_end || {
            let yes = i + 1 < len && four_byte(u, dd[i + 1]);
            fourbyte_end = if yes { i + 2 } else { 0 };
            yes
        };
        if in_fourbyte {
            cur[F] = prev[F] + 75;
            modes[i][F] = F as u8 + 1;
        } else if double_byte(u) {
            cur[D] = prev[D] + 90;
            modes[i][D] = D as u8 + 1;
            if region1(u) {
                cur[R1] = prev[R1] + 72;
                modes[i][R1] = R1 as u8 + 1;
            } else if region2(u) {
                cur[R2] = prev[R2] + 72;
                modes[i][R2] = R2 as u8 + 1;
            }
        }
        if i == len - 1 {
            for j in 0..MODES {
                if modes[i][j] != 0 {
                    cur[j] += eod[j];
                }
            }
        }
        for j in 0..MODES {
            for k in 0..MODES {
                if j != k && modes[i][k] != 0 {
                    let cost = cur[k] + switch[k][j];
                    if modes[i][j] == 0 || cost < cur[j] {
                        cur[j] = cost;
                        modes[i][j] = k as u8 + 1;
                    }
                }
            }
        }
        prev = cur;
    }
    let mut mode = (0..MODES).fold(0, |best, j| if prev[j] < prev[best] { j } else { best });
    let mut out = vec![0; len];
    for i in (0..len).rev() {
        mode = usize::from(modes[i][mode]) - 1;
        out[i] = mode;
    }
    out
}

fn push(bits: &mut Vec<bool>, value: u32, n: u32) {
    bits.extend((0..n).rev().map(|i| (value >> i) & 1 == 1));
}

/// The bit stream of `dd` in `modes`, after an ECI designator when `eci` is not 0 (Zint's `hx_calculate_binary`).
fn bitstream(dd: &[u32], modes: &[usize], eci: u32) -> Vec<bool> {
    let mut bits = Vec::new();
    if eci != 0 {
        push(&mut bits, 8, 4);
        match eci {
            0..=127 => push(&mut bits, eci, 8),
            128..=16383 => {
                push(&mut bits, 2, 2);
                push(&mut bits, eci, 14);
            }
            _ => {
                push(&mut bits, 6, 3);
                push(&mut bits, eci, 21);
            }
        }
    }
    let len = dd.len();
    let mut pos = 0;
    while pos < len {
        let mode = modes[pos];
        let mut n = 0;
        let mut doubles = 0;
        loop {
            if mode == B && dd[pos + n] > 0xFF {
                doubles += 1;
            }
            n += 1;
            if !(pos + n < len && modes[pos + n] == mode) {
                break;
            }
        }
        let block = &dd[pos..pos + n];
        match mode {
            N => {
                push(&mut bits, 1, 4);
                let (mut i, mut count) = (0, 0);
                while i < n {
                    count = (n - i).min(3);
                    let v = block[i..i + count].iter().fold(0, |v, d| v * 10 + d - u32::from(b'0'));
                    push(&mut bits, v, 10);
                    i += count;
                }
                push(&mut bits, 1020 + count as u32, 10);
            }
            T => {
                push(&mut bits, 2, 4);
                let mut sub = 1;
                for &u in block {
                    if submode(u) != sub {
                        push(&mut bits, 62, 6);
                        sub = submode(u);
                    }
                    let v = if sub == 1 { text1(u) } else { text2(u) };
                    push(&mut bits, v.unwrap_or(0), 6);
                }
                push(&mut bits, 63, 6);
            }
            B => {
                push(&mut bits, 3, 4);
                push(&mut bits, (n + doubles) as u32, 13);
                for &u in block {
                    push(&mut bits, u, if u > 0xFF { 16 } else { 8 });
                }
            }
            R1 | R2 => {
                let other = if mode == R1 { R2 } else { R1 };
                if pos == 0 || modes[pos - 1] != other {
                    push(&mut bits, if mode == R1 { 4 } else { 5 }, 4);
                }
                for &u in block {
                    let (hi, lo) = ((u >> 8) as i32, (u & 0xFF) as i32);
                    let glyph = if mode == R2 {
                        0x5E * (hi - 0xD8) + (lo - 0xA1)
                    } else if (0xA8A1..=0xA8C0).contains(&u) {
                        (lo - 0xA1) + 0xFCA
                    } else if (0xA1..=0xA3).contains(&hi) && (0xA1..=0xFE).contains(&lo) {
                        0x5E * (hi - 0xA1) + (lo - 0xA1) + 0xEB0
                    } else {
                        0x5E * (hi - 0xB0) + (lo - 0xA1)
                    };
                    push(&mut bits, glyph as u32, 12);
                }
                let next_other = pos + n < len && modes[pos + n] == other;
                push(&mut bits, if next_other { 4094 } else { 4095 }, 12);
            }
            D => {
                push(&mut bits, 6, 4);
                for &u in block {
                    let (hi, lo) = (u >> 8, u & 0xFF);
                    push(&mut bits, 0xBE * (hi - 0x81) + lo - if lo <= 0x7E { 0x40 } else { 0x41 }, 15);
                }
                push(&mut bits, 32767, 15);
            }
            _ => {
                for pair in block.chunks(2) {
                    push(&mut bits, 7, 4);
                    let (b1, b2) = (pair[0] >> 8, pair[0] & 0xFF);
                    let (b3, b4) = (pair[1] >> 8, pair[1] & 0xFF);
                    push(&mut bits, 0x3138 * (b1 - 0x81) + 0x04EC * (b2 - 0x30) + 0x0A * (b3 - 0x81) + (b4 - 0x30), 21);
                }
            }
        }
        pos += n;
    }
    bits
}

/// The (count, data codewords, check codewords) block batches of `version` at `level` (1-4).
fn blocks(version: usize, level: usize) -> [(usize, usize, usize); 3] {
    let at = (version - 1) * 36 + (level - 1) * 9;
    let b = &t::BLOCKS[at..at + 9];
    [0, 1, 2].map(|i| (usize::from(b[3 * i]), usize::from(b[3 * i + 1]), usize::from(b[3 * i + 2])))
}

fn data_codewords(version: usize, level: usize) -> usize {
    blocks(version, level).iter().map(|(n, k, _)| n * k).sum()
}

/// A Han Xin symbol and what was chosen for it.
pub struct Symbol {
    pub version: usize,
    pub level: usize,
    pub mask: usize,
    /// The data codewords (before error correction).
    pub data: Vec<u8>,
    pub grid: Grid,
}

/// Encode code units (see [`units`]) after an ECI designator (`eci` 0: none). `level` 1-4 is kept as given (`None`:
/// the highest that fits the smallest version), `version` 1-84 and `mask` 0-3 are chosen when `None`.
pub fn encode(
    dd: &[u32],
    eci: u32,
    level: Option<usize>,
    version: Option<usize>,
    mask: Option<usize>,
) -> Result<Symbol, String> {
    if dd.is_empty() {
        return Err("the Chinese Sensible Code data is empty".into());
    }
    let modes = define_modes(dd);
    let bits = bitstream(dd, &modes, eci);
    let codewords = bits.len().div_ceil(8);
    let mut level = level.unwrap_or(0);
    let explicit = level != 0;
    if !(1..=4).contains(&level) {
        level = 1;
    }
    let mut v = (1..=84).find(|v| data_codewords(*v, level) >= codewords).ok_or_else(|| {
        format!(
            "too much data for Chinese Sensible Code: {codewords} codewords (at most {})",
            data_codewords(84, level)
        )
    })?;
    if let Some(want) = version {
        if !(1..=84).contains(&want) || want < v {
            return Err(format!("Chinese Sensible Code version {want} cannot hold {codewords} codewords"));
        }
        v = want;
    }
    if !explicit {
        while level < 4 && codewords <= data_codewords(v, level + 1) {
            level += 1;
        }
    }
    let size = v * 2 + 21;
    let mut data = vec![0u8; data_codewords(v, level)];
    for (i, b) in bits.iter().enumerate() {
        if *b {
            data[i >> 3] |= 0x80 >> (i & 7);
        }
    }

    // Error correction: each block's data words followed by its check words.
    let field = Field::new(0x163);
    let mut full = Vec::new();
    let mut at = 0;
    for (count, k, ec) in blocks(v, level) {
        for _ in 0..count {
            let block: Vec<u8> = (at..at + k).map(|i| data.get(i).copied().unwrap_or(0)).collect();
            at += k;
            full.extend_from_slice(&block);
            full.extend(field.check(&block, ec));
        }
    }
    // Picket fence: every 13th codeword in turn (5.8.2).
    let picket: Vec<u8> = (0..13).flat_map(|s| full.iter().skip(s).step_by(13).copied()).collect::<Vec<_>>();

    let mut grid = setup_grid(size, v);
    let total_bits = picket.len() * 8;
    let mut j = 0;
    for cell in grid.iter_mut() {
        if j >= total_bits {
            break;
        }
        if *cell == 0 {
            if picket[j >> 3] & (0x80 >> (j & 7)) != 0 {
                *cell = 0x01;
            }
            j += 1;
        }
    }
    let mask = apply_mask(&mut grid, size, v, level, mask.filter(|m| *m < 4));
    let grid = (0..size).map(|y| (0..size).map(|x| grid[y * size + x] & 1 == 1).collect()).collect();
    Ok(Symbol { version: v, level, mask, data, grid })
}

/// Chinese Sensible Code of `data`; `ec` the error correction level 1-4 (default: the highest that fits).
pub fn han_xin(data: &str, ec: Option<&str>) -> Result<Grid, String> {
    let level = match ec.map(str::trim).filter(|e| !e.is_empty()) {
        None => None,
        Some(e) => match e.trim_start_matches(['L', 'l']).parse::<usize>() {
            Ok(l @ 1..=4) => Some(l),
            _ => return Err(format!("Chinese Sensible Code error correction is 1 to 4 (got {e:?})")),
        },
    };
    Ok(encode(&units(data), 0, level, None, None)?.grid)
}

// The grid while building: bit 0x10 marks function modules, bit 0x01 dark.
fn set(grid: &mut [u8], size: usize, x: i32, y: i32, value: u8) {
    if (0..size as i32).contains(&x) && (0..size as i32).contains(&y) {
        let k = y as usize * size + x as usize;
        if grid[k] == 0 {
            grid[k] = value;
        }
    }
}

fn finder(grid: &mut [u8], size: usize, x: usize, y: usize, rows: [u8; 7]) {
    for (yp, row) in rows.iter().enumerate() {
        for xp in 0..7 {
            grid[(y + yp) * size + x + xp] = if row & (0x40 >> xp) != 0 { 0x11 } else { 0x10 };
        }
    }
}

fn alignment(grid: &mut [u8], size: usize, x: i32, y: i32, w: i32, h: i32) {
    set(grid, size, x, y, 0x11);
    set(grid, size, x - 1, y + 1, 0x10);
    for i in 1..=w {
        set(grid, size, x - i, y, 0x11);
        set(grid, size, x - i - 1, y + 1, 0x10);
    }
    for i in 1..h {
        set(grid, size, x, y + i, 0x11);
        set(grid, size, x - 1, y + i + 1, 0x10);
    }
}

fn assistant(grid: &mut [u8], size: usize, x: i32, y: i32) {
    for dy in -1..=1 {
        for dx in -1..=1 {
            set(grid, size, x + dx, y + dy, if dx == 0 && dy == 0 { 0x11 } else { 0x10 });
        }
    }
}

/// Finder patterns, separators, the reserved function information and (version 4 up) the alignment patterns.
fn setup_grid(size: usize, version: usize) -> Vec<u8> {
    let mut g = vec![0u8; size * size];
    finder(&mut g, size, 0, 0, [0x7F, 0x40, 0x5F, 0x50, 0x57, 0x57, 0x57]);
    finder(&mut g, size, 0, size - 7, [0x7F, 0x01, 0x7D, 0x05, 0x75, 0x75, 0x75]);
    finder(&mut g, size, size - 7, 0, [0x7F, 0x01, 0x7D, 0x05, 0x75, 0x75, 0x75]);
    finder(&mut g, size, size - 7, size - 7, [0x75, 0x75, 0x75, 0x05, 0x7D, 0x01, 0x7F]);
    for (row, n) in [(7, 8), (8, 9)] {
        for i in 0..n {
            for (y, x) in [
                (row, i),
                (i, row),
                (row, size - i - 1),
                (size - i - 1, row),
                (i, size - row - 1),
                (size - row - 1, i),
                (size - row - 1, size - i - 1),
                (size - i - 1, size - row - 1),
            ] {
                g[y * size + x] = 0x10;
            }
        }
    }
    if version > 3 {
        let (k, r, m) = (
            i32::from(t::MODULE_K[version - 1]),
            i32::from(t::MODULE_R[version - 1]),
            i32::from(t::MODULE_M[version - 1]),
        );
        let s = size as i32;
        let step = |n: i32| if n < m { k } else { r - 1 };
        // Assistant patterns on the left and right edges.
        let (mut y, mut n) = (0, 0);
        while y < s {
            if n & 1 == 0 {
                if m & 1 == 1 {
                    assistant(&mut g, size, 0, y);
                }
            } else {
                if m & 1 == 0 {
                    assistant(&mut g, size, 0, y);
                }
                assistant(&mut g, size, s - 1, y);
            }
            y += step(n);
            n += 1;
        }
        // ... and on the top and bottom edges.
        let (mut x, mut n) = (s - 1, 0);
        while x >= 0 {
            if n & 1 == 0 {
                if m & 1 == 1 {
                    assistant(&mut g, size, x, s - 1);
                }
            } else {
                if m & 1 == 0 {
                    assistant(&mut g, size, x, s - 1);
                }
                assistant(&mut g, size, x, 0);
            }
            x -= step(n);
            n += 1;
        }
        // The alignment pattern: an L in every other block, alternating row by row.
        let (mut y, mut ny, mut column_switch) = (0, 0, true);
        while y < s {
            let h = step(ny);
            let mut row_switch = column_switch;
            column_switch = !column_switch;
            let (mut x, mut nx) = (s - 1, 0);
            while x >= 0 {
                let w = step(nx);
                if row_switch && !(y == 0 && x == s - 1) {
                    alignment(&mut g, size, x, y, w, h);
                }
                row_switch = !row_switch;
                x -= w;
                nx += 1;
            }
            y += h;
            ny += 1;
        }
    }
    g
}

/// Write the function information (version, level, mask and their check words) round the finder patterns.
fn function_info(g: &mut [u8], size: usize, version: usize, level: usize, mask: usize) {
    let mut bits = Vec::with_capacity(34);
    push(&mut bits, version as u32 + 20, 8);
    push(&mut bits, level as u32 - 1, 2);
    push(&mut bits, mask as u32, 2);
    let words: Vec<u8> = bits.chunks(4).map(|c| c.iter().fold(0, |v, b| v << 1 | u8::from(*b))).collect();
    for e in Field::new(0x13).check(&words, 4) {
        push(&mut bits, u32::from(e), 4);
    }
    bits.resize(34, false);
    for i in 0..9 {
        let at = |y: usize, x: usize| y * size + x;
        if bits[i] {
            g[at(8, i)] = 1;
            g[at(size - 9, size - i - 1)] = 1;
        }
        if bits[i + 8] {
            g[at(8 - i, 8)] = 1;
            g[at(size - 9 + i, size - 9)] = 1;
        }
        if bits[i + 17] {
            g[at(i, size - 9)] = 1;
            g[at(size - 1 - i, 8)] = 1;
        }
        if bits[i + 25] {
            g[at(8, size - 9 + i)] = 1;
            g[at(size - 9, 8 - i)] = 1;
        }
    }
}

/// Zint's mask penalty (`hx_evaluate`) of a 0/1 grid.
fn evaluate(l: &[u8], size: usize) -> u32 {
    let mut result = 0;
    let light_run = |get: &dyn Fn(i32) -> u8, from: i32, step: i32| {
        // Three light modules (or the edge) before (`step` -1) or after (+1) a finder-like pattern.
        let mut count = 0;
        for i in 0..3 {
            let p = from + step * i;
            if p < 0 || p >= size as i32 {
                return true;
            }
            if get(p) != 0 {
                break;
            }
            count += 1;
        }
        count == 3
    };
    for line in 0..size {
        for vertical in [true, false] {
            let get = |p: i32| if vertical { l[p as usize * size + line] } else { l[line * size + p as usize] };
            let mut p = 0i32;
            while p + 7 <= size as i32 {
                let found = if vertical {
                    get(p) != 0
                        && get(p + 1) != get(p + 5)
                        && get(p + 2) != 0
                        && get(p + 3) == 0
                        && get(p + 4) != 0
                        && get(p + 6) != 0
                } else {
                    let at = line * size + p as usize;
                    let w = &l[at..at + 7];
                    w == [1, 0, 1, 0, 1, 1, 1] || w == [1, 1, 1, 0, 1, 0, 1]
                };
                if found {
                    if light_run(&get, p - 1, -1) || light_run(&get, p + 7, 1) {
                        result += 50;
                    }
                    p += 1;
                }
                p += 1;
            }
            // Runs of 3 or more modules of one colour.
            let (mut block, mut state) = (0u32, 0u8);
            for p in 0..size as i32 {
                if get(p) == state {
                    block += 1;
                } else {
                    if block >= 3 {
                        result += block * 4;
                    }
                    block = 1;
                    state = get(p);
                }
            }
            if block >= 3 {
                result += block * 4;
            }
        }
    }
    result
}

/// Mask the data modules with the lowest-penalty pattern (or `user`), write the function information; the pattern.
fn apply_mask(grid: &mut [u8], size: usize, version: usize, level: usize, user: Option<usize>) -> usize {
    let mut masks = vec![0u8; grid.len()];
    for y in 0..size {
        for x in 0..size {
            let k = y * size + x;
            if grid[k] & 0xF0 == 0 {
                let (i, j) = (y + 1, x + 1);
                if (i + j) & 1 == 0 {
                    masks[k] |= 0x02;
                }
                if ((i + j) % 3 + j % 3) & 1 == 0 {
                    masks[k] |= 0x04;
                }
                if (i % j + j % i + i % 3 + j % 3) & 1 == 0 {
                    masks[k] |= 0x08;
                }
            }
        }
    }
    let best = user.unwrap_or_else(|| {
        let mut penalty = [0u32; 4];
        for (pattern, p) in penalty.iter_mut().enumerate() {
            let bit = if pattern == 0 { 0 } else { 1 << pattern };
            let mut local: Vec<u8> =
                grid.iter().zip(&masks).map(|(g, m)| if m & bit != 0 { g ^ 1 } else { g & 0x0F }).collect();
            function_info(&mut local, size, version, level, pattern);
            *p = evaluate(&local, size);
        }
        (1..4).fold(0, |best, p| if penalty[p] < penalty[best] { p } else { best })
    });
    if best != 0 {
        for (g, m) in grid.iter_mut().zip(&masks) {
            if m & (1 << best) != 0 {
                *g ^= 1;
            }
        }
    }
    function_info(grid, size, version, level, best);
    best
}
