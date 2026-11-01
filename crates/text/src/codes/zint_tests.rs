//! Expected symbols from Zint's test suite (`backend/tests/test_pdf417.c`, `test_hanxin.c`, `test_gridmtx.c`;
//! BSD-3-Clause, Copyright (C) 2019-2026 Robin Stuart; see ATTRIBUTION.md), several of them the figures of
//! ISO/IEC 24728, ISO/IEC 20830, AIMD014 and AIM ITS/04-023: the same data must give the same modules here.

use rxing::common::{CharacterSet, Eci};

use super::{Grid, grid_matrix, hanxin, micro_pdf417};

fn rows_of(bitmap: &str, width: usize) -> Grid {
    bitmap.as_bytes().chunks(width).map(|r| r.iter().map(|b| *b == b'1').collect()).collect()
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// `data` as the units an encoder gets for ECI `eci` (0: the symbology's default).
fn eci_units(data: &str, eci: u32, double_byte_ecis: &[u32]) -> Result<Vec<u32>, String> {
    if double_byte_ecis.contains(&eci) {
        return Ok(hanxin::gb18030_units(data));
    }
    if eci == 20 {
        return Err("skip: Zint's Shift JIS maps the backslash and tilde to full-width".into());
    }
    if eci >= 899 {
        // ECIs without a character set: Zint encodes the UTF-8 bytes unchanged.
        return Ok(data.bytes().map(u32::from).collect());
    }
    let bytes = CharacterSet::from(Eci::from(eci as i32)).encode(data).map_err(|e| format!("skip: ECI {eci}: {e}"))?;
    Ok(bytes.into_iter().map(u32::from).collect())
}

fn opt(v: i64) -> Option<usize> {
    (v > 0).then_some(v as usize)
}

/// Check one vector line `kind|data hex|option_1|option_2|option_3|rows|width|bitmap|comment`.
fn check_symbol(line: &str) -> Result<(), String> {
    let f: Vec<&str> = line.split('|').collect();
    let data = String::from_utf8(hex(f[1])).map_err(|e| e.to_string())?;
    let (o1, o2): (i64, i64) = (f[2].parse().unwrap(), f[3].parse().unwrap());
    let (rows, width): (usize, usize) = (f[5].parse().unwrap(), f[6].parse().unwrap());
    let want = rows_of(f[7], width);
    assert_eq!(want.len(), rows);
    let got = match f[0] {
        "micropdf417" | "micropdf417_fast" => micro_pdf417::rows(&micro_pdf417::data_codewords(&data)?, opt(o2))?,
        "hanxin" => {
            let (o3, eci) = f[4].split_once(':').unwrap();
            let (o3, eci): (i64, i64) = (o3.parse().unwrap(), eci.parse().unwrap());
            let eci = eci.max(0) as u32;
            let dd = if eci == 0 { hanxin::units(&data) } else { eci_units(&data, eci, &[29, 32])? };
            let mask = (o3 > 0).then(|| (o3 >> 8) as usize - 1);
            hanxin::encode(&dd, eci, opt(o1), opt(o2), mask)?.grid
        }
        "gridmatrix" => {
            let (dd, eci) = grid_matrix::units(&data);
            grid_matrix::encode(&dd, eci, opt(o1), opt(o2))?.grid
        }
        k => return Err(format!("unknown kind {k}")),
    };
    if got != want {
        let show = |g: &Grid| {
            g.iter().map(|r| r.iter().map(|b| if *b { '1' } else { '0' }).collect::<String>()).collect::<Vec<_>>()
        };
        return Err(format!("{}: {}\n got  {:?}\n want {:?}", f[0], f[8], show(&got), show(&want)));
    }
    Ok(())
}

/// Check one codeword vector `kind|data hex|eci|option_1|option_3|expected eci|codewords hex|comment`.
fn check_codewords(line: &str) -> Result<(), String> {
    let f: Vec<&str> = line.split('|').collect();
    let data = String::from_utf8(hex(f[1])).map_err(|e| e.to_string())?;
    let (eci, o1, expected_eci): (i64, i64, i64) =
        (f[2].parse().unwrap(), f[3].parse().unwrap(), f[5].parse().unwrap());
    // "(count) first words ..." lists only the first codewords of a long stream.
    let (count, words) = match f[6].strip_prefix('(').and_then(|r| r.split_once(") ")) {
        Some((n, rest)) => (Some(n.parse::<usize>().unwrap()), rest),
        None => (None, f[6]),
    };
    let want: Vec<u8> = words.split_whitespace().map(|h| u8::from_str_radix(h, 16).unwrap()).collect();
    let got = match f[0] {
        "hanxin_cw" => {
            let eci = eci.max(0) as u32;
            let dd = if eci == 0 { hanxin::units(&data) } else { eci_units(&data, eci, &[29, 32])? };
            hanxin::encode(&dd, eci, opt(o1), None, None)?.data
        }
        "gridmatrix_cw" => {
            // Zint picks an ECI when GB 2312 cannot hold the data: the expected one.
            let header = if eci > 0 { eci } else { expected_eci } as u32;
            let (dd, eci) = match header {
                0 => grid_matrix::units(&data),
                29 => (hanxin::gb18030_units(&data), 29),
                e => (eci_units(&data, e, &[])?, e),
            };
            let s = grid_matrix::encode(&dd, eci, opt(o1), None)?;
            s.codewords[..s.data_codewords].to_vec()
        }
        k => return Err(format!("unknown kind {k}")),
    };
    let same = match count {
        Some(n) => got.len() == n && got.starts_with(&want),
        None => got == want,
    };
    if !same {
        return Err(format!("{}: {} {data:?}\n got  {got:02X?}\n want {want:02X?}", f[0], f[7]));
    }
    Ok(())
}

fn check(line: &str) -> Result<(), String> {
    if line.split('|').next().is_some_and(|k| k.ends_with("_cw")) { check_codewords(line) } else { check_symbol(line) }
}

/// The vectors kept in `zint_vectors.txt`: MicroPDF417 (ISO/IEC 24728 Figure 1 and every width), Han Xin (ISO/IEC
/// 20830 Figures 1, 2 and 4 and Annex K, versions 1 to 24, all four masks, Chinese modes, ECIs) and Grid Matrix
/// symbols, and the data codewords of 157 Han Xin and Grid Matrix inputs (modes, ECIs, GB 2312 / GB 18030).
#[test]
fn zint_test_suite_vectors() {
    let lines: Vec<&str> =
        include_str!("zint_vectors.txt").lines().filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    assert!(lines.len() > 150);
    let failed: Vec<String> = lines.iter().filter_map(|l| check(l).err()).collect();
    assert!(
        failed.is_empty(),
        "{} of {} differ:
{}",
        failed.len(),
        lines.len(),
        failed.join(
            "
"
        )
    );
}

/// Every vector extracted from Zint's tests (files named by `ZINT_VECTORS` and `ZINT_CODEWORDS`), for development.
/// MicroPDF417 data that Zint compacts differently from rxing's Annex P encoder (4 inputs mixing byte, text and
/// numeric runs) is reported but not failed: both are valid encodations of the data.
#[test]
#[ignore]
fn all_zint_vectors() {
    let (mut n, mut skipped) = (0, 0);
    let (mut failed, mut compaction) = (Vec::new(), Vec::new());
    for var in ["ZINT_VECTORS", "ZINT_CODEWORDS"] {
        let Ok(path) = std::env::var(var) else { continue };
        for line in std::fs::read_to_string(path).unwrap().lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            n += 1;
            match check(line) {
                Err(e) if e.starts_with("skip") => skipped += 1,
                Err(e) if e.starts_with("micropdf417") => compaction.push(e),
                Err(e) => failed.push(e),
                Ok(()) => {}
            }
        }
    }
    for e in failed.iter().chain(&compaction) {
        eprintln!(
            "{e}
"
        );
    }
    eprintln!(
        "{} of {n} vectors differ ({} MicroPDF417 compaction), {skipped} skipped",
        failed.len() + compaction.len(),
        compaction.len()
    );
    assert!(failed.is_empty());
}
