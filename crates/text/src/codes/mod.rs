//! Barcode encoders OpenEtch writes itself, for the LightBurn barcode types rxing cannot encode (see `barcode`):
//! 2 of 5 Industrial / IATA, Plessey, the ISBN and VIN checks ([`linear`]), Micro QR and rMQR ([`qr_micro`]),
//! PDF417 Micro ([`micro_pdf417`]), Chinese Sensible Code ([`hanxin`]), Grid Matrix ([`grid_matrix`]) and the
//! AprilTag and ArUco fiducial tags ([`tags`]).

mod apriltag_tables;
mod aruco_tables;
pub mod grid_matrix;
pub mod hanxin;
mod hanxin_tables;
pub mod linear;
pub mod micro_pdf417;
pub mod qr_micro;
pub mod rs;
pub mod tags;
#[cfg(test)]
mod zint_tests;

/// A module grid: `grid[y][x]`, row 0 at the top, `true` dark.
pub type Grid = Vec<Vec<bool>>;
