//! Reading `JCZ_COR_2_1` and `LMC1COR_1.0` files.
//!
//! The file layouts (label length 0x16, header skips 0xE / 0x1FA, row-major dx-then-dy pairs, scale offsets) follow
//! `_read_correction_file`, `_read_int_correction_file`, `_read_float_correction_file` and
//! `get_scale_from_correction_file` in galvoplotter `galvo/controller.py` @ fc35136 and meerk40t
//! `balormk/controller.py`. Rewritten in Rust; unlike the originals it checks lengths and rejects unknown labels.
//!
//! galvoplotter: MIT License, Copyright (c) 2023 MeerK40t.
//! meerk40t: MIT License, Copyright (c) 2021 meerk40t.
//! Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
//! documentation files (the "Software"), to deal in the Software without restriction, including without limitation
//! the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
//! permit persons to whom the Software is furnished to do so, subject to the following conditions: The above
//! copyright notice and this permission notice shall be included in all copies or substantial portions of the
//! Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED.

use crate::{CorError, CorFormat, CorTable, DEFAULT_TRAILER, NODE_COUNT, valid_scale, validate_nodes};

/// Bytes of the label area at the start of every file.
pub(crate) const LABEL_LEN: usize = 0x16;
/// `JCZ_COR_2_1` label, NUL-terminated ASCII.
pub(crate) const JCZ_LABEL: &[u8; 12] = b"JCZ_COR_2_1\0";
/// Offset of the f64 galvo-units-per-mm in a JCZ file (label + 6 bytes).
pub(crate) const JCZ_SCALE_OFFSET: usize = 28;
/// Offset of the first (dx, dy) pair in a JCZ file (label + 0xE header).
pub(crate) const JCZ_GRID_OFFSET: usize = LABEL_LEN + 0xE;
/// Bytes per JCZ node (two i32).
pub(crate) const JCZ_NODE_LEN: usize = 8;

/// Offset of the first (dx, dy) pair in an LMC1COR file (label + 0x1FA header).
const LMC_GRID_OFFSET: usize = LABEL_LEN + 0x1FA;
/// Offset of the scale in an LMC1COR file: label + 2 bytes, then the 44th f64 (meerk40t reads `63d`, index 43).
const LMC_SCALE_OFFSET: usize = LABEL_LEN + 2 + 43 * 8;
/// Bytes per LMC1COR node (two f64).
const LMC_NODE_LEN: usize = 16;

/// `"LMC1COR_1.0"` as UTF-16LE without a BOM: 22 bytes, exactly the label area.
fn is_lmc_label(label: &[u8]) -> bool {
    let utf16: Vec<u8> = "LMC1COR_1.0".encode_utf16().flat_map(u16::to_le_bytes).collect();
    label == utf16.as_slice()
}

pub(crate) fn parse(bytes: &[u8]) -> Result<CorTable, CorError> {
    let label = bytes.get(..LABEL_LEN).ok_or(CorError::TooShort { expected: LABEL_LEN, actual: bytes.len() })?;
    if label.starts_with(JCZ_LABEL) {
        parse_jcz(bytes)
    } else if is_lmc_label(label) {
        parse_lmc(bytes)
    } else {
        Err(CorError::UnknownFormat { label: printable(label) })
    }
}

fn parse_jcz(bytes: &[u8]) -> Result<CorTable, CorError> {
    let grid_end = JCZ_GRID_OFFSET + NODE_COUNT * JCZ_NODE_LEN;
    check_len(bytes, grid_end)?;
    let scale = f64_at(bytes, JCZ_SCALE_OFFSET);
    let nodes: Vec<[f64; 2]> = bytes[JCZ_GRID_OFFSET..grid_end]
        .as_chunks::<JCZ_NODE_LEN>()
        .0
        .iter()
        .map(|c| [f64::from(i32_at(c, 0)), f64::from(i32_at(c, 4))])
        .collect();
    validate_nodes(&nodes)?;
    let mut reserved = [0u8; 16];
    reserved.copy_from_slice(&bytes[JCZ_LABEL.len()..JCZ_SCALE_OFFSET]);
    Ok(CorTable {
        format: CorFormat::Jcz21,
        scale_units_per_mm: Some(scale).filter(|s| valid_scale(*s)),
        nodes,
        reserved,
        trailer: bytes[grid_end..].to_vec(),
    })
}

fn parse_lmc(bytes: &[u8]) -> Result<CorTable, CorError> {
    let grid_end = LMC_GRID_OFFSET + NODE_COUNT * LMC_NODE_LEN;
    check_len(bytes, grid_end)?;
    let scale = f64_at(bytes, LMC_SCALE_OFFSET);
    let nodes: Vec<[f64; 2]> = bytes[LMC_GRID_OFFSET..grid_end]
        .as_chunks::<LMC_NODE_LEN>()
        .0
        .iter()
        .map(|c| [f64_at(c, 0), f64_at(c, 8)])
        .collect();
    validate_nodes(&nodes)?;
    Ok(CorTable {
        format: CorFormat::Lmc1Cor10,
        scale_units_per_mm: Some(scale).filter(|s| valid_scale(*s)),
        nodes,
        reserved: [0; 16],
        trailer: DEFAULT_TRAILER.to_vec(),
    })
}

fn check_len(bytes: &[u8], expected: usize) -> Result<(), CorError> {
    if bytes.len() < expected { Err(CorError::TooShort { expected, actual: bytes.len() }) } else { Ok(()) }
}

/// Little-endian f64 at `at`; the caller has checked the length.
fn f64_at(bytes: &[u8], at: usize) -> f64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    f64::from_le_bytes(b)
}

/// Little-endian i32 at `at`; the caller has checked the length.
fn i32_at(bytes: &[u8], at: usize) -> i32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&bytes[at..at + 4]);
    i32::from_le_bytes(b)
}

/// ASCII rendering of a label for error messages; NULs dropped, other non-printables as '.'.
fn printable(label: &[u8]) -> String {
    label.iter().filter(|&&b| b != 0).map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GRID_SIZE;

    /// A synthetic LMC1COR file with node i = (i, -i / 2) and the given scale.
    fn lmc_file(scale: f64) -> Vec<u8> {
        let mut f: Vec<u8> = "LMC1COR_1.0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        f.resize(LMC_GRID_OFFSET, 0);
        f[LMC_SCALE_OFFSET..LMC_SCALE_OFFSET + 8].copy_from_slice(&scale.to_le_bytes());
        for i in 0..NODE_COUNT {
            f.extend_from_slice(&(i as f64 * 0.5).to_le_bytes());
            f.extend_from_slice(&(-(i as f64) / 4.0).to_le_bytes());
        }
        f
    }

    #[test]
    fn parses_lmc1cor() {
        let t = parse(&lmc_file(511.9921875)).expect("parse");
        assert_eq!(t.format(), CorFormat::Lmc1Cor10);
        assert_eq!(t.field_mm(), Some(128.0));
        assert_eq!(t.node(0, 1), Some([0.5, -0.25]));
        assert_eq!(t.node(1, 0), Some([GRID_SIZE as f64 * 0.5, -(GRID_SIZE as f64) / 4.0]));
    }

    #[test]
    fn lmc1cor_without_scale() {
        assert_eq!(parse(&lmc_file(0.0)).expect("parse").scale_units_per_mm(), None);
    }

    #[test]
    fn truncated_files_are_rejected() {
        let f = lmc_file(511.9921875);
        assert!(matches!(parse(&f[..f.len() - 1]), Err(CorError::TooShort { .. })));
        assert!(matches!(parse(&f[..10]), Err(CorError::TooShort { expected: LABEL_LEN, actual: 10 })));
        let mut jcz = JCZ_LABEL.to_vec();
        jcz.resize(JCZ_GRID_OFFSET + 100, 0);
        assert!(matches!(parse(&jcz), Err(CorError::TooShort { expected: 33836, .. })));
    }

    #[test]
    fn unknown_label_is_rejected() {
        let mut f = b"JCZ_COR_3_0\0".to_vec();
        f.resize(40_000, 0);
        match parse(&f) {
            Err(CorError::UnknownFormat { label }) => assert_eq!(label, "JCZ_COR_3_0"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        let mut f = lmc_file(511.9921875);
        let at = LMC_GRID_OFFSET + (2 * GRID_SIZE + 3) * LMC_NODE_LEN + 8;
        f[at..at + 8].copy_from_slice(&40_000.0f64.to_le_bytes());
        assert!(matches!(parse(&f), Err(CorError::OffsetOutOfRange { row: 2, col: 3, .. })));
    }
}
