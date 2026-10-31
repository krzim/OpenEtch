//! The `WriteCorTable` / `WriteCorLine` upload sequence.
//!
//! Mirrors `_write_correction_table`, `write_cor_table` and the value conversion in `_read_*_correction_file` of
//! galvoplotter `galvo/controller.py` @ fc35136 and meerk40t `balormk/controller.py`: `WriteCorTable(1)`, then one
//! `WriteCorLine(dx, dy, flag)` per node in file order, flag 0 for the first and 1 for the rest; negative offsets are
//! sent sign-magnitude, `(-v + 0x8000) & 0xFFFF`. Rewritten in Rust.
//!
//! galvoplotter: MIT License, Copyright (c) 2023 MeerK40t.
//! meerk40t: MIT License, Copyright (c) 2021 meerk40t.
//! Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
//! documentation files (the "Software"), to deal in the Software without restriction, including without limitation
//! the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
//! permit persons to whom the Software is furnished to do so, subject to the following conditions: The above
//! copyright notice and this permission notice shall be included in all copies or substantial portions of the
//! Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED.

use crate::{CorTable, round_offset};

/// `WriteCorTable` argument that precedes uploading a table (meerk40t `write_cor_table(True)`).
pub const COR_TABLE_LOAD: u16 = 1;
/// `WriteCorTable` argument meerk40t sends, with no lines after it, to run without a correction file.
pub const COR_TABLE_BLANK: u16 = 0;

/// One `WriteCorLine` command's parameters, already encoded as the controller expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorLine {
    /// X offset, sign-magnitude (bit 15 = negative). See [`encode_offset`].
    pub dx: u16,
    /// Y offset, sign-magnitude.
    pub dy: u16,
    /// True only for the first line of a table. On the wire the flag is inverted: 0 for the first line, 1 after it.
    pub first: bool,
}

impl CorLine {
    /// The third `WriteCorLine` parameter: 0 for the first line, 1 otherwise.
    pub fn flag(&self) -> u16 {
        if self.first { 0 } else { 1 }
    }
}

/// Encodes an offset in galvo units the way meerk40t does: `v` if non-negative, else `(-v + 0x8000) & 0xFFFF`.
///
/// Only values in ±[`crate::MAX_OFFSET`] encode unambiguously (−32768 wraps to 0); [`CorTable`] never holds others.
pub fn encode_offset(v: i32) -> u16 {
    let raw = if v < 0 { 0x8000 - i64::from(v) } else { i64::from(v) };
    (raw & 0xFFFF) as u16
}

impl CorTable {
    /// The 4225 `WriteCorLine` parameter sets, in upload order. Send `WriteCorTable(`[`COR_TABLE_LOAD`]`)` first.
    pub fn upload_lines(&self) -> Vec<CorLine> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, [dx, dy])| CorLine {
                dx: encode_offset(round_offset(*dx)),
                dy: encode_offset(round_offset(*dy)),
                first: i == 0,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NODE_COUNT, round_offset};

    #[test]
    fn sign_magnitude() {
        assert_eq!(encode_offset(0), 0);
        assert_eq!(encode_offset(1), 1);
        assert_eq!(encode_offset(-1), 0x8001);
        assert_eq!(encode_offset(-2520), 0x8000 + 2520);
        assert_eq!(encode_offset(0x7FFF), 0x7FFF);
        assert_eq!(encode_offset(-0x7FFF), 0xFFFF);
        // meerk40t's masking: -32768 wraps to 0. CorTable rejects such values.
        assert_eq!(encode_offset(-0x8000), 0);
    }

    #[test]
    fn identity_upload() {
        let lines = CorTable::identity(None).upload_lines();
        assert_eq!(lines.len(), NODE_COUNT);
        assert!(lines[0].first && lines[0].flag() == 0);
        assert!(lines[1..].iter().all(|l| !l.first && l.flag() == 1 && l.dx == 0 && l.dy == 0));
    }

    #[test]
    fn float_offsets_are_rounded_before_encoding() {
        let mut nodes = vec![[0.0, 0.0]; NODE_COUNT];
        nodes[0] = [-2.5, 3.5];
        let t = CorTable::from_nodes(None, nodes).expect("valid");
        assert_eq!(round_offset(-2.5), -2);
        assert_eq!(t.upload_lines()[0], CorLine { dx: 0x8002, dy: 4, first: true });
    }
}
