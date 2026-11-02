//! RDWorks `.rd` files (Ruida controller jobs, also LightBurn's "Save RD file") as [`Art`]: every layer's cut
//! polylines with the layer's speed, power and colour as a line layer.
//!
//! The file is read by the `ruida` crate (`ruida::rd::read_rd`: the scrambling key is detected, commands are parsed,
//! moves end a polyline and cuts extend it; facts from MeerK40t's MIT-licensed Ruida module). What an `.rd` file can
//! hold beyond that is not read: it is the controller's command stream, not a drawing, so curves arrive as the
//! polylines the writing program flattened them to, fills as their scan lines, and images as the scan lines of their
//! dots (each a polyline). Layer settings: the maximum power of laser 1 becomes the layer's power, the cutting speed
//! its speed; laser 2 and minimum power are reported in a warning when they differ.
//!
//! Coordinates are the controller's machine frame (mm from the origin corner, positive into the bed). The
//! [`RdFrame`] says which corner that is and, when known, the bed size, so the art comes out in OpenEtch's frame (+Y
//! away from the operator; centred on the bed when its size is given, else relative to the origin corner).

use doc::{CutSettings, FillMode};
use kurbo::{BezPath, Point};

use crate::{Art, ArtLayer, ArtPath, FileError, Result};

/// Largest `.rd` file read.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Most polylines imported.
pub const MAX_POLYLINES: usize = 200_000;

/// The machine frame of the file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RdFrame {
    /// The origin corner is on the right (X runs right to left).
    pub mirror_x: bool,
    /// The origin corner is at the rear (Y runs from the rear toward the operator).
    pub mirror_y: bool,
    /// The bed `[width, height]` mm: coordinates are then centred on the bed (the field frame of a gantry).
    pub bed_mm: Option<[f64; 2]>,
}

impl Default for RdFrame {
    /// The rear right origin MeerK40t's writer implies, no bed.
    fn default() -> Self {
        RdFrame { mirror_x: true, mirror_y: true, bed_mm: None }
    }
}

impl RdFrame {
    /// A machine point in mm to the art frame.
    pub fn to_art(&self, p: [f64; 2]) -> Point {
        let c = self.bed_mm.map_or([0.0, 0.0], |b| [b[0] / 2.0, b[1] / 2.0]);
        let sx = if self.mirror_x { -1.0 } else { 1.0 };
        let sy = if self.mirror_y { -1.0 } else { 1.0 };
        Point::new(sx * (p[0] - c[0]), sy * (p[1] - c[1]))
    }
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Read an `.rd` file into art. Refused when the key cannot be found, the file holds no cuts, or it is too large.
pub fn parse(bytes: &[u8], frame: RdFrame) -> Result<Art> {
    if bytes.len() > MAX_BYTES {
        return Err(FileError::TooLarge(format!("the .rd file is larger than {} MB", MAX_BYTES / (1024 * 1024))));
    }
    let read = ruida::rd::read_rd(bytes, None).map_err(|e| FileError::Invalid(format!("not a readable .rd file: {e}")))?;
    let mut art = Art::default();
    for w in &read.warnings {
        art.warn(w.clone());
    }
    let mut count = 0usize;
    for (i, l) in read.job.layers.iter().enumerate() {
        if l.polylines.is_empty() {
            continue;
        }
        let layer_index = art.layers.len();
        let speed = if l.speed_mm_s.is_finite() && l.speed_mm_s > 0.0 { l.speed_mm_s } else { CutSettings::default().speed_mm_s };
        if speed != l.speed_mm_s {
            art.warn(format!("RD layer {i} has no cutting speed; the default speed was used"));
        }
        let power = l.max_power[0].clamp(0.0, 100.0);
        if l.min_power[0] != l.max_power[0] {
            art.warn(format!(
                "RD layer {i}: minimum power {:.1} % (corners) differs from maximum {:.1} %; the maximum is used",
                l.min_power[0], l.max_power[0]
            ));
        }
        if l.max_power[1] > 0.0 && l.max_power[1] != l.max_power[0] {
            art.warn(format!("RD layer {i}: laser 2's power ({:.1} %) is not imported", l.max_power[1]));
        }
        art.layers.push(ArtLayer {
            name: format!("RD layer {i} ({})", hex(l.color)),
            color: l.color,
            settings: CutSettings { mode: FillMode::Line, power_pct: power, speed_mm_s: speed, ..CutSettings::default() },
            sub_layers: Vec::new(),
        });
        for pl in &l.polylines {
            count += 1;
            if count > MAX_POLYLINES {
                return Err(FileError::TooLarge(format!("the .rd file holds more than {MAX_POLYLINES} polylines")));
            }
            let mut path = BezPath::new();
            let mut pts = pl.iter().map(|p| frame.to_art(*p));
            let Some(first) = pts.next() else { continue };
            path.move_to(first);
            for p in pts {
                path.line_to(p);
            }
            if pl.len() > 2 && pl.first() == pl.last() {
                path.close_path();
            }
            art.paths.push(ArtPath { path, label: format!("RD layer {i}"), color: Some(l.color), layer: Some(layer_index) });
        }
    }
    if art.paths.is_empty() {
        return Err(FileError::Invalid("the .rd file holds no cuts".into()));
    }
    if read.unknown_commands > 0 {
        art.warn("commands this reader does not know were skipped (settings such as air assist, blow and frequencies are not imported)");
    }
    Ok(art)
}

#[cfg(test)]
mod tests {
    use kurbo::{PathEl, Shape};
    use ruida::rd::{RdJob, RdLayer, WriteOptions, write_rd};

    use super::*;

    fn job() -> RdJob {
        RdJob {
            layers: vec![
                RdLayer {
                    color: [255, 0, 0],
                    speed_mm_s: 120.0,
                    min_power: [35.0, 0.0],
                    max_power: [35.0, 0.0],
                    frequency: None,
                    polylines: vec![vec![[10.0, 10.0], [30.0, 10.0], [30.0, 20.0], [10.0, 10.0]]],
                },
                RdLayer {
                    color: [0, 0, 255],
                    speed_mm_s: 300.5,
                    min_power: [10.0, 0.0],
                    max_power: [12.5, 0.0],
                    frequency: None,
                    polylines: vec![vec![[100.0, 50.0], [100.0, 60.25]], vec![[101.0, 50.0], [101.0, 60.25]]],
                },
            ],
        }
    }

    fn points(a: &ArtPath) -> Vec<(f64, f64)> {
        a.path
            .elements()
            .iter()
            .filter_map(|e| match e {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some((p.x, p.y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn round_trip_with_the_writer() {
        let bytes = write_rd(&job(), &WriteOptions::default()).unwrap();
        // Front left origin, no bed: machine coordinates as they are.
        let art = parse(&bytes, RdFrame { mirror_x: false, mirror_y: false, bed_mm: None }).unwrap();
        assert_eq!(art.layers.len(), 2);
        assert_eq!(art.layers[0].color, [255, 0, 0]);
        assert!((art.layers[0].settings.power_pct - 35.0).abs() < 0.01);
        assert_eq!(art.layers[0].settings.speed_mm_s, 120.0);
        assert!((art.layers[1].settings.power_pct - 12.5).abs() < 0.01);
        assert_eq!(art.layers[1].settings.speed_mm_s, 300.5);
        assert_eq!(art.paths.len(), 3);
        assert_eq!(points(&art.paths[0]), vec![(10.0, 10.0), (30.0, 10.0), (30.0, 20.0), (10.0, 10.0)]);
        assert!(matches!(art.paths[0].path.elements().last(), Some(PathEl::ClosePath)));
        assert_eq!(points(&art.paths[2]), vec![(101.0, 50.0), (101.0, 60.25)]);
        assert_eq!(art.paths[1].layer, Some(1));
        assert!(art.warnings.iter().any(|w| w.contains("minimum power")));
    }

    #[test]
    fn the_frame_maps_corners_and_centres_on_the_bed() {
        let bytes = write_rd(&job(), &WriteOptions::default()).unwrap();
        // Rear right origin on a 600 x 400 bed: machine (10, 10) is 10 mm in from the right and the rear.
        let art = parse(&bytes, RdFrame { mirror_x: true, mirror_y: true, bed_mm: Some([600.0, 400.0]) }).unwrap();
        assert_eq!(points(&art.paths[0])[0], (290.0, 190.0));
        let b = art.paths[0].path.bounding_box();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (270.0, 180.0, 290.0, 190.0));
        // Without a bed, relative to the corner.
        let art = parse(&bytes, RdFrame::default()).unwrap();
        assert_eq!(points(&art.paths[0])[0], (-10.0, -10.0));
    }

    #[test]
    fn refusals() {
        assert!(parse(b"not an rd file at all", RdFrame::default()).is_err());
        let empty = write_rd(&RdJob::default(), &WriteOptions::default()).unwrap();
        assert!(parse(&empty, RdFrame::default()).is_err());
    }
}
