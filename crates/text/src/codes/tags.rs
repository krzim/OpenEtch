//! Fiducial tags: AprilTag 16h5, 25h9, 36h10 and 36h11 (codes from the AprilTag 3 library, BSD-2-Clause), ArUco
//! 7 x 7 (OpenCV's DICT_7X7_1000, Apache-2.0, whose first 50, 100 and 250 markers are DICT_7X7_50, _100 and _250) and
//! the ArUco library's ARUCO_MIP_36h12 (LightBurn's "ArUco MIP 36h11", identified from markers 0-2 of the owner's
//! LightBurn 2.2: they are not AprilTag 36h11 codes in any rotation or mirror, at least 5 bits off).
//! The data is the tag id. The symbol is the tag's black-bordered square (LightBurn's quiet zone adds the white border
//! around it): a dark border ring and the code's bits inside, a 0 bit dark (both libraries draw 1 bits white).

use super::Grid;
use super::apriltag_tables as t;
use super::aruco_tables;

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
pub const TAG36H10: Family =
    Family { name: "AprilTag 36h10", codes: &t::TAG36H10, bit_x: &t::TAG36_BIT_X, bit_y: &t::TAG36_BIT_Y, width: 8, min_distance: 10 };
pub const TAG36H11: Family =
    Family { name: "AprilTag 36h11", codes: &t::TAG36H11, bit_x: &t::TAG36_BIT_X, bit_y: &t::TAG36_BIT_Y, width: 8, min_distance: 11 };
/// ArUco 7 x 7 (OpenCV DICT_7X7_1000): a one-module border round 7 x 7 bits laid out row by row.
pub const ARUCO_7X7: Family = Family {
    name: "ArUco 7x7",
    codes: &aruco_tables::ARUCO_7X7,
    bit_x: &ARUCO_7X7_BITS.0,
    bit_y: &ARUCO_7X7_BITS.1,
    width: 9,
    min_distance: 14,
};

/// ARUCO_MIP_36h12 (LightBurn's "ArUco MIP 36h11"): a one-module border round 6 x 6 bits, the code's lowest bit at
/// the top-left, row by row.
pub const ARUCO_MIP_36H12: Family = Family {
    name: "ArUco MIP 36h11",
    codes: &aruco_tables::ARUCO_MIP_36H12,
    bit_x: &ARUCO_MIP_BITS.0,
    bit_y: &ARUCO_MIP_BITS.1,
    width: 8,
    min_distance: 12,
};

/// [`row_major`] with the bits in the other order: the code's first (most significant) bit at the bottom-right.
const fn row_major_lsb_first<const N: usize>(n: u8) -> ([u8; N], [u8; N]) {
    let (xs, ys) = row_major::<N>(n);
    let (mut rx, mut ry) = ([0; N], [0; N]);
    let mut i = 0;
    while i < N {
        rx[i] = xs[N - 1 - i];
        ry[i] = ys[N - 1 - i];
        i += 1;
    }
    (rx, ry)
}

const ARUCO_MIP_BITS: ([u8; 36], [u8; 36]) = row_major_lsb_first(6);

/// The (x, y) positions of an n x n bit square inside a one-module border, row by row from the top-left.
const fn row_major<const N: usize>(n: u8) -> ([u8; N], [u8; N]) {
    let (mut xs, mut ys) = ([0; N], [0; N]);
    let mut i = 0;
    while i < N {
        xs[i] = 1 + (i as u8) % n;
        ys[i] = 1 + (i as u8) / n;
        i += 1;
    }
    (xs, ys)
}

const ARUCO_7X7_BITS: ([u8; 49], [u8; 49]) = row_major(7);

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
        for f in [TAG16H5, TAG25H9, TAG36H10, TAG36H11, ARUCO_7X7, ARUCO_MIP_36H12] {
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

    /// The minimum distance among the first `n` codes in any rotation (OpenCV's smaller 7 x 7 dictionaries are the
    /// first 50, 100 and 250 markers of DICT_7X7_1000, with a larger distance).
    fn min_distance(f: &Family, n: usize) -> u32 {
        let rot: Vec<[u64; 4]> = f.codes[..n]
            .iter()
            .map(|c| {
                let g0 = f.code_grid(*c);
                let g1 = rotate(&g0);
                let g2 = rotate(&g1);
                [f.read(&g0), f.read(&g1), f.read(&g2), f.read(&rotate(&g2))]
            })
            .collect();
        let mut min = u32::MAX;
        for (i, a) in rot.iter().enumerate() {
            for r in 1..4 {
                min = min.min((a[0] ^ a[r]).count_ones());
            }
            for b in &rot[i + 1..] {
                min = min.min(b.iter().map(|rb| (a[0] ^ rb).count_ones()).min().unwrap());
            }
        }
        min
    }

    /// LightBurn 2.2's "ArUco MIP 36h11" markers 0, 1 and 2 (Create Bar Code, exported as SVG; `#` dark), as the
    /// owner's sample shows them: ARUCO_MIP_36h12's first three codes.
    #[test]
    fn aruco_mip_matches_lightburns_markers() {
        let lightburn = [
            ["########", "#.#...##", "##.#####", "##.#...#", "####..##", "#..#.#.#", "###.#..#", "########"],
            ["########", "#.#.##.#", "#..##.##", "#..##.##", "###.####", "########", "####..##", "########"],
            ["########", "##.##..#", "#.##...#", "#..#...#", "#..#..##", "######.#", "###.####", "########"],
        ];
        for (id, rows) in lightburn.iter().enumerate() {
            let want: Grid = rows.iter().map(|r| r.chars().map(|c| c == '#').collect()).collect();
            assert_eq!(ARUCO_MIP_36H12.grid(&id.to_string()).unwrap(), want, "marker {id}");
        }
        // None of them is an AprilTag 36h11 code, in any rotation.
        for rows in lightburn {
            let g: Grid = rows.iter().map(|r| r.chars().map(|c| c == '#').collect()).collect();
            let mut r = g.clone();
            for _ in 0..4 {
                assert!(!TAG36H11.codes.contains(&TAG36H11.read(&r)));
                r = rotate(&r);
            }
        }
        assert_eq!(ARUCO_MIP_36H12.codes.len(), 250);
        assert!(ARUCO_MIP_36H12.grid("250").is_err());
    }

    #[test]
    fn aruco_7x7_dictionaries() {
        // OpenCV's DICT_7X7_50 / _100 / _250 / _1000 are built with distances 19, 18, 17 and 14.
        for (n, d) in [(50, 19), (100, 18), (250, 17)] {
            let min = min_distance(&ARUCO_7X7, n);
            assert!(min >= d, "DICT_7X7_{n}: minimum distance {min}");
        }
        // Marker 0 of DICT_7X7_50 (OpenCV bytes 221, 92, 108, 165, 202, 10, 1): its top bit row is 1101110 (1 white,
        // so light, light, dark, light, light, light, dark inside the dark border).
        let g = ARUCO_7X7.grid("0").unwrap();
        assert_eq!(g.len(), 9);
        assert!(g[0].iter().all(|d| *d) && g[8].iter().all(|d| *d) && g.iter().all(|r| r[0] && r[8]));
        assert_eq!(g[1], [true, false, false, true, false, false, false, true, true]);
        // The last bit (bottom-right) is the low bit of OpenCV's 7th byte.
        assert!(!g[7][7]);
        assert_eq!(ARUCO_7X7.grid("999").unwrap().len(), 9);
        assert!(ARUCO_7X7.grid("1000").is_err());
    }

    #[test]
    fn tag_grids() {
        let g = TAG36H11.grid("0").unwrap();
        assert_eq!(g.len(), 8);
        assert!(g[0].iter().all(|d| *d) && g[7].iter().all(|d| *d));
        assert_eq!(TAG36H11.read(&g), 0xd7e00984b);
        assert!(TAG36H11.grid("587").is_err() && TAG16H5.grid("x").is_err());
        assert_eq!(TAG16H5.grid("29").unwrap().len(), 6);
        // tag36h10 id 0 is 0x1a42f9469 (tag36h10.c), its last id 2319.
        let g = TAG36H10.grid("0").unwrap();
        assert_eq!(g.len(), 8);
        assert_eq!(TAG36H10.read(&g), 0x1a42f9469);
        assert!(TAG36H10.grid("2319").is_ok() && TAG36H10.grid("2320").is_err());
    }
}
