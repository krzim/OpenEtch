//! Barcode encoders OpenEtch writes itself, for the LightBurn barcode types rxing cannot encode (see `barcode`):
//! 2 of 5 Industrial / IATA, Plessey, the ISBN and VIN checks ([`linear`]), Micro QR and rMQR ([`qr_micro`]) and
//! AprilTag ([`tags`]).

mod apriltag_tables;
pub mod linear;
pub mod qr_micro;
pub mod tags;

/// A module grid: `grid[y][x]`, row 0 at the top, `true` dark.
pub type Grid = Vec<Vec<bool>>;
