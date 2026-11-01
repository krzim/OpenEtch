//! HPGL / PLT import (LightBurn's Import of `.plt` / `.hpgl` plotter files).
//!
//! Instructions are two letters followed by comma- or space-separated numbers and end at `;`, a newline or the next
//! instruction. Plotter units are 0.025 mm (40 per mm, LightBurn's default "HPGL precision"; `units_per_mm`
//! changes it). Read: `IN`/`DF` (reset), `PU`/`PD` (pen up/down, optionally with points), `PA`/`PR` (absolute and
//! relative coordinates), `SP` (pen number: each pen becomes its own labelled path set), `CI` (circle around the pen),
//! `AA`/`AR` (arcs), `EA`/`ER`/`RA`/`RR` (rectangles, edge or filled, at the pen), `EP`/`PM` (polygon mode is drawn as
//! outlines). Labels (`LB`), scaling (`IP`/`SC`), line types and encoded polylines (`PE`) are skipped with a warning.

use kurbo::{BezPath, Point, Vec2};

use crate::{Art, Result, invalid};

/// Default plotter units per millimetre.
pub const UNITS_PER_MM: f64 = 40.0;
/// Largest file read.
pub const MAX_BYTES: usize = 50 * 1024 * 1024;

struct State {
    pos: Point,
    down: bool,
    relative: bool,
    pen: u32,
    current: BezPath,
    open: bool,
}

impl State {
    fn flush(&mut self, art: &mut Art) {
        let path = std::mem::take(&mut self.current);
        art.push(path, format!("pen {}", self.pen), None);
        self.open = false;
    }

    fn line_to(&mut self, p: Point) {
        if !self.open {
            self.current.move_to(self.pos);
            self.open = true;
        }
        self.current.line_to(p);
        self.pos = p;
    }

    fn move_to(&mut self, p: Point) {
        if self.open {
            // Keep drawing into the same path set; a move starts a new subpath.
            self.open = false;
        }
        self.pos = p;
    }
}

/// Points of an arc around `c` from `start` sweeping `deg` (counter-clockwise for positive), one per `step` degrees.
fn arc_points(c: Point, start: Point, deg: f64, step: f64) -> Vec<Point> {
    let v = start - c;
    let r = v.hypot();
    let a0 = v.y.atan2(v.x);
    let n = ((deg.abs() / step.max(0.5)).ceil() as usize).clamp(1, 3600);
    (1..=n)
        .map(|i| {
            let a = a0 + (deg * i as f64 / n as f64).to_radians();
            c + Vec2::new(r * a.cos(), r * a.sin())
        })
        .collect()
}

/// Parse HPGL text into paths (mm, +Y up as plotters are).
pub fn parse(text: &str, units_per_mm: f64) -> Result<Art> {
    if !(units_per_mm.is_finite() && units_per_mm > 0.0) {
        return invalid("units per mm must be a positive number");
    }
    if text.len() > MAX_BYTES {
        return Err(crate::FileError::TooLarge(format!("the file is larger than {} MB", MAX_BYTES / 1024 / 1024)));
    }
    let k = 1.0 / units_per_mm;
    let mut art = Art::default();
    let mut st = State { pos: Point::ZERO, down: false, relative: false, pen: 1, current: BezPath::new(), open: false };
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut seen = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if !c.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() || !bytes[i + 1].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let op = [c.to_ascii_uppercase(), bytes[i + 1].to_ascii_uppercase()];
        i += 2;
        if &op == b"LB" {
            // Label text runs to the label terminator (ETX by default).
            while i < bytes.len() && bytes[i] != 0x03 {
                i += 1;
            }
            skipped.push("LB".into());
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_alphabetic() && bytes[i] != b';' {
            i += 1;
        }
        let args: Vec<f64> = text[start..i]
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .collect();
        seen += 1;
        let pt = |st: &State, x: f64, y: f64| if st.relative { st.pos + Vec2::new(x * k, y * k) } else { Point::new(x * k, y * k) };
        match &op {
            b"IN" | b"DF" => {
                st.flush(&mut art);
                st.relative = false;
                st.down = false;
                st.pos = Point::ZERO;
            }
            b"SP" => {
                let pen = args.first().map_or(0, |p| *p as u32);
                if pen != st.pen {
                    st.flush(&mut art);
                    st.pen = pen;
                }
            }
            b"PA" | b"PR" | b"PU" | b"PD" => {
                match &op {
                    b"PA" => st.relative = false,
                    b"PR" => st.relative = true,
                    b"PU" => st.down = false,
                    _ => st.down = true,
                }
                for [x, y] in args.as_chunks::<2>().0 {
                    let p = pt(&st, *x, *y);
                    if st.down { st.line_to(p) } else { st.move_to(p) }
                }
            }
            b"CI" => {
                let Some(&r) = args.first() else { continue };
                let r = r.abs() * k;
                let c = st.pos;
                let pts = arc_points(c, c + Vec2::new(r, 0.0), 360.0, args.get(1).copied().unwrap_or(5.0));
                let mut p = BezPath::new();
                p.move_to(c + Vec2::new(r, 0.0));
                for q in pts {
                    p.line_to(q);
                }
                p.close_path();
                art.push(p, format!("pen {}", st.pen), None);
            }
            b"AA" | b"AR" => {
                if args.len() < 3 {
                    continue;
                }
                let c = if &op == b"AA" { Point::new(args[0] * k, args[1] * k) } else { st.pos + Vec2::new(args[0] * k, args[1] * k) };
                let pts = arc_points(c, st.pos, args[2], args.get(3).copied().unwrap_or(5.0));
                for q in pts {
                    if st.down { st.line_to(q) } else { st.move_to(q) }
                }
            }
            b"EA" | b"ER" | b"RA" | b"RR" => {
                if args.len() < 2 {
                    continue;
                }
                let corner = if op[1] == b'A' { Point::new(args[0] * k, args[1] * k) } else { st.pos + Vec2::new(args[0] * k, args[1] * k) };
                let mut p = BezPath::new();
                p.move_to(st.pos);
                p.line_to((corner.x, st.pos.y));
                p.line_to(corner);
                p.line_to((st.pos.x, corner.y));
                p.close_path();
                art.push(p, format!("pen {}", st.pen), None);
            }
            b"PM" | b"EP" | b"FP" | b"LT" | b"VS" | b"WU" | b"PW" | b"SI" | b"DI" | b"DT" | b"SC" | b"IP" | b"PE" | b"CS"
            | b"FT" | b"BP" | b"PS" | b"RO" | b"PG" | b"SS" | b"SA" | b"NP" | b"PC" | b"TR" | b"MC" | b"QL" | b"EC" => {
                if matches!(&op, b"SC" | b"IP" | b"PE" | b"RO") {
                    skipped.push(String::from_utf8_lossy(&op).into_owned());
                }
            }
            _ => skipped.push(String::from_utf8_lossy(&op).into_owned()),
        }
        if i < bytes.len() && bytes[i] == b';' {
            i += 1;
        }
    }
    st.flush(&mut art);
    if seen == 0 {
        return invalid("no HPGL instructions found");
    }
    skipped.sort();
    skipped.dedup();
    if !skipped.is_empty() {
        art.warn(format!("skipped HPGL instructions OpenEtch does not read: {}", skipped.join(", ")));
    }
    if art.paths.is_empty() {
        return invalid("the HPGL file draws nothing");
    }
    Ok(art)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn pens_lines_and_units() {
        let art = parse("IN;SP1;PU0,0;PD400,0,400,400;PU;SP2;PA800,800;PD;PR40,0,0,40;SP0;", 40.0).unwrap();
        assert_eq!(art.paths.len(), 2);
        assert_eq!(art.paths[0].label, "pen 1");
        assert_eq!(art.paths[0].path.bounding_box(), kurbo::Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(art.paths[1].path.bounding_box(), kurbo::Rect::new(20.0, 20.0, 21.0, 21.0));
    }

    #[test]
    fn circles_arcs_rectangles_and_skips() {
        let art = parse("IN;PA400,400;CI200;EA800,800;PD;AA400,400,90;LBhello\u{3};SC0,1,0,1;", 40.0).unwrap();
        let b = art.paths[0].path.bounding_box();
        assert!((b.width() - 10.0).abs() < 1e-6 && (b.center().x - 10.0).abs() < 1e-6, "{b:?}");
        assert_eq!(art.paths[1].path.bounding_box(), kurbo::Rect::new(10.0, 10.0, 20.0, 20.0));
        assert!(art.warnings[0].contains("LB") && art.warnings[0].contains("SC"), "{:?}", art.warnings);
        assert!(parse("hello", 40.0).is_err());
    }
}
