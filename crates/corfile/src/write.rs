//! Writing `JCZ_COR_2_1` files.

use crate::parse::{JCZ_GRID_OFFSET, JCZ_LABEL, JCZ_NODE_LEN, JCZ_SCALE_OFFSET};
use crate::{CorTable, NODE_COUNT, round_offset};

impl CorTable {
    /// Serialises as `JCZ_COR_2_1`.
    ///
    /// For a table parsed from a JCZ file, `parse(write)` reproduces it exactly and the bytes match the original file
    /// (reserved header bytes and the trailer are kept). Tables from other sources get zero reserved bytes, the default
    /// trailer, offsets rounded ties-to-even, and a scale of 0.0 when they have none (which parses back as `None`).
    pub fn write_jcz_2_1(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(JCZ_GRID_OFFSET + NODE_COUNT * JCZ_NODE_LEN + self.trailer.len());
        out.extend_from_slice(JCZ_LABEL);
        out.extend_from_slice(&self.reserved);
        debug_assert_eq!(out.len(), JCZ_SCALE_OFFSET);
        out.extend_from_slice(&self.scale_units_per_mm.unwrap_or(0.0).to_le_bytes());
        debug_assert_eq!(out.len(), JCZ_GRID_OFFSET);
        for [dx, dy] in &self.nodes {
            out.extend_from_slice(&round_offset(*dx).to_le_bytes());
            out.extend_from_slice(&round_offset(*dy).to_le_bytes());
        }
        out.extend_from_slice(&self.trailer);
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::{CorFormat, CorTable, GRID_SIZE, NODE_COUNT};

    #[test]
    fn identity_round_trips() {
        let t = CorTable::identity(Some(426.2439024390244));
        let bytes = t.write_jcz_2_1();
        assert_eq!(bytes.len(), 33840);
        assert_eq!(CorTable::parse(&bytes), Ok(t));
    }

    #[test]
    fn constructed_table_round_trips() {
        let nodes: Vec<[f64; 2]> = (0..NODE_COUNT).map(|i| [i as f64 - 2000.0, -(i as f64) / 2.0 + 0.5]).collect();
        // Halves round to even on write, so compare against the rounded grid.
        let rounded: Vec<[f64; 2]> = nodes.iter().map(|n| [n[0].round_ties_even(), n[1].round_ties_even()]).collect();
        let t = CorTable::from_nodes(None, nodes).expect("valid");
        let back = CorTable::parse(&t.write_jcz_2_1()).expect("parse");
        assert_eq!(back.format(), CorFormat::Jcz21);
        assert_eq!(back.scale_units_per_mm(), None);
        assert_eq!(back.nodes(), rounded.as_slice());
        assert_eq!(back.node(1, 0), Some([GRID_SIZE as f64 - 2000.0, -32.0]));
    }
}
