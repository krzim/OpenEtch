//! Grid Matrix (AIMD014 Rev. 1.63 / GB/T 21049-2007): 13 versions (18 x 18 to 162 x 162 modules) of 6 x 6
//! macromodules, error correction levels 1-5. Ported from Zint's `gridmtx.c` and `gridmtx.h` (BSD-3-Clause, Copyright
//! (C) 2009-2026 Robin Stuart; its mode optimisation adapted from Project Nayuki, MIT), so OpenEtch draws the same
//! symbols as Zint, except where Zint's data would not read back (the tests read every bit stream back): the shifted
//! characters `[ \ ] ^ _ `` ` (`push_shift`), a numeral group cut short by CR LF (`in_numeral`) and a numeral group
//! left after its non-digit (`bitstream`). See ATTRIBUTION.md.
//!
//! The data is GB 2312 (Hanzi mode for Chinese characters, ASCII as itself) when every character is in it, as Zint
//! does by default; otherwise ISO/IEC 8859-1 after an ECI 3 designator, else UTF-8 after ECI 26. The modes (Hanzi,
//! numeral, lower and upper case, mixed, byte) are chosen for the shortest bit stream.

use rxing::common::CharacterSet;

use super::Grid;
use super::rs::Field;

// Encoding modes (1-based as in Zint; 0 is "none yet", 7 the end-of-data row of the switch tables).
const HANZI: usize = 1;
const NUMERAL: usize = 2;
const LOWER: usize = 3;
const UPPER: usize = 4;
const MIXED: usize = 5;
const BYTE: usize = 6;
const EOD: usize = 7;

/// Table 11: the codewords a version is recommended for, and the most it holds.
const RECOMMEND_CWS: [usize; 13] = [9, 30, 59, 114, 170, 237, 315, 405, 506, 618, 741, 875, 1021];
const MAX_CWS: [usize; 13] = [11, 40, 89, 146, 218, 305, 405, 521, 650, 794, 953, 1125, 1313];
/// Data codewords per version and level 1-5 (level 1 is not allowed in version 1).
const DATA_CWS: [[usize; 5]; 13] = [
    [0, 15, 13, 11, 9],
    [45, 40, 35, 30, 25],
    [89, 79, 69, 59, 49],
    [146, 130, 114, 98, 81],
    [218, 194, 170, 146, 121],
    [305, 271, 237, 203, 169],
    [405, 360, 315, 270, 225],
    [521, 463, 405, 347, 289],
    [650, 578, 506, 434, 361],
    [794, 706, 618, 530, 441],
    [953, 847, 741, 635, 529],
    [1125, 1000, 875, 750, 625],
    [1313, 1167, 1021, 875, 729],
];
/// Table A.1: block size N1, blocks of N1 (B1) and of N1 - 1 (B2) codewords per version.
const N1: [usize; 13] = [18, 50, 98, 81, 121, 113, 113, 116, 121, 126, 118, 125, 122];
const B1: [usize; 13] = [1, 1, 1, 2, 2, 2, 2, 3, 2, 7, 5, 10, 6];
const B2: [usize; 13] = [0, 0, 0, 0, 0, 1, 2, 2, 4, 0, 4, 0, 6];
/// Table A.1: check codewords E1 for the first B3 blocks, E2 for the rest, per version and level.
const E1B3E2: [[[usize; 3]; 5]; 13] = [
    [[0, 0, 0], [3, 1, 0], [5, 1, 0], [7, 1, 0], [9, 1, 0]],
    [[5, 1, 0], [10, 1, 0], [15, 1, 0], [20, 1, 0], [25, 1, 0]],
    [[9, 1, 0], [19, 1, 0], [29, 1, 0], [39, 1, 0], [49, 1, 0]],
    [[8, 2, 0], [16, 2, 0], [24, 2, 0], [32, 2, 0], [41, 1, 40]],
    [[12, 2, 0], [24, 2, 0], [36, 2, 0], [48, 2, 0], [61, 1, 60]],
    [[11, 3, 0], [23, 1, 22], [34, 2, 33], [45, 3, 0], [57, 1, 56]],
    [[12, 1, 11], [23, 2, 22], [34, 3, 33], [45, 4, 0], [57, 1, 56]],
    [[12, 2, 11], [23, 5, 0], [35, 3, 34], [47, 1, 46], [58, 4, 57]],
    [[12, 6, 0], [24, 6, 0], [36, 6, 0], [48, 6, 0], [61, 1, 60]],
    [[13, 4, 12], [26, 1, 25], [38, 5, 37], [51, 2, 50], [63, 7, 0]],
    [[12, 6, 11], [24, 4, 23], [36, 2, 35], [47, 9, 0], [59, 7, 58]],
    [[13, 5, 12], [25, 10, 0], [38, 5, 37], [50, 10, 0], [63, 5, 62]],
    [[13, 1, 12], [25, 3, 24], [37, 5, 36], [49, 7, 48], [61, 9, 60]],
];
/// Table 9: the codeword switching from mode `row` (0 none, 7 end of data) to mode `col + 1`, and its bit length.
const SWITCH: [[u32; 6]; 8] = [
    [1, 2, 3, 4, 5, 7],
    [0, 8161, 8162, 8163, 8164, 8165],
    [1019, 0, 1020, 1021, 1022, 1023],
    [28, 29, 0, 30, 124, 126],
    [28, 29, 30, 0, 124, 126],
    [1009, 1010, 1011, 1012, 0, 1015],
    [1, 2, 3, 4, 5, 0],
    [8160, 1018, 27, 27, 1008, 0],
];
const SWITCH_BITS: [[u32; 6]; 8] = [
    [4, 4, 4, 4, 4, 4],
    [0, 13, 13, 13, 13, 13],
    [10, 0, 10, 10, 10, 10],
    [5, 5, 0, 5, 7, 7],
    [5, 5, 5, 0, 7, 7],
    [10, 10, 10, 10, 0, 10],
    [4, 4, 4, 4, 4, 0],
    [13, 10, 5, 5, 10, 4],
];
/// The non-digits numeral mode takes (one per three digits), besides CR LF.
const NUMERAL_NONDIGITS: &[u8] = b" +-.,";

/// The code units of `data` and the ECI they need (0: none). GB 2312 when every character is in it (a Chinese
/// character is one unit `first << 8 | second`, ASCII one byte), else ISO/IEC 8859-1 (ECI 3), else UTF-8 (ECI 26).
pub fn units(data: &str) -> (Vec<u32>, u32) {
    let mut gb = Vec::new();
    let mut buf = [0u8; 4];
    for c in data.chars() {
        if c.is_ascii() {
            gb.push(u32::from(c));
            continue;
        }
        match CharacterSet::GB2312.encode(c.encode_utf8(&mut buf)).ok().as_deref() {
            Some(&[a, b]) if gb2312(a, b) => gb.push(u32::from(a) << 8 | u32::from(b)),
            _ => {
                return if data.chars().all(super::hanxin::latin1) {
                    (data.chars().map(u32::from).collect(), 3)
                } else {
                    (data.bytes().map(u32::from).collect(), 26)
                };
            }
        }
    }
    (gb, 0)
}

/// Whether `a b` is a character of GB 2312 itself (ISO-IR-58: 682 symbols in rows A1-A9, 6763 Hanzi in B0-F7).
/// rxing encodes "GB2312" as GBK, which also maps small Roman numerals, vertical forms, a few Latin letters and
/// private-use characters into free cells of rows A1-A9 and D7: those take the ECI route like any other character
/// outside GB 2312. (A1A4 and A1AA stay GBK's U+00B7 and U+2014, which GB 2312 readers built on GBK or GB 18030 give
/// back; Zint maps U+30FB and U+2015 there.)
fn gb2312(a: u8, b: u8) -> bool {
    let cells = |lo: u8, hi: u8| (lo..=hi).contains(&b);
    match a {
        0xA1 | 0xA3 => cells(0xA1, 0xFE),
        0xA2 => cells(0xB1, 0xE2) || cells(0xE5, 0xEE) || cells(0xF1, 0xFC),
        0xA4 => cells(0xA1, 0xF3),
        0xA5 => cells(0xA1, 0xF6),
        0xA6 => cells(0xA1, 0xB8) || cells(0xC1, 0xD8),
        0xA7 => cells(0xA1, 0xC1) || cells(0xD1, 0xF1),
        0xA8 => cells(0xA1, 0xBA) || cells(0xC5, 0xE9),
        0xA9 => cells(0xA4, 0xEF),
        0xD7 => cells(0xA1, 0xF9),
        0xB0..=0xF7 => cells(0xA1, 0xFE),
        _ => false,
    }
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
fn numeral_nondigit(u: u32) -> Option<usize> {
    NUMERAL_NONDIGITS.iter().position(|c| u32::from(*c) == u)
}
fn crlf(dd: &[u32], i: usize) -> bool {
    i + 1 < dd.len() && dd[i] == 13 && dd[i + 1] == 10
}

/// Whether unit `i` can start or continue numeral mode; sets where the current numeral group ends and its cost per
/// unit (Zint's `gm_in_numeral`).
fn in_numeral(dd: &[u32], i: usize, end: &mut usize, cost: &mut u32) -> bool {
    if i < *end {
        return true;
    }
    let (mut p, mut digits, mut nondigit, mut nondigit_at) = (i, 0, 0, 0);
    // A group is up to three digits and one non-digit, CR LF counting as one (Zint looks 4 units ahead, so "1\r\n2"
    // ended a group short of three digits, and the next group's CR LF left it read back as "1\r\n20").
    let mut window = i + 4;
    while p < dd.len() && p < window && digits < 3 {
        if digit(dd[p]) {
            digits += 1;
        } else if numeral_nondigit(dd[p]).is_some() {
            if nondigit != 0 {
                *end = 0;
                return false;
            }
            nondigit = 1;
            nondigit_at = p;
        } else if crlf(dd, p) {
            if nondigit != 0 {
                *end = 0;
                return false;
            }
            p += 1;
            window += 1;
            nondigit = 2;
            nondigit_at = p;
        } else {
            break;
        }
        p += 1;
    }
    if digits == 0 {
        *end = 0;
        return false;
    }
    if nondigit != 0 && nondigit_at == p - 1 {
        nondigit = 0;
    }
    *end = i + digits + nondigit;
    *cost = match (digits, nondigit) {
        (3, 2) => 24,
        (3, 1) => 30,
        (3, _) => 20,
        (2, 2) => 30,
        (2, 1) => 40,
        (2, _) => 30,
        (_, 2) => 40,
        _ => 60,
    };
    true
}

/// The cheapest mode (1-6) for every unit (Zint's `gm_define_modes`, costs in sixths of a bit).
fn define_modes(dd: &[u32]) -> Vec<usize> {
    const M: u32 = 6;
    let head = [4 * M, (4 + 2) * M, 4 * M, 4 * M, 4 * M, (4 + 9) * M];
    let switch: [[u32; 6]; 6] = [
        [0, (13 + 2) * M, 13 * M, 13 * M, 13 * M, (13 + 9) * M],
        [10 * M, 0, 10 * M, 10 * M, 10 * M, (10 + 9) * M],
        [5 * M, (5 + 2) * M, 0, 5 * M, 7 * M, (7 + 9) * M],
        [5 * M, (5 + 2) * M, 5 * M, 0, 7 * M, (7 + 9) * M],
        [10 * M, (10 + 2) * M, 10 * M, 10 * M, 0, (10 + 9) * M],
        [4 * M, (4 + 2) * M, 4 * M, 4 * M, 4 * M, 0],
    ];
    let eod = [13 * M, 10 * M, 5 * M, 5 * M, 10 * M, 4 * M];
    let (h, n, l, u, m, b) = (0, 1, 2, 3, 4, 5);
    let len = dd.len();
    let (mut numeral_end, mut numeral_cost, mut byte_count) = (0usize, 0u32, 0u32);
    let mut modes = vec![[0u8; 6]; len];
    let mut prev = head;
    for i in 0..len {
        let mut cur = [0u32; 6];
        let c = dd[i];
        let mut double_byte = c > 0xFF;
        let space = !double_byte && c == u32::from(b' ');
        let (mut is_digit, mut is_lower, mut is_upper, mut control, mut double_digit, mut eol) =
            (false, false, false, false, false, false);
        if !double_byte && !space {
            is_digit = digit(c);
            if !is_digit {
                is_lower = lower(c);
                is_upper = !is_lower && upper(c);
                if !is_lower && !is_upper {
                    control = c < 0x7F;
                    eol = crlf(dd, i);
                }
            } else if i + 1 < len {
                double_digit = digit(dd[i + 1]);
            }
        }
        cur[h] = prev[h] + if double_digit || eol { 39 } else { 78 };
        modes[i][h] = HANZI as u8;
        if byte_count == 512 || (double_byte && byte_count == 511) {
            cur[b] = head[b];
            if double_byte && byte_count == 511 {
                cur[b] += 48;
                double_byte = false;
            }
            byte_count = 0;
        }
        cur[b] += prev[b] + if double_byte { 96 } else { 48 };
        modes[i][b] = BYTE as u8;
        byte_count += if double_byte { 2 } else { 1 };
        if in_numeral(dd, i, &mut numeral_end, &mut numeral_cost) {
            cur[n] = prev[n] + numeral_cost;
            modes[i][n] = NUMERAL as u8;
        }
        if control {
            cur[l] = prev[l] + 78;
            modes[i][l] = LOWER as u8;
            cur[u] = prev[u] + 78;
            modes[i][u] = UPPER as u8;
            cur[m] = prev[m] + 96;
            modes[i][m] = MIXED as u8;
        } else {
            if is_lower || space {
                cur[l] = prev[l] + 30;
                modes[i][l] = LOWER as u8;
            }
            if is_upper || space {
                cur[u] = prev[u] + 30;
                modes[i][u] = UPPER as u8;
            }
            if is_digit || is_lower || is_upper || space {
                cur[m] = prev[m] + 36;
                modes[i][m] = MIXED as u8;
            }
        }
        if i + 1 == len {
            for j in 0..6 {
                if modes[i][j] != 0 {
                    cur[j] += eod[j];
                }
            }
        }
        for j in 0..6 {
            for k in 0..6 {
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
    let mut mode = (0..6).fold(0, |best, j| if prev[j] < prev[best] { j } else { best }) + 1;
    let mut out = vec![0; len];
    for i in (0..len).rev() {
        mode = usize::from(modes[i][mode - 1]);
        out[i] = mode;
    }
    out
}

fn push(bits: &mut Vec<bool>, value: u32, n: u32) {
    bits.extend((0..n).rev().map(|i| (value >> i) & 1 == 1));
}

fn put(bits: &mut [bool], at: usize, value: u32, n: u32) {
    for i in 0..n as usize {
        bits[at + i] = (value >> (n as usize - 1 - i)) & 1 == 1;
    }
}

/// A control or other character after a shift (Table 7): the 32 controls, then the 32 ASCII punctuation characters
/// in order (`!` 32 .. `/` 46, `:` 47 .. `@` 53, `[` 54 .. `` ` `` 59, `{` 60 .. `~` 63). Only these are ever
/// shifted (letters, digits and space have their own values in every text mode). Zint's `gm_add_shift_char` takes
/// `[ \ ] ^ _ `` ` as c - 46, which gives them the values of `. / : ; < =`; here they keep their own.
fn push_shift(bits: &mut Vec<bool>, c: u32) {
    let glyph = match c {
        0..=0x1F => c,
        0x21..=0x2F => c - 0x21 + 32,
        0x3A..=0x40 => c - 0x3A + 47,
        0x5B..=0x60 => c - 0x5B + 54,
        0x7B..=0x7E => c - 0x7B + 60,
        // Never shifted (see `define_modes`).
        _ => 0,
    };
    push(bits, glyph, 6);
}

const MAX_BITS: usize = 9191;

/// The bit stream of `dd` after an ECI designator when `eci` is not 0 (Zint's `gm_encode`).
fn bitstream(dd: &[u32], eci: u32, bits: &mut Vec<bool>) -> Result<(), String> {
    let too_long = || "too much data for Grid Matrix (at most 1313 codewords)".to_string();
    if eci != 0 {
        push(bits, 12, 4);
        match eci {
            0..=1023 => push(bits, eci, 11),
            1024..=32767 => {
                push(bits, 2, 2);
                push(bits, eci, 15);
            }
            _ => {
                push(bits, 3, 2);
                push(bits, eci, 20);
            }
        }
    }
    let modes = define_modes(dd);
    let len = dd.len();
    let (mut sp, mut current, mut numeral_cnt, mut numeral_pad_at, mut byte_count_at, mut byte_count) =
        (0, 0, 0, 0, 0, 0u32);
    while sp < len {
        let next = modes[sp];
        if next != current {
            if current == BYTE {
                put(bits, byte_count_at, byte_count - 1, 9);
                byte_count = 0;
            } else if current == NUMERAL && numeral_cnt != 0 {
                put(bits, numeral_pad_at, 3 - numeral_cnt, 2);
            }
            push(bits, SWITCH[current][next - 1], SWITCH_BITS[current][next - 1]);
            if bits.len() > MAX_BITS {
                return Err(too_long());
            }
        }
        let last = current;
        current = next;
        let c = dd[sp];
        match current {
            HANZI => {
                let glyph = if c > 0xFF {
                    let (c1, c2) = (c >> 8, c & 0xFF);
                    if (0xA1..=0xA9).contains(&c1) {
                        0x60 * (c1 - 0xA1) + (c2 - 0xA0)
                    } else {
                        0x60 * (c1 - 0xB0 + 9) + (c2 - 0xA0)
                    }
                } else if crlf(dd, sp) {
                    sp += 1;
                    7776
                } else if sp + 1 < len && digit(c) && digit(dd[sp + 1]) {
                    sp += 1;
                    8033 + 10 * (c - u32::from(b'0')) + (dd[sp] - u32::from(b'0'))
                } else {
                    7777 + c
                };
                push(bits, glyph, 13);
                sp += 1;
            }
            NUMERAL => {
                if last != current {
                    numeral_pad_at = bits.len();
                    push(bits, 0, 2);
                }
                numeral_cnt = 0;
                let mut nondigit_at: Option<u32> = None;
                let mut punt = 0;
                let mut num = [0u32; 3];
                loop {
                    let c = dd[sp];
                    if digit(c) {
                        num[numeral_cnt as usize] = c - u32::from(b'0');
                        numeral_cnt += 1;
                    } else if numeral_nondigit(c).is_some() {
                        if nondigit_at.is_some() {
                            break;
                        }
                        punt = c;
                        nondigit_at = Some(numeral_cnt);
                    } else if crlf(dd, sp) {
                        if nondigit_at.is_some() {
                            break;
                        }
                        punt = c;
                        sp += 1;
                        nondigit_at = Some(numeral_cnt);
                    } else {
                        break;
                    }
                    sp += 1;
                    // A group needs a digit. The modes are chosen unit by unit, so they can leave numeral mode right
                    // after a non-digit (in Zint too, "111.2AAb" read back as "111.0002AAb"); the digit that always
                    // follows it then still joins the group.
                    let more = sp < len && (modes[sp] == NUMERAL || (numeral_cnt == 0 && digit(dd[sp])));
                    if !(numeral_cnt < 3 && more) {
                        break;
                    }
                }
                if let Some(at) = nondigit_at {
                    let base = if punt == 13 { 15 } else { numeral_nondigit(punt).unwrap_or(0) as u32 * 3 };
                    push(bits, 1000 + base + at, 10);
                }
                push(bits, 100 * num[0] + 10 * num[1] + num[2], 10);
            }
            BYTE => {
                if last != current {
                    byte_count_at = bits.len();
                    push(bits, 0, 9);
                }
                let mut glyph = c;
                if byte_count == 512 || (glyph > 0xFF && byte_count == 511) {
                    if glyph > 0xFF && byte_count == 511 {
                        push(bits, glyph >> 8, 8);
                        glyph &= 0xFF;
                        byte_count += 1;
                    }
                    put(bits, byte_count_at, byte_count - 1, 9);
                    push(bits, 7, 4);
                    byte_count_at = bits.len();
                    push(bits, 0, 9);
                    byte_count = 0;
                }
                push(bits, glyph, if glyph > 0xFF { 16 } else { 8 });
                sp += 1;
                byte_count += if glyph > 0xFF { 2 } else { 1 };
            }
            MIXED => {
                if c == u32::from(b' ') || digit(c) || upper(c) || lower(c) {
                    let glyph = if c == u32::from(b' ') {
                        62
                    } else if digit(c) {
                        c - u32::from(b'0')
                    } else if upper(c) {
                        c - u32::from(b'A') + 10
                    } else {
                        c - u32::from(b'a') + 36
                    };
                    push(bits, glyph, 6);
                } else {
                    push(bits, 1014, 10);
                    push_shift(bits, c);
                }
                sp += 1;
            }
            _ => {
                // Upper or lower case.
                let in_set = if current == UPPER { upper(c) } else { lower(c) };
                if c == u32::from(b' ') {
                    push(bits, 26, 5);
                } else if in_set {
                    push(bits, c - u32::from(if current == UPPER { b'A' } else { b'a' }), 5);
                } else {
                    push(bits, 125, 7);
                    push_shift(bits, c);
                }
                sp += 1;
            }
        }
        if bits.len() > MAX_BITS {
            return Err(too_long());
        }
    }
    if current == BYTE {
        put(bits, byte_count_at, byte_count - 1, 9);
    } else if current == NUMERAL && numeral_cnt != 0 {
        put(bits, numeral_pad_at, 3 - numeral_cnt, 2);
    }
    push(bits, SWITCH[EOD][current - 1], SWITCH_BITS[EOD][current - 1]);
    if bits.len() > MAX_BITS {
        return Err(too_long());
    }
    Ok(())
}

/// A Grid Matrix symbol and what was chosen for it.
pub struct Symbol {
    pub version: usize,
    pub level: usize,
    /// The codewords in symbol order (data and check words interleaved by block).
    pub codewords: Vec<u8>,
    /// How many of the codewords the data took (before padding).
    pub data_codewords: usize,
    pub grid: Grid,
}

/// Encode code units (see [`units`]) after an ECI designator (`eci` 0: none). `level` 1-5 (raised to the version's
/// minimum, lowered when the data needs room) and `version` 1-13 are chosen when `None`.
pub fn encode(dd: &[u32], eci: u32, level: Option<usize>, version: Option<usize>) -> Result<Symbol, String> {
    if dd.is_empty() {
        return Err("the Grid Matrix data is empty".into());
    }
    let mut bits = Vec::new();
    bitstream(dd, eci, &mut bits)?;
    let pad = (7 - bits.len() % 7) % 7;
    bits.extend(std::iter::repeat_n(false, pad));
    let n = bits.len() / 7;
    let auto_layers = (1..=12).find(|i| RECOMMEND_CWS[i - 1] >= n).unwrap_or(13);
    let min_layers = (1..=12).find(|i| MAX_CWS[i - 1] >= n).unwrap_or(13);
    let layers = match version {
        Some(v) if (1..=13).contains(&v) && v >= min_layers => v,
        Some(v) => return Err(format!("Grid Matrix version {v} cannot hold {n} codewords")),
        None => auto_layers,
    };
    let mut ecc = match layers {
        1 => 5,
        2 | 3 => 4,
        _ => 3,
    };
    let min_rec = match layers {
        1 => 4,
        2 => 2,
        _ => 1,
    };
    if let Some(l @ 1..=5) = level {
        ecc = l.max(min_rec);
    }
    if n > DATA_CWS[layers - 1][ecc - 1] {
        let min_ecc = if layers > 1 { 1 } else { 2 };
        loop {
            ecc -= 1;
            if !(n > DATA_CWS[layers - 1][ecc - 1] && ecc > min_ecc) {
                break;
            }
        }
        if n > DATA_CWS[layers - 1][ecc - 1] {
            return Err(format!("too much data for Grid Matrix: {n} codewords"));
        }
    }

    // Data codewords, padding, and the blocks' check words interleaved.
    let mut data = vec![0u8; 1313];
    for (i, w) in bits.chunks(7).enumerate() {
        data[i] = w.iter().fold(0, |v, b| v << 1 | u8::from(*b));
    }
    let total_data = DATA_CWS[layers - 1][ecc - 1];
    for (i, d) in data.iter_mut().enumerate().take(total_data).skip(n + 1) {
        if i & 1 == 1 {
            *d = 0x7E;
        }
    }
    let (n1, b1, b2) = (N1[layers - 1], B1[layers - 1], B2[layers - 1]);
    let [e1, b3, e2] = E1B3E2[layers - 1][ecc - 1];
    let blocks = b1 + b2;
    let field = Field::new(0x89);
    let mut cws = vec![0u8; b1 * n1 + b2 * (n1 - 1)];
    let mut wp = 0;
    for i in 0..blocks {
        let size = if i < b1 { n1 } else { n1 - 1 };
        let ecc_size = if i < b3 { e1 } else { e2 };
        let mut block = data[wp..wp + size - ecc_size].to_vec();
        wp += size - ecc_size;
        block.extend(field.check(&block, ecc_size));
        for (j, w) in block.iter().enumerate() {
            cws[blocks * j + i] = *w;
        }
    }

    let size = 6 + layers * 12;
    let per_dim = 1 + layers * 2;
    let mut g = vec![false; size * size];
    for y in 0..per_dim {
        for x in 0..per_dim {
            let at = macromodule(per_dim, x, y);
            let cw = u32::from(cws[at + 1]) << 7 | u32::from(cws[at]);
            let (i, j) = (x * 6 + 1, (y * 6 + 1) * size);
            if cw & 0x2000 != 0 {
                g[j + i + 2] = true;
            }
            if cw & 0x1000 != 0 {
                g[j + i + 3] = true;
            }
            for r in 1..4 {
                for c in 0..4 {
                    if (cw >> (15 - r * 4 - c)) & 1 == 1 {
                        g[j + r * size + i + c] = true;
                    }
                }
            }
        }
    }
    // The layer id in each macromodule's top-left corner, ring by ring from the centre.
    let layer_ids: Vec<usize> = (0..=layers).map(|i| if ecc == 1 { 3 - (i & 3) } else { (i + 5 - ecc) & 3 }).collect();
    let mut ids = vec![0usize; per_dim * per_dim];
    let (mut start, mut stop) = (per_dim / 2, per_dim / 2);
    for id in &layer_ids {
        for i in start..=stop {
            ids[start * per_dim + i] = *id;
            ids[i * per_dim + start] = *id;
            ids[(per_dim - start - 1) * per_dim + i] = *id;
            ids[i * per_dim + (per_dim - start - 1)] = *id;
        }
        start = start.wrapping_sub(1);
        stop += 1;
    }
    for i in 0..per_dim {
        for j in 0..per_dim {
            let id = ids[i * per_dim + j];
            let at = (i * 6 + 1) * size + j * 6 + 1;
            g[at] |= id & 2 != 0;
            g[at + 1] |= id & 1 != 0;
        }
    }
    // Macromodule frames: dark round every other macromodule.
    for x in 0..per_dim {
        let mut dark = x & 1 == 0;
        for y in 0..per_dim {
            if dark {
                let (xo, yo) = (x * 6, y * 6 * size);
                for i in 0..5 {
                    g[yo + xo + i] = true;
                    g[yo + 5 * size + xo + i] = true;
                    g[yo + i * size + xo] = true;
                    g[yo + i * size + xo + 5] = true;
                }
                g[yo + 5 * size + xo + 5] = true;
            }
            dark = !dark;
        }
    }
    let grid = (0..size).map(|y| g[y * size..(y + 1) * size].to_vec()).collect();
    Ok(Symbol { version: layers, level: ecc, codewords: cws, data_codewords: n, grid })
}

/// The index of the first codeword of macromodule (x, y): rings from the outside in, two codewords each.
fn macromodule(per_dim: usize, x: usize, y: usize) -> usize {
    let (x, y, d) = (x as i64, y as i64, per_dim as i64);
    let r = if x < y { x.min(d - y - 1) } else { y.min(d - x - 1) };
    let m = (((d - 1) >> 1) - r) * 2 + 1;
    let b = m * m - 1;
    let idx = if x == r {
        b - y + r
    } else if y == r {
        b - (m - 1) * 4 + (x - r)
    } else if x == d - r - 1 {
        b - (m - 1) * 3 + (y - r)
    } else {
        b - (m - 1) - (x - r)
    };
    (idx as usize) << 1
}

/// Grid Matrix of `data`; `ec` the error correction level 1-5 (default: the version's recommended level).
pub fn grid_matrix(data: &str, ec: Option<&str>) -> Result<Grid, String> {
    let level = match ec.map(str::trim).filter(|e| !e.is_empty()) {
        None => None,
        Some(e) => match e.trim_start_matches(['L', 'l']).parse::<usize>() {
            Ok(l @ 1..=5) => Some(l),
            _ => return Err(format!("Grid Matrix error correction is 1 to 5 (got {e:?})")),
        },
    };
    let (dd, eci) = units(data);
    Ok(encode(&dd, eci, level, None)?.grid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table 7 in ASCII order (the inverse of `push_shift`).
    fn unshift(g: u32) -> u8 {
        (match g {
            0..=31 => g,
            32..=46 => g - 32 + 0x21,
            47..=53 => g - 47 + 0x3A,
            54..=59 => g - 54 + 0x5B,
            _ => g - 60 + 0x7B,
        }) as u8
    }

    /// Read a bit stream back: the bytes (a Hanzi unit as its two GB 2312 bytes) and the ECI. Written from the mode
    /// tables, apart from the encoder, to check what it writes.
    fn read(bits: &[bool]) -> (Vec<u8>, u32) {
        let mut p = 0;
        let mut get = |n: usize| {
            let v = bits[p..p + n].iter().fold(0, |v, b| v << 1 | u32::from(*b));
            p += n;
            v
        };
        let (mut out, mut eci, mut mode, mut first) = (Vec::new(), 0, 0, true);
        loop {
            match mode {
                0 => {
                    mode = match get(4) {
                        0 => return (out, eci),
                        12 if first => {
                            eci = if get(1) == 0 {
                                get(10)
                            } else if get(1) == 0 {
                                get(15)
                            } else {
                                get(20)
                            };
                            0
                        }
                        7 => BYTE,
                        m @ 1..=5 => m as usize,
                        m => panic!("mode {m}"),
                    };
                    first = false;
                }
                HANZI => match get(13) {
                    v @ 0..=7775 => {
                        let row = v / 96;
                        let c1 = if row < 9 { 0xA1 + row } else { 0xB0 + row - 9 };
                        out.extend([c1 as u8, (0xA0 + v % 96) as u8]);
                    }
                    7776 => out.extend(b"\r\n"),
                    v @ 7777..=8032 => out.push((v - 7777) as u8),
                    v @ 8033..=8132 => out.extend(format!("{:02}", v - 8033).bytes()),
                    8160 => return (out, eci),
                    v @ 8161..=8165 => mode = (v - 8161 + 2) as usize,
                    v => panic!("hanzi {v}"),
                },
                NUMERAL => {
                    let pad = get(2);
                    let mut group = out.len();
                    loop {
                        let v = get(10);
                        if v >= 1018 {
                            // Drop the padding digits of the last group.
                            for _ in 0..pad {
                                let i = out[group..].iter().rposition(u8::is_ascii_digit).unwrap() + group;
                                out.remove(i);
                            }
                            if v == 1018 {
                                return (out, eci);
                            }
                            mode = [HANZI, LOWER, UPPER, MIXED, BYTE][v as usize - 1019];
                            break;
                        }
                        let (punct, value) = if v >= 1000 { (Some(v - 1000), get(10)) } else { (None, v) };
                        group = out.len();
                        for (i, d) in format!("{value:03}").bytes().enumerate() {
                            if let Some(q) = punct.filter(|q| q % 3 == i as u32) {
                                let k = (q / 3) as usize;
                                out.extend(if k == 5 { &b"\r\n"[..] } else { &b" +-.,"[k..=k] });
                            }
                            out.push(d);
                        }
                    }
                }
                LOWER | UPPER => {
                    let base = if mode == LOWER { b'a' } else { b'A' };
                    match get(5) {
                        v @ 0..=25 => out.push(base + v as u8),
                        26 => out.push(b' '),
                        27 => return (out, eci),
                        28 => mode = HANZI,
                        29 => mode = NUMERAL,
                        30 => mode = if mode == LOWER { UPPER } else { LOWER },
                        v => match v << 2 | get(2) {
                            124 => mode = MIXED,
                            125 => out.push(unshift(get(6))),
                            126 => mode = BYTE,
                            x => panic!("case code {x}"),
                        },
                    }
                }
                MIXED => match get(6) {
                    v @ 0..=9 => out.push(b'0' + v as u8),
                    v @ 10..=35 => out.push(b'A' + (v - 10) as u8),
                    v @ 36..=61 => out.push(b'a' + (v - 36) as u8),
                    62 => out.push(b' '),
                    v => match v << 4 | get(4) {
                        1008 => return (out, eci),
                        x @ 1009..=1012 => mode = [HANZI, NUMERAL, LOWER, UPPER][x as usize - 1009],
                        1014 => out.push(unshift(get(6))),
                        1015 => mode = BYTE,
                        x => panic!("mixed code {x}"),
                    },
                },
                _ => {
                    let n = get(9) + 1;
                    out.extend((0..n).map(|_| get(8) as u8));
                    mode = 0;
                }
            }
        }
    }

    fn round_trip(s: &str) {
        let (dd, eci) = units(s);
        let mut bits = Vec::new();
        bitstream(&dd, eci, &mut bits).unwrap();
        let want: Vec<u8> =
            dd.iter().flat_map(|u| if *u > 0xFF { vec![(u >> 8) as u8, *u as u8] } else { vec![*u as u8] }).collect();
        let (got, got_eci) = read(&bits);
        assert_eq!((&got, got_eci), (&want, eci), "{s:?} reads back as {:?}", String::from_utf8_lossy(&got));
    }

    #[test]
    fn data_reads_back() {
        // Zint (2.16) gives `[ \ ] ^ _ `` ` after a shift the values of `. / : ; < =`, can end a numeral group short
        // of three digits before the next one ("1\r\n2\r\n3" read back as "1\r\n20\r\n3") and can leave numeral mode
        // right after a non-digit ("111.2AAb" read back as "111.0002AAb").
        for s in [
            "A[B\\C]D^E_F`G{H|I}J~K",
            "a[b\\c]d^e_f`g!h",
            "WQBHJNBWFGT]<+",
            "1\r\n2\r\n3",
            "\r\n62\r\n56802730951",
            "111.2AAb",
            "12.2.12A.bAbA2",
            "212.1b12",
            "1.2.3.4,5 6+7-8\r\n9",
            "12.",
            ".1",
            "1\r\n",
            "电池充电器 Ω 12.5V",
            "Größe ÿ",
            "Ωmega € 😀",
        ] {
            round_trip(s);
        }
        // Random mixes of digits, the numeral punctuation, CR LF, letters, shifted characters and Chinese.
        let pieces = ["1", "2", "9", ".", ",", " ", "+", "-", "\r\n", "A", "b", "[", "_", "\u{1}", "汉", "é", "12345"];
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let n = 1 + x % 14;
            let s: String = (0..n).map(|i| pieces[((x >> (4 * i)) % pieces.len() as u64) as usize]).collect();
            round_trip(&s);
        }
    }

    #[test]
    fn gb2312_is_gb2312() {
        let cells = (0xA1..=0xFE).flat_map(|a| (0xA1..=0xFE).map(move |b| (a, b))).filter(|(a, b)| gb2312(*a, *b));
        assert_eq!(cells.count(), 682 + 6763);
        assert_eq!(units("电池·").1, 0);
        // GBK's additions in GB 2312's rows (small Roman numerals, vertical forms, Latin letters) take the ECI.
        assert_eq!(units("ⅰ电池").1, 26);
        assert_eq!(units("︵").1, 26);
        assert_eq!(units("ɑ").1, 26);
    }
}
