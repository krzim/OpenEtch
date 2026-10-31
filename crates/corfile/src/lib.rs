//! EzCad lens correction (.cor) files: parse, upload sequence, host-side correction. Pure (no I/O, wasm-ok).
//!
//! A correction file is a 65x65 grid of (dx, dy) offsets in galvo units, row-major. The controller interpolates it in
//! hardware once it has been uploaded with `WriteCorTable(1)` followed by 4225 `WriteCorLine` commands; this crate
//! produces those lines ([`CorTable::upload_lines`]) but never talks to the machine (`machine` maps them to `lmc`).
//!
//! Two on-disk formats are understood:
//! - `JCZ_COR_2_1` (EzCad 2): ASCII label, an f64 galvo-units-per-mm scale at offset 28, i32 LE pairs from offset 36,
//!   then trailing bytes (one i32 in files seen so far). Parsed and written byte-for-byte.
//! - `LMC1COR_1.0` (older): UTF-16LE label, a 0x1FA-byte header after the 0x16-byte label, then f64 LE pairs.
//!   Parsed only.
//!
//! Layout and upload order follow meerk40t `balormk/controller.py` / galvoplotter `galvo/controller.py` (MIT); see the
//! headers of `parse.rs` and `upload.rs`.

mod correct;
mod error;
mod parse;
mod upload;
mod write;

pub use error::CorError;
pub use upload::{COR_TABLE_BLANK, COR_TABLE_LOAD, CorLine, encode_offset};

/// Nodes per side of the correction grid.
pub const GRID_SIZE: usize = 65;
/// Total nodes in the grid (65 * 65).
pub const NODE_COUNT: usize = GRID_SIZE * GRID_SIZE;
/// Galvo units from one field edge to the other (0x0000..=0xFFFF). EzCad stores units-per-mm = 65535 / field size.
pub const FIELD_SPAN_UNITS: f64 = 65535.0;
/// Largest offset magnitude that survives the sign-magnitude encoding of `WriteCorLine` (15 bits).
pub const MAX_OFFSET: i32 = 0x7FFF;

/// Which file format a table was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorFormat {
    /// EzCad 2 `JCZ_COR_2_1`: integer offsets.
    Jcz21,
    /// `LMC1COR_1.0`: floating-point offsets.
    Lmc1Cor10,
}

/// A 65x65 lens correction grid.
///
/// Invariants (checked by every constructor): exactly [`NODE_COUNT`] nodes, every offset finite and, once rounded,
/// within ±[`MAX_OFFSET`] galvo units, so the upload encoding never wraps.
#[derive(Debug, Clone, PartialEq)]
pub struct CorTable {
    format: CorFormat,
    /// Galvo units per mm, `None` when the file holds no usable (finite, positive) value.
    scale_units_per_mm: Option<f64>,
    /// (dx, dy) offsets in galvo units, row-major: index = row * 65 + col.
    nodes: Vec<[f64; 2]>,
    /// JCZ header bytes 12..28 (label padding and 6 unknown bytes; all zero in files seen so far).
    reserved: [u8; 16],
    /// Bytes after the grid in a JCZ file.
    trailer: Vec<u8>,
}

/// Trailer written for tables that did not come from a JCZ file: the i32 7 that EzCad wrote in the owner's
/// `jcz150.cor`. Its meaning is unknown.
pub(crate) const DEFAULT_TRAILER: [u8; 4] = 7i32.to_le_bytes();

impl CorTable {
    /// All-zero grid (no correction), `JCZ_COR_2_1`, with the given scale.
    pub fn identity(scale_units_per_mm: Option<f64>) -> Self {
        CorTable {
            format: CorFormat::Jcz21,
            scale_units_per_mm: scale_units_per_mm.filter(|s| valid_scale(*s)),
            nodes: vec![[0.0, 0.0]; NODE_COUNT],
            reserved: [0; 16],
            trailer: DEFAULT_TRAILER.to_vec(),
        }
    }

    /// Builds a `JCZ_COR_2_1` table from row-major (dx, dy) offsets in galvo units.
    pub fn from_nodes(scale_units_per_mm: Option<f64>, nodes: Vec<[f64; 2]>) -> Result<Self, CorError> {
        validate_nodes(&nodes)?;
        let mut table = CorTable::identity(scale_units_per_mm);
        table.nodes = nodes;
        Ok(table)
    }

    /// Parses either supported format. See the crate docs for the layouts.
    pub fn parse(bytes: &[u8]) -> Result<Self, CorError> {
        parse::parse(bytes)
    }

    /// Format the table was read from (`Jcz21` for constructed tables).
    pub fn format(&self) -> CorFormat {
        self.format
    }

    /// Galvo units per mm stored in the file, if usable.
    pub fn scale_units_per_mm(&self) -> Option<f64> {
        self.scale_units_per_mm
    }

    /// Field size implied by the scale: 65535 / units-per-mm. 153.75 mm for the B6's 150 mm lens file.
    pub fn field_mm(&self) -> Option<f64> {
        self.scale_units_per_mm.map(|s| FIELD_SPAN_UNITS / s)
    }

    /// All (dx, dy) offsets in galvo units, row-major.
    pub fn nodes(&self) -> &[[f64; 2]] {
        &self.nodes
    }

    /// (dx, dy) at `row`, `col` (both 0..65), or `None` when out of range.
    pub fn node(&self, row: usize, col: usize) -> Option<[f64; 2]> {
        (row < GRID_SIZE && col < GRID_SIZE).then(|| self.nodes[row * GRID_SIZE + col])
    }

    /// Bytes stored after the grid by [`CorTable::write_jcz_2_1`]: the file's own trailer for parsed JCZ files,
    /// otherwise the i32 7 seen in EzCad output (meaning unknown).
    pub fn trailer(&self) -> &[u8] {
        &self.trailer
    }

    /// True when every offset is zero (uploading it is equivalent to no correction).
    pub fn is_identity(&self) -> bool {
        self.nodes.iter().all(|n| n[0] == 0.0 && n[1] == 0.0)
    }
}

pub(crate) fn valid_scale(s: f64) -> bool {
    s.is_finite() && s > 0.0
}

/// Offsets are sent as integers; this is the rounding meerk40t uses (Python `round`, ties to even).
pub(crate) fn round_offset(v: f64) -> i32 {
    // Callers only pass validated values, so the cast cannot saturate.
    v.round_ties_even() as i32
}

pub(crate) fn validate_nodes(nodes: &[[f64; 2]]) -> Result<(), CorError> {
    if nodes.len() != NODE_COUNT {
        return Err(CorError::NodeCount { expected: NODE_COUNT, actual: nodes.len() });
    }
    for (index, node) in nodes.iter().enumerate() {
        for &value in node {
            if !value.is_finite() || value.round_ties_even().abs() > f64::from(MAX_OFFSET) {
                return Err(CorError::OffsetOutOfRange { row: index / GRID_SIZE, col: index % GRID_SIZE, value });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_zero_and_full_size() {
        let t = CorTable::identity(Some(426.0));
        assert_eq!(t.nodes().len(), NODE_COUNT);
        assert!(t.is_identity());
        assert_eq!(t.node(64, 64), Some([0.0, 0.0]));
        assert_eq!(t.node(65, 0), None);
        assert_eq!(t.node(0, 65), None);
    }

    #[test]
    fn invalid_scale_is_dropped() {
        assert_eq!(CorTable::identity(Some(0.0)).field_mm(), None);
        assert_eq!(CorTable::identity(Some(f64::NAN)).field_mm(), None);
        assert_eq!(CorTable::identity(Some(-1.0)).scale_units_per_mm(), None);
        assert_eq!(CorTable::identity(Some(511.9921875)).field_mm(), Some(128.0));
    }

    #[test]
    fn from_nodes_validates() {
        assert!(matches!(CorTable::from_nodes(None, vec![]), Err(CorError::NodeCount { actual: 0, .. })));
        let mut nodes = vec![[0.0, 0.0]; NODE_COUNT];
        nodes[GRID_SIZE + 2] = [0.0, 32767.4];
        assert!(CorTable::from_nodes(None, nodes.clone()).is_ok());
        nodes[GRID_SIZE + 2] = [-32768.0, 0.0];
        assert!(matches!(
            CorTable::from_nodes(None, nodes.clone()),
            Err(CorError::OffsetOutOfRange { row: 1, col: 2, .. })
        ));
        nodes[GRID_SIZE + 2] = [f64::INFINITY, 0.0];
        assert!(CorTable::from_nodes(None, nodes).is_err());
    }

    #[test]
    fn rounding_is_ties_to_even() {
        assert_eq!(round_offset(2.5), 2);
        assert_eq!(round_offset(3.5), 4);
        assert_eq!(round_offset(-2.5), -2);
        assert_eq!(round_offset(-2.6), -3);
    }
}
