//! Fiducial tags: AprilTag 16h5, 25h9, 36h10 and 36h11 (codes from the AprilTag 3 library, BSD-2-Clause). The data is
//! the tag id. The symbol is the tag's black-bordered square (LightBurn's quiet zone adds the white border around
//! it): a dark border ring and the code's bits inside, a 0 bit dark (AprilTag draws 1 bits white).

use super::Grid;
use super::apriltag_tables as t;

/// A tag family: its codes, bit positions inside the bordered square and that square's width.
pub struct Family {
    pub name: &'static str,
    pub codes: &'static [u64],
    pub bit_x: &'static [u8],
    pub bit_y: &'static [u8],
    /// Width of the black-bordered square in modules.
    pub width: usize,
    /// Minimum Hamming distance between any two codes in any rotation.
    pub min_distance: u32,
}

pub const TAG16H5: Family =
    Family { name: "AprilTag 16h5", codes: &t::TAG16H5, bit_x: &t::TAG16H5_BIT_X, bit_y: &t::TAG16H5_BIT_Y, width: 6, min_distance: 5 };
pub const TAG25H9: Family =
    Family { name: "AprilTag 25h9", codes: &t::TAG25H9, bit_x: &t::TAG25H9_BIT_X, bit_y: &t::TAG25H9_BIT_Y, width: 7, min_distance: 9 };
pub const TAG36H11: Family =
    Family { name: "AprilTag 36h11", codes: &t::TAG36H11, bit_x: &t::TAG36_BIT_X, bit_y: &t::TAG36_BIT_Y, width: 8, min_distance: 11 };

impl Family {
    /// The tag with id `data` (a whole number 0..codes).
    pub fn grid(&self, data: &str) -> Result<Grid, String> {
        let n = self.codes.len();
        let id: usize = data
            .trim()
            .parse()
            .ok()
            .filter(|i| *i < n)
            .ok_or_else(|| format!("{} needs a tag id 0 to {} (got {data:?})", self.name, n - 1))?;
        Ok(self.code_grid(self.codes[id]))
    }

    fn code_grid(&self, code: u64) -> Grid {
        let w = self.width;
        let mut g = vec![vec![false; w]; w];
        for (y, row) in g.iter_mut().enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                *cell = x == 0 || y == 0 || x == w - 1 || y == w - 1;
            }
        }
        let nbits = self.bit_x.len();
        for i in 0..nbits {
            let one = (code >> (nbits - 1 - i)) & 1 == 1;
            g[usize::from(self.bit_y[i])][usize::from(self.bit_x[i])] = !one;
        }
        g
    }

    /// The code a grid shows (the inverse of [`Family::code_grid`]).
    #[cfg(test)]
    fn read(&self, g: &Grid) -> u64 {
        let nbits = self.bit_x.len();
        (0..nbits).fold(0, |a, i| (a << 1) | u64::from(!g[usize::from(self.bit_y[i])][usize::from(self.bit_x[i])]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotate(g: &Grid) -> Grid {
        let w = g.len();
        (0..w).map(|y| (0..w).map(|x| g[w - 1 - x][y]).collect()).collect()
    }

    /// The families' defining property: any two codes, in any of the four rotations, differ in at least
    /// `min_distance` bits (a mistyped code would almost surely break it).
    #[test]
    fn families_keep_their_hamming_distance() {
        for f in [TAG16H5, TAG25H9, TAG36H11] {
            let rotations: Vec<[u64; 4]> = f
                .codes
                .iter()
                .map(|c| {
                    let g0 = f.code_grid(*c);
                    let g1 = rotate(&g0);
                    let g2 = rotate(&g1);
                    let g3 = rotate(&g2);
                    [f.read(&g0), f.read(&g1), f.read(&g2), f.read(&g3)]
                })
                .collect();
            let mut min = u32::MAX;
            for (i, a) in rotations.iter().enumerate() {
                assert_eq!(a[0], f.codes[i]);
                for r in 1..4 {
                    min = min.min((a[0] ^ a[r]).count_ones());
                }
                for b in &rotations[i + 1..] {
                    for rb in b {
                        min = min.min((a[0] ^ rb).count_ones());
                    }
                }
            }
            assert!(min >= f.min_distance, "{}: minimum distance {min}", f.name);
        }
    }

    #[test]
    fn tag_grids() {
        let g = TAG36H11.grid("0").unwrap();
        assert_eq!(g.len(), 8);
        assert!(g[0].iter().all(|d| *d) && g[7].iter().all(|d| *d));
        assert_eq!(TAG36H11.read(&g), 0xd7e00984b);
        assert!(TAG36H11.grid("587").is_err() && TAG16H5.grid("x").is_err());
        assert_eq!(TAG16H5.grid("29").unwrap().len(), 6);
    }
}
