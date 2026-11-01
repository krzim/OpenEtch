//! MicroPDF417 (ISO/IEC 24728:2006): 34 sizes of 1 to 4 data columns and 4 to 44 rows. The data codewords come from
//! rxing's PDF417 high-level encoder (Apache-2.0; text, byte and numeric compaction are the same as PDF417's, but a
//! MicroPDF417 symbol starts in byte compaction, so text gets its latch) and its PDF417 symbol table supplies the bar
//! patterns. The size table, row
//! address patterns and their start positions are ISO/IEC 24728 Table 1 and ISO/IEC 15438 Table 2 as tabulated in
//! Zint's `pdf417_tabs.h` (BSD-3-Clause); the check words are Reed-Solomon over GF(929) like PDF417's, with the
//! generator of Annex F. See ATTRIBUTION.md.
//!
//! Each row is two modules tall (the minimum row height of 5.8.2), so a symbol of `r` rows is `2 r` modules high.

use std::sync::OnceLock;

use rxing::common::CharacterSet;
use rxing::pdf417::encoder::Compaction;
use rxing::pdf417::encoder::pdf_417_high_level_encoder::encodeHighLevel;
use rxing::pdf417::pdf_417_common::{SYMBOL_TABLE, getCodeword};

use super::Grid;

/// Per size (variant 1-34): data columns, rows and check codewords (ISO/IEC 24728 Table 1).
const COLUMNS: [u8; 34] =
    [1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4];
const ROWS: [u8; 34] = [
    11, 14, 17, 20, 24, 28, 8, 11, 14, 17, 20, 23, 26, 6, 8, 10, 12, 15, 20, 26, 32, 38, 44, 4, 6, 8, 10, 12, 15, 20,
    26, 32, 38, 44,
];
const CHECK: [u8; 34] = [
    7, 7, 7, 8, 8, 8, 8, 9, 9, 10, 11, 13, 15, 12, 14, 16, 18, 21, 26, 32, 38, 44, 50, 8, 12, 14, 16, 18, 21, 26, 32,
    38, 44, 50,
];
/// Per size: the first row's left, centre and right row address pattern (1-52) and cluster (0, 1, 2 for clusters 0, 3
/// and 6) (ISO/IEC 24728 Tables 10-12).
const LEFT_RAP: [u8; 34] = [
    1, 8, 36, 19, 9, 25, 1, 1, 8, 36, 19, 9, 27, 1, 7, 15, 25, 37, 1, 1, 21, 15, 1, 47, 1, 7, 15, 25, 37, 1, 1, 21, 15,
    1,
];
const CENTRE_RAP: [u8; 34] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 7, 15, 25, 37, 17, 9, 29, 31, 25, 19, 1, 7, 15, 25, 37, 17, 9, 29, 31, 25,
];
const RIGHT_RAP: [u8; 34] = [
    9, 8, 36, 19, 17, 33, 1, 9, 8, 36, 19, 17, 35, 1, 7, 15, 25, 37, 33, 17, 37, 47, 49, 43, 1, 7, 15, 25, 37, 33, 17,
    37, 47, 49,
];
const START_CLUSTER: [u8; 34] =
    [0, 1, 2, 0, 2, 0, 0, 0, 1, 2, 0, 2, 2, 0, 0, 2, 0, 0, 0, 0, 2, 2, 0, 1, 0, 0, 2, 0, 0, 0, 0, 2, 2, 0];
/// Left and right row address patterns (10 modules) and centre ones, 1-52 (ISO/IEC 15438 Table 2).
const RAP_SIDE: [u16; 52] = [
    0x322, 0x3A2, 0x3B2, 0x332, 0x372, 0x37A, 0x33A, 0x3BA, 0x39A, 0x3DA, 0x3CA, 0x38A, 0x30A, 0x31A, 0x312, 0x392,
    0x3D2, 0x3D6, 0x3D4, 0x394, 0x3B4, 0x3A4, 0x3A6, 0x3AE, 0x3AC, 0x3A8, 0x328, 0x32C, 0x32E, 0x326, 0x336, 0x3B6,
    0x396, 0x316, 0x314, 0x334, 0x374, 0x364, 0x366, 0x36E, 0x36C, 0x368, 0x348, 0x358, 0x35C, 0x35E, 0x34E, 0x34C,
    0x344, 0x346, 0x342, 0x362,
];
const RAP_CENTRE: [u16; 52] = [
    0x2CE, 0x24E, 0x26E, 0x22E, 0x226, 0x236, 0x216, 0x212, 0x21A, 0x23A, 0x232, 0x222, 0x262, 0x272, 0x27A, 0x2FA,
    0x2F2, 0x2F6, 0x276, 0x274, 0x264, 0x266, 0x246, 0x242, 0x2C2, 0x2E2, 0x2E6, 0x2E4, 0x2EC, 0x26C, 0x22C, 0x228,
    0x268, 0x2E8, 0x2C8, 0x2CC, 0x2C4, 0x2C6, 0x286, 0x28E, 0x28C, 0x29C, 0x298, 0x2B8, 0x2B0, 0x290, 0x2D0, 0x250,
    0x258, 0x25C, 0x2DC, 0x2DE,
];

/// Zint's automatic sizing (`pdf_MicroAutosize`): data capacities in ascending order and the variant (1-34) used
/// for each (of two sizes with one capacity, the one Zint picks).
const AUTOSIZE: [(u8, u8); 28] = [
    (4, 1),
    (6, 14),
    (7, 2),
    (8, 7),
    (10, 3),
    (12, 25),
    (13, 8),
    (14, 16),
    (16, 5),
    (18, 17),
    (19, 9),
    (20, 6),
    (24, 10),
    (29, 11),
    (30, 28),
    (33, 12),
    (34, 19),
    (37, 13),
    (39, 29),
    (46, 20),
    (54, 30),
    (58, 21),
    (70, 22),
    (72, 31),
    (82, 23),
    (90, 32),
    (108, 33),
    (126, 34),
];

/// The smallest size (0-based variant) of `columns` (or of any width, `None`) holding `n` data codewords.
fn variant(n: usize, columns: Option<usize>) -> Option<usize> {
    let fits = |v: &usize| usize::from(COLUMNS[*v]) * usize::from(ROWS[*v]) - usize::from(CHECK[*v]) >= n;
    match columns {
        Some(c) => (0..34).filter(|v| usize::from(COLUMNS[*v]) == c).find(fits),
        None => AUTOSIZE.iter().find(|(cap, _)| usize::from(*cap) >= n).map(|(_, v)| usize::from(*v) - 1),
    }
}

/// The 17-module bar pattern of every codeword in clusters 0, 3 and 6, from rxing's sorted symbol table.
fn patterns() -> &'static [[u32; 929]; 3] {
    static P: OnceLock<Box<[[u32; 929]; 3]>> = OnceLock::new();
    P.get_or_init(|| {
        let mut p = Box::new([[0u32; 929]; 3]);
        for s in SYMBOL_TABLE {
            // Bar widths b1..b4 of the pattern (it starts with a bar): cluster = (b1 - b2 + b3 - b4 + 9) mod 9.
            let mut widths = Vec::with_capacity(8);
            let (mut run, mut last) = (0i32, true);
            for i in (0..17).rev() {
                let bit = (s >> i) & 1 == 1;
                if bit == last {
                    run += 1;
                } else {
                    widths.push(run);
                    run = 1;
                    last = bit;
                }
            }
            widths.push(run);
            let cluster = (widths[0] - widths[2] + widths[4] - widths[6]).rem_euclid(9);
            let cw = getCodeword(s);
            if cluster % 3 == 0 && (0..929).contains(&cw) {
                p[(cluster / 3) as usize][cw as usize] = s;
            }
        }
        p
    })
}

/// The data codewords of `data` (rxing's PDF417 compaction; UTF-8 after ECI 26 when a character is beyond Latin-1).
pub fn data_codewords(data: &str) -> Result<Vec<u16>, String> {
    let latin1 = data.chars().all(|c| u32::from(c) <= 0xFF);
    let encoding = if latin1 { None } else { Some(CharacterSet::UTF8) };
    let high_level = |compaction| {
        std::panic::catch_unwind(|| encodeHighLevel(data, compaction, encoding, false))
            .map_err(|_| "the PDF417 Micro encoder failed on this data".to_string())?
            .map_err(|e| format!("PDF417 Micro cannot encode {data:?}: {e}"))
    };
    let mut s = high_level(Compaction::AUTO)?;
    // Annex P only takes 13 digits or more as numbers; short all-digit data is smaller as numeric compaction.
    if data.bytes().all(|b| b.is_ascii_digit()) {
        let numeric = high_level(Compaction::NUMERIC)?;
        if numeric.chars().count() < s.chars().count() + 1 {
            s = numeric;
        }
    }
    let mut cws: Vec<u16> = s.chars().map(|c| c as u16).collect();
    // rxing starts in text compaction like PDF417; MicroPDF417 starts in byte compaction (ISO/IEC 24728 5.4.3), so
    // text first needs its latch (after any ECI designator, which keeps the mode).
    let mut at = 0;
    while let Some(eci) = cws.get(at).filter(|c| (925..=927).contains(*c)) {
        at += if *eci == 926 { 3 } else { 2 };
    }
    if cws.get(at).is_some_and(|c| ![900, 901, 902, 924].contains(c)) {
        cws.insert(at, 900);
    }
    Ok(cws)
}

/// Reed-Solomon check words over GF(929) with the generator (x - 3)(x - 3^2)...(x - 3^k) (ISO/IEC 15438 5.10).
pub fn check_words(data: &[u16], k: usize) -> Vec<u16> {
    let g = generator(k);
    // Remainder of data(x) x^k divided by g(x); the check words are its negated coefficients, highest first.
    let mut rem = vec![0u32; k];
    for d in data {
        let t = (u32::from(*d) + rem[k - 1]) % 929;
        for j in (1..k).rev() {
            rem[j] = (rem[j - 1] + 929 - t * g[j] % 929) % 929;
        }
        rem[0] = (929 - t * g[0] % 929) % 929;
    }
    rem.iter().rev().map(|r| ((929 - r) % 929) as u16).collect()
}

/// The generator's coefficients below x^k, lowest degree first (the order of ISO/IEC 24728 Annex F).
pub fn generator(k: usize) -> Vec<u32> {
    let mut g = vec![1u32];
    let mut root = 1u32;
    for _ in 0..k {
        root = root * 3 % 929;
        let mut next = vec![0u32; g.len() + 1];
        for (i, c) in g.iter().enumerate() {
            next[i + 1] = (next[i + 1] + c) % 929;
            next[i] = (next[i] + 929 - c * root % 929) % 929;
        }
        g = next;
    }
    g.truncate(k);
    g
}

/// The MicroPDF417 rows (one module row each) of the data codewords `cws`, `columns` 1-4 or the smallest size.
pub fn rows(cws: &[u16], columns: Option<usize>) -> Result<Vec<Vec<bool>>, String> {
    if cws.len() > 126 {
        return Err(format!("too much data for PDF417 Micro: {} codewords (at most 126)", cws.len()));
    }
    let v = variant(cws.len(), columns.filter(|c| (1..=4).contains(c)))
        .or_else(|| variant(cws.len(), None))
        .ok_or_else(|| format!("too much data for PDF417 Micro: {} codewords", cws.len()))?;
    let (cols, rows, k) = (usize::from(COLUMNS[v]), usize::from(ROWS[v]), usize::from(CHECK[v]));
    let mut all = cws.to_vec();
    all.resize(cols * rows - k, 900);
    all.extend(check_words(&all, k));
    let pats = patterns();
    let (mut left, mut centre, mut right) =
        (usize::from(LEFT_RAP[v]) - 1, usize::from(CENTRE_RAP[v]).max(1) - 1, usize::from(RIGHT_RAP[v]) - 1);
    let mut cluster = usize::from(START_CLUSTER[v]);
    let mut out = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut bits = Vec::new();
        let mut push = |value: u32, n: u32| bits.extend((0..n).rev().map(|i| (value >> i) & 1 == 1));
        let words = &all[r * cols..(r + 1) * cols];
        push(u32::from(RAP_SIDE[left]), 10);
        for (i, w) in words.iter().enumerate() {
            // The centre pattern goes before the second column of 3, the third of 4.
            if (cols == 3 && i == 1) || (cols == 4 && i == 2) {
                push(u32::from(RAP_CENTRE[centre]), 10);
            }
            push(pats[cluster][usize::from(*w)], 17);
        }
        push(u32::from(RAP_SIDE[right]), 10);
        push(1, 1);
        out.push(bits);
        left = (left + 1) % 52;
        centre = (centre + 1) % 52;
        right = (right + 1) % 52;
        cluster = (cluster + 1) % 3;
    }
    Ok(out)
}

/// PDF417 Micro of `data`: each row two modules tall.
pub fn micro_pdf417(data: &str) -> Result<Grid, String> {
    let rows = rows(&data_codewords(data)?, None)?;
    Ok(rows.into_iter().flat_map(|r| [r.clone(), r]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_is_annex_f() {
        // ISO/IEC 24728 Annex F, k = 7 and the first words of k = 50.
        assert_eq!(generator(7), [76, 925, 537, 597, 784, 691, 437]);
        assert_eq!(generator(50)[..6], [923, 797, 576, 875, 156, 706]);
        // A whole codeword sequence with its check words is a multiple of the generator: dividing again leaves 0.
        let data = [900u16, 1, 63, 125, 900];
        let mut all = data.to_vec();
        all.extend(check_words(&data, 8));
        assert!(check_words(&all, 8).iter().all(|c| *c == 0));
    }

    #[test]
    fn every_codeword_has_a_pattern_in_each_cluster() {
        let p = patterns();
        for (cluster, row) in p.iter().enumerate() {
            assert!(row.iter().all(|s| *s >> 16 == 1), "cluster {}", cluster * 3);
        }
        // Codeword 1 in cluster 0 (rxing's encoder table: 0x1eaf0).
        assert_eq!(p[0][1], 0x1eaf0);
    }

    #[test]
    fn text_starts_with_its_latch() {
        assert_eq!(data_codewords("ABCDEF").unwrap(), [900, 1, 63, 125]);
        assert_eq!(data_codewords("AB").unwrap(), [901, 65, 66], "short text is smaller as bytes (Annex P)");
        assert_eq!(data_codewords("1234567").unwrap()[0], 902);
        // After an ECI designator (UTF-8 for characters beyond Latin-1) the latch still comes first.
        let cws = data_codewords("Ωx").unwrap();
        assert_eq!(cws[..2], [927, 26]);
        assert!([900, 901, 924].contains(&cws[2]));
        assert!(rows(&[900; 127], None).is_err());
    }
}
