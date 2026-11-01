//! Linear codes rxing does not encode: 2 of 5 Industrial (Standard 2 of 5), 2 of 5 IATA and UK Plessey, plus the
//! input checks that turn ISBN into EAN-13 and VIN into Code 39.
//!
//! The bar patterns follow Zint's backend (BSD-3-Clause, Copyright (C) 2008 Robin Stuart; `2of5.c`, `plessey.c`,
//! `upcean.c`, `code.c`): pattern strings list alternating bar / space widths in modules, starting with a bar, with a
//! wide element 3 modules (a 3:1 ratio). See ATTRIBUTION.md.

/// Bars and spaces of a pattern string ("31311...") as modules: `true` dark.
fn modules(patterns: &[&str]) -> Vec<bool> {
    let mut out = Vec::new();
    let mut dark = true;
    for p in patterns {
        for c in p.bytes() {
            let w = usize::from(c - b'0');
            out.extend(std::iter::repeat_n(dark, w));
            dark = !dark;
        }
    }
    out
}

/// The 2 of 5 digit patterns (Zint `C25IndustTable`): each digit is five bars, two of them wide, every space narrow.
const C25_INDUSTRIAL: [&str; 10] = [
    "1111313111", "3111111131", "1131111131", "3131111111", "1111311131", "3111311111", "1131311111", "1111113131",
    "3111113111", "1131113111",
];

/// Longest 2 of 5 data accepted (Zint allows 45 digits for Industrial and IATA).
const MAX_C25: usize = 45;

fn digits(data: &str, what: &str, max: usize) -> Result<(), String> {
    if data.is_empty() || data.len() > max || !data.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{what} encodes 1 to {max} digits (0-9)"));
    }
    Ok(())
}

/// 2 of 5 Industrial (Standard 2 of 5): start "313111", the digits, stop "31113" (Zint `c25ind`).
pub fn two_of_five_industrial(data: &str) -> Result<Vec<bool>, String> {
    digits(data, "2 of 5 Industrial", MAX_C25)?;
    let mut p = vec!["313111"];
    p.extend(data.bytes().map(|b| C25_INDUSTRIAL[usize::from(b - b'0')]));
    p.push("31113");
    Ok(modules(&p))
}

/// 2 of 5 IATA: the Industrial digits between start "1111" and stop "311" (Zint `c25iata`).
pub fn two_of_five_iata(data: &str) -> Result<Vec<bool>, String> {
    digits(data, "2 of 5 IATA", MAX_C25)?;
    let mut p = vec!["1111"];
    p.extend(data.bytes().map(|b| C25_INDUSTRIAL[usize::from(b - b'0')]));
    p.push("311");
    Ok(modules(&p))
}

/// The CRC polynomial of UK Plessey (x^8 + x^7 + x^6 + x^5 + x^3 + 1), highest power first (Zint `plessey.c`, from
/// GNU Barcode).
const PLESSEY_POLY: [u8; 9] = [1, 1, 1, 1, 0, 1, 0, 0, 1];

/// The 8 check bits UK Plessey appends: the remainder of the data bits (4 per character, least significant first)
/// divided by [`PLESSEY_POLY`].
pub fn plessey_check_bits(values: &[u8]) -> [u8; 8] {
    let mut bits: Vec<u8> = values.iter().flat_map(|v| (0..4).map(move |j| (v >> j) & 1)).collect();
    let n = bits.len();
    bits.extend([0; 8]);
    for i in 0..n {
        if bits[i] == 1 {
            for (j, g) in PLESSEY_POLY.iter().enumerate() {
                bits[i + j] ^= g;
            }
        }
    }
    let mut out = [0; 8];
    out.copy_from_slice(&bits[n..n + 8]);
    out
}

/// UK Plessey: hexadecimal digits (0-9, A-F), each four bits least significant first (a 1 is "31", a 0 "13"), the
/// 8-bit CRC, between start "31311331" and stop "331311313" (Zint `plessey`).
pub fn plessey(data: &str) -> Result<Vec<bool>, String> {
    let up = data.to_ascii_uppercase();
    if up.is_empty() || up.len() > 67 || !up.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Plessey encodes 1 to 67 hexadecimal digits (0-9, A-F)".into());
    }
    let values: Vec<u8> = up.bytes().map(|b| (b as char).to_digit(16).expect("checked") as u8).collect();
    let bit = |b: u8| if b == 1 { "31" } else { "13" };
    let mut p = vec!["31311331"];
    for v in &values {
        p.extend((0..4).map(|j| bit((v >> j) & 1)));
    }
    p.extend(plessey_check_bits(&values).iter().map(|b| bit(*b)));
    p.push("331311313");
    Ok(modules(&p))
}

/// The EAN-13 digits (13, check digit included) of an ISBN: an ISBN-13 (978 / 979 prefix, with or without its check
/// digit) or an ISBN-10 (with or without its check digit, `X` for 10), hyphens and spaces ignored. A given check
/// digit must be right.
pub fn isbn_to_ean13(data: &str) -> Result<String, String> {
    let s: String = data.chars().filter(|c| !matches!(c, '-' | ' ')).collect::<String>().to_ascii_uppercase();
    let ean_check = |d: &[u8]| -> u8 {
        let sum: u32 = d.iter().enumerate().map(|(i, x)| u32::from(*x) * if i % 2 == 0 { 1 } else { 3 }).sum();
        ((10 - sum % 10) % 10) as u8
    };
    let nums = |s: &str| -> Option<Vec<u8>> { s.bytes().map(|b| b.is_ascii_digit().then(|| b - b'0')).collect() };
    let bad = || format!("{data:?} is not an ISBN (10 or 13 digits; ISBN-13 starts 978 or 979)");
    let twelve: Vec<u8> = match s.len() {
        12 | 13 => {
            let d = nums(&s).ok_or_else(bad)?;
            if !(d.starts_with(&[9, 7, 8]) || d.starts_with(&[9, 7, 9])) {
                return Err(bad());
            }
            if d.len() == 13 && ean_check(&d[..12]) != d[12] {
                return Err(format!("ISBN {data:?}: wrong check digit (expected {})", ean_check(&d[..12])));
            }
            d[..12].to_vec()
        }
        9 | 10 => {
            let d = nums(&s[..9]).ok_or_else(bad)?;
            if s.len() == 10 {
                let sum: u32 = d.iter().enumerate().map(|(i, x)| u32::from(*x) * (10 - i as u32)).sum();
                let check = (11 - sum % 11) % 11;
                let given = match s.as_bytes()[9] {
                    b'X' => 10,
                    b if b.is_ascii_digit() => u32::from(b - b'0'),
                    _ => return Err(bad()),
                };
                if given != check {
                    let want = if check == 10 { "X".to_string() } else { check.to_string() };
                    return Err(format!("ISBN {data:?}: wrong check digit (expected {want})"));
                }
            }
            [9, 7, 8].iter().copied().chain(d).collect()
        }
        _ => return Err(bad()),
    };
    let check = ean_check(&twelve);
    Ok(twelve.iter().chain(std::iter::once(&check)).map(|d| char::from(b'0' + d)).collect())
}

/// A Vehicle Identification Number checked for Code 39 (Zint `vin`): 17 characters 0-9 and A-Z without I, O and Q,
/// and for North American VINs (first character 1-5) the check digit in position 9.
pub fn vin(data: &str) -> Result<String, String> {
    let s = data.trim().to_ascii_uppercase();
    if s.len() != 17 || !s.bytes().all(|b| b.is_ascii_digit() || (b.is_ascii_uppercase() && !matches!(b, b'I' | b'O' | b'Q'))) {
        return Err("a VIN is 17 characters: 0-9 and A-Z without I, O and Q".into());
    }
    if matches!(s.as_bytes()[0], b'1'..=b'5') {
        const WEIGHTS: [u32; 17] = [8, 7, 6, 5, 4, 3, 2, 10, 0, 9, 8, 7, 6, 5, 4, 3, 2];
        let value = |b: u8| -> u32 {
            match b {
                b'0'..=b'9' => u32::from(b - b'0'),
                // Transliteration: A-H 1-8, J-N 1-5, P 7, R 9, S-Z 2-9.
                b'A'..=b'H' => u32::from(b - b'A') + 1,
                b'J'..=b'N' => u32::from(b - b'J') + 1,
                b'P' => 7,
                b'R' => 9,
                _ => u32::from(b - b'S') + 2,
            }
        };
        let sum: u32 = s.bytes().zip(WEIGHTS).map(|(b, w)| value(b) * w).sum();
        let check = match sum % 11 {
            10 => b'X',
            r => b'0' + r as u8,
        };
        if s.as_bytes()[8] != check {
            return Err(format!("VIN {s}: wrong check digit in position 9 (expected {})", check as char));
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Widths of the runs of a module row.
    fn runs(m: &[bool]) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < m.len() {
            let j = (i..m.len()).find(|&j| m[j] != m[i]).unwrap_or(m.len());
            out.push(j - i);
            i = j;
        }
        out
    }

    #[test]
    fn two_of_five_patterns() {
        // Industrial: start 313111, "1" 3111111131, stop 31113.
        let m = two_of_five_industrial("1").unwrap();
        assert_eq!(runs(&m), [3, 1, 3, 1, 1, 1, 3, 1, 1, 1, 1, 1, 1, 1, 3, 1, 3, 1, 1, 1, 3]);
        assert!(m[0] && *m.last().unwrap());
        let i = two_of_five_iata("1").unwrap();
        assert_eq!(runs(&i), [1, 1, 1, 1, 3, 1, 1, 1, 1, 1, 1, 1, 3, 1, 3, 1, 1]);
        // Every digit has exactly two wide bars.
        for p in C25_INDUSTRIAL {
            assert_eq!(p.bytes().step_by(2).filter(|b| *b == b'3').count(), 2);
            assert!(p.bytes().skip(1).step_by(2).all(|b| b == b'1'));
        }
        assert!(two_of_five_industrial("12a").is_err() && two_of_five_iata("").is_err());
        assert!(two_of_five_iata(&"1".repeat(46)).is_err());
    }

    #[test]
    fn plessey_crc_divides_out() {
        let values = [1u8, 2, 3, 0xA];
        let check = plessey_check_bits(&values);
        // Data bits followed by the check bits leave no remainder.
        let mut bits: Vec<u8> = values.iter().flat_map(|v| (0..4).map(move |j| (v >> j) & 1)).collect();
        bits.extend(check);
        for i in 0..bits.len() - 8 {
            if bits[i] == 1 {
                for (j, g) in PLESSEY_POLY.iter().enumerate() {
                    bits[i + j] ^= g;
                }
            }
        }
        assert!(bits.iter().all(|b| *b == 0));
        let m = plessey("123a").unwrap();
        // Start 8 elements, 4 x 4 data bits and 8 check bits of 2 elements each, stop 9 elements; modules 4 per bit.
        assert_eq!(runs(&m).len(), 8 + 16 * 2 + 8 * 2 + 9);
        assert_eq!(m.len(), 16 + (16 + 8) * 4 + 19);
        assert!(plessey("12G").is_err());
    }

    #[test]
    fn isbn_and_vin() {
        assert_eq!(isbn_to_ean13("978-0-306-40615-7").unwrap(), "9780306406157");
        assert_eq!(isbn_to_ean13("978030640615").unwrap(), "9780306406157");
        assert_eq!(isbn_to_ean13("0-306-40615-2").unwrap(), "9780306406157");
        assert_eq!(isbn_to_ean13("030640615").unwrap(), "9780306406157");
        assert_eq!(isbn_to_ean13("0-8044-2957-X").unwrap(), "9780804429573");
        assert!(isbn_to_ean13("0-306-40615-3").unwrap_err().contains("expected 2"));
        assert!(isbn_to_ean13("9780306406158").unwrap_err().contains("expected 7"));
        assert!(isbn_to_ean13("1234567890123").is_err());
        assert_eq!(vin("1m8gdm9axkp042788").unwrap(), "1M8GDM9AXKP042788");
        assert!(vin("1M8GDM9A1KP042788").unwrap_err().contains("expected X"));
        assert!(vin("WVWZZZ1JZXW000001").is_ok(), "European VINs have no check digit");
        assert!(vin("1M8GDM9AXKP04278O").is_err() && vin("SHORT").is_err());
    }
}
