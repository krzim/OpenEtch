//! Host-side bilinear correction, for previews and diagnostics only. The controller applies the uploaded table in
//! hardware; nothing here feeds the job encoder.
//!
//! **Unverified against hardware.** Assumptions:
//! - The 65 nodes per side span the whole field evenly: node spacing is `field_mm / 64` (65535 / 64 galvo units), node 0 at
//!   `-field_mm / 2` and node 64 at `+field_mm / 2`.
//! - Coordinates are in the lens frame, centred, before any profile axis swap/negate: x runs along columns, y along
//!   rows, row 0 at -y. Evidence from `jcz150.cor`: dx is 0 down the centre column and takes the sign of the column
//!   offset, dy is 0 along the centre row and takes the sign of the row offset (an outward, pincushion-style pull).
//! - Offsets are added to the requested position: the result is where the controller is assumed to drive the beam.
//! - Outside the field the nearest edge offsets are used (no extrapolation).

use crate::{CorTable, FIELD_SPAN_UNITS, GRID_SIZE, valid_scale};

/// Cells per side (65 nodes, 64 gaps).
const CELLS: usize = GRID_SIZE - 1;

impl CorTable {
    /// Applies the table to a lens-frame point in mm using the file's own field size. Returns the point unchanged when
    /// the table has no scale. See the module docs for the (unverified) conventions.
    pub fn correct(&self, x_mm: f64, y_mm: f64) -> (f64, f64) {
        match self.field_mm() {
            Some(field_mm) => self.correct_in_field(field_mm, x_mm, y_mm),
            None => (x_mm, y_mm),
        }
    }

    /// As [`CorTable::correct`], with an explicit field size (e.g. from the machine profile). Returns the point
    /// unchanged when `field_mm` is not finite and positive.
    pub fn correct_in_field(&self, field_mm: f64, x_mm: f64, y_mm: f64) -> (f64, f64) {
        if !valid_scale(field_mm) || !x_mm.is_finite() || !y_mm.is_finite() {
            return (x_mm, y_mm);
        }
        let [dx, dy] = self.offset_units_at(grid_coord(field_mm, x_mm), grid_coord(field_mm, y_mm));
        let mm_per_unit = field_mm / FIELD_SPAN_UNITS;
        (x_mm + dx * mm_per_unit, y_mm + dy * mm_per_unit)
    }

    /// Bilinear (dx, dy) in galvo units at fractional grid position (`col`, `row`), each in 0..=64.
    fn offset_units_at(&self, col: f64, row: f64) -> [f64; 2] {
        let (c0, tc) = split(col);
        let (r0, tr) = split(row);
        let at = |r: usize, c: usize| self.nodes[r * GRID_SIZE + c];
        let (n00, n01, n10, n11) = (at(r0, c0), at(r0, c0 + 1), at(r0 + 1, c0), at(r0 + 1, c0 + 1));
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let mix = |i: usize| lerp(lerp(n00[i], n01[i], tc), lerp(n10[i], n11[i], tc), tr);
        [mix(0), mix(1)]
    }
}

/// Lens-frame mm to a fractional node index, clamped to the grid.
fn grid_coord(field_mm: f64, v_mm: f64) -> f64 {
    ((v_mm / field_mm + 0.5) * CELLS as f64).clamp(0.0, CELLS as f64)
}

/// Fractional index to (cell, t) with the cell in 0..64 so `cell + 1` is always a node.
fn split(v: f64) -> (usize, f64) {
    // v is clamped to 0..=64, so the cast is exact and in range.
    let cell = (v.floor() as usize).min(CELLS - 1);
    (cell, v - cell as f64)
}

#[cfg(test)]
mod tests {
    use crate::{CorTable, GRID_SIZE, NODE_COUNT};

    /// dx = col - 32, dy = 2 * (row - 32): linear, so bilinear interpolation is exact.
    fn linear() -> CorTable {
        let nodes = (0..NODE_COUNT)
            .map(|i| [(i % GRID_SIZE) as f64 - 32.0, 2.0 * ((i / GRID_SIZE) as f64 - 32.0)])
            .collect();
        CorTable::from_nodes(Some(511.9921875), nodes).expect("valid") // 128 mm field, 2 mm node spacing
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn identity_does_nothing() {
        assert_eq!(CorTable::identity(Some(511.9921875)).correct(12.5, -40.0), (12.5, -40.0));
    }

    #[test]
    fn no_scale_does_nothing() {
        let t = CorTable::from_nodes(None, linear().nodes().to_vec()).expect("valid");
        assert_eq!(t.correct(10.0, 10.0), (10.0, 10.0));
        assert_eq!(linear().correct_in_field(0.0, 1.0, 2.0), (1.0, 2.0));
    }

    #[test]
    fn interpolates_linear_grid_exactly() {
        let t = linear();
        let per_unit = 128.0 / 65535.0;
        // x = 3 mm is column 33.5 → dx 1.5 units; y = -5 mm is row 29.5 → dy -5 units.
        assert!(close(t.correct(3.0, -5.0), (3.0 + 1.5 * per_unit, -5.0 - 5.0 * per_unit)));
        assert!(close(t.correct(0.0, 0.0), (0.0, 0.0)));
    }

    #[test]
    fn hits_nodes_and_corners() {
        let t = linear();
        let per_unit = 128.0 / 65535.0;
        assert!(close(t.correct(-64.0, -64.0), (-64.0 - 32.0 * per_unit, -64.0 - 64.0 * per_unit)));
        assert!(close(t.correct(64.0, 64.0), (64.0 + 32.0 * per_unit, 64.0 + 64.0 * per_unit)));
    }

    #[test]
    fn clamps_outside_the_field() {
        let t = linear();
        let per_unit = 128.0 / 65535.0;
        assert!(close(t.correct(100.0, -100.0), (100.0 + 32.0 * per_unit, -100.0 - 64.0 * per_unit)));
        assert_eq!(t.correct(f64::NAN, 0.0).1, 0.0);
    }
}
