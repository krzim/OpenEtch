//! G-code import (LightBurn's Import of `.gcode` / `.nc` files, roadmap UX-036): the cutting moves of a laser, plotter
//! or CNC program as paths.
//!
//! Read: comments (`(...)`, `;` to the end of the line), `%` and line numbers (`N`) skipped, spaces ignored, any
//! letter case; `G0` (travel), `G1` (line), `G2`/`G3` (arcs in the XY plane by centre offsets `I`/`J` or radius
//! `R`, flattened to lines within 0.01 mm), `G20`/`G21` (inches / mm), `G90`/`G91` (absolute / incremental),
//! `G17` (XY plane), `M3`/`M4`/`M5` and `S` (laser power). Z, feeds, dwells and the other codes are ignored; a few
//! that move the work frame (`G92`, `G28`/`G30`, `G53`-`G59`, arcs in other planes) are applied as best possible and
//! reported in a warning.
//!
//! Which moves cut: when the program turns a spindle or laser on anywhere (`M3`/`M4`), a feed move cuts only while it
//! is on with `S > 0`; a program without `M3`/`M4` (a plotter's or a router's) cuts with every feed move. Cuts are
//! grouped by power: one path set per `S` value (labelled `S 500`, or `cut` without power), each move chain a
//! subpath. Coordinates are the program's (mm, +Y away from the operator, the art's +Y up). Written from the G-code
//! language as GRBL's and LinuxCNC's documentation describe it; no code was taken from either.

use std::collections::BTreeMap;

use kurbo::{BezPath, Point, Vec2};

use crate::{Art, Result, invalid};

/// Largest file read.
pub const MAX_BYTES: usize = 50 * 1024 * 1024;
/// Most points an import may produce (arcs included).
pub const MAX_POINTS: usize = 5_000_000;
/// Largest chord error when flattening arcs, mm.
const ARC_TOLERANCE_MM: f64 = 0.01;

/// One word: a letter and its number.
#[derive(Debug, Clone, Copy)]
struct Word {
    letter: u8,
    value: f64,
}

fn is_code(w: &Word, letter: u8, code: f64) -> bool {
    w.letter == letter && (w.value - code).abs() < 1e-6
}

/// The words of one line, comments removed; `Err` for a malformed number.
fn words(line: &str) -> std::result::Result<Vec<Word>, String> {
    let mut clean = Vec::with_capacity(line.len());
    let mut in_comment = false;
    for b in line.bytes() {
        match b {
            b'(' => in_comment = true,
            b')' => in_comment = false,
            b';' if !in_comment => break,
            _ if in_comment => {}
            b' ' | b'\t' | b'\r' | b'%' => {}
            _ => clean.push(b.to_ascii_uppercase()),
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < clean.len() {
        let letter = clean[i];
        if !letter.is_ascii_uppercase() {
            return Err(format!("unexpected character '{}'", letter as char));
        }
        i += 1;
        let start = i;
        while i < clean.len() && (clean[i].is_ascii_digit() || matches!(clean[i], b'.' | b'-' | b'+')) {
            i += 1;
        }
        let text = std::str::from_utf8(&clean[start..i]).unwrap_or("");
        let value: f64 = text.parse().map_err(|_| format!("'{}' has no number", letter as char))?;
        if !value.is_finite() {
            return Err(format!("'{}{text}' is not a finite number", letter as char));
        }
        out.push(Word { letter, value });
    }
    Ok(out)
}

/// The cut paths being built, one per power.
#[derive(Default)]
struct Cuts {
    /// Key: power in thousandths of S (or `i64::MIN` for "cut" without power).
    by_power: BTreeMap<i64, BezPath>,
    /// The chain being drawn: its power key and last point.
    open: Option<(i64, Point)>,
    points: usize,
}

impl Cuts {
    fn line(&mut self, key: i64, from: Point, to: Point) -> Result<()> {
        if (to - from).hypot() < 1e-9 {
            return Ok(());
        }
        let path = self.by_power.entry(key).or_default();
        if self.open.is_none_or(|(k, last)| k != key || (last - from).hypot() > 1e-9) {
            path.move_to(from);
            self.points += 1;
        }
        path.line_to(to);
        self.points += 1;
        self.open = Some((key, to));
        if self.points > MAX_POINTS {
            return Err(crate::FileError::TooLarge(format!("the program draws more than {MAX_POINTS} points")));
        }
        Ok(())
    }

    fn break_chain(&mut self) {
        self.open = None;
    }
}

/// Points along an arc in the XY plane from `from` to `to` around `c` (clockwise for `cw`), `to` included. A full
/// circle when `from` and `to` coincide.
fn arc(from: Point, to: Point, c: Point, cw: bool) -> Vec<Point> {
    let r = (from - c).hypot();
    let a0 = (from.y - c.y).atan2(from.x - c.x);
    let a1 = (to.y - c.y).atan2(to.x - c.x);
    let tau = std::f64::consts::TAU;
    let mut sweep = if cw { a0 - a1 } else { a1 - a0 };
    sweep = sweep.rem_euclid(tau);
    if sweep < 1e-9 {
        sweep = tau;
    }
    // Chord error r(1 - cos(step/2)) <= tolerance.
    let step = if r > ARC_TOLERANCE_MM { 2.0 * (1.0 - ARC_TOLERANCE_MM / r).acos() } else { tau };
    let n = ((sweep / step.max(1e-3)).ceil() as usize).clamp(1, 100_000);
    let dir = if cw { -1.0 } else { 1.0 };
    let mut pts: Vec<Point> = (1..n)
        .map(|i| {
            let a = a0 + dir * sweep * i as f64 / n as f64;
            c + Vec2::new(r * a.cos(), r * a.sin())
        })
        .collect();
    pts.push(to);
    pts
}

/// The centre of an `R`-form arc from `a` to `b` (radius `r`, negative for the longer arc), or `None` when no circle
/// of that radius joins them.
fn arc_center_r(a: Point, b: Point, r: f64, cw: bool) -> Option<Point> {
    let d = b - a;
    let chord = d.hypot();
    if chord < 1e-9 || r.abs() * 2.0 < chord - 1e-6 {
        return None;
    }
    let h = (r * r - chord * chord / 4.0).max(0.0).sqrt();
    let mid = a + d * 0.5;
    let n = Vec2::new(-d.y, d.x) / chord;
    // For a short arc (r > 0) the centre is right of the travel direction for clockwise arcs, left for
    // counter-clockwise; a negative radius picks the other side (the long arc).
    let left = !cw;
    let side = if (r > 0.0) == left { 1.0 } else { -1.0 };
    Some(mid + n * (h * side))
}

/// Parse G-code text into paths (mm, +Y up).
pub fn parse(text: &str) -> Result<Art> {
    if text.len() > MAX_BYTES {
        return Err(crate::FileError::TooLarge(format!("the file is larger than {} MB", MAX_BYTES / 1024 / 1024)));
    }
    // Whether the program switches a spindle or laser at all (decides which feed moves cut).
    let mut uses_spindle = false;
    for line in text.lines() {
        if let Ok(ws) = words(line)
            && ws.iter().any(|w| is_code(w, b'M', 3.0) || is_code(w, b'M', 4.0))
        {
            uses_spindle = true;
            break;
        }
    }
    let mut art = Art::default();
    let mut cuts = Cuts::default();
    let (mut inches, mut incremental, mut xy_plane) = (false, false, true);
    let (mut motion, mut spindle_on, mut s) = (None::<u8>, false, 0.0f64);
    let mut pos = Point::ZERO;
    // Work offset added by G92 (approximated: applied to later coordinates).
    let mut offset = Vec2::ZERO;
    let mut codes_seen = 0usize;
    let mut bad_lines = 0usize;
    let mut skipped: Vec<&'static str> = Vec::new();
    for line in text.lines() {
        let ws = match words(line) {
            Ok(ws) => ws,
            Err(_) => {
                bad_lines += 1;
                continue;
            }
        };
        if ws.is_empty() {
            continue;
        }
        let (mut x, mut y, mut i, mut j, mut r) = (None, None, None, None, None);
        let mut non_modal_skip = false;
        let mut set_offset = false;
        for w in &ws {
            match w.letter {
                b'G' => {
                    codes_seen += 1;
                    match w.value {
                        v if (v - 0.0).abs() < 1e-6 => motion = Some(0),
                        v if (v - 1.0).abs() < 1e-6 => motion = Some(1),
                        v if (v - 2.0).abs() < 1e-6 => motion = Some(2),
                        v if (v - 3.0).abs() < 1e-6 => motion = Some(3),
                        v if (v - 17.0).abs() < 1e-6 => xy_plane = true,
                        v if (v - 18.0).abs() < 1e-6 || (v - 19.0).abs() < 1e-6 => xy_plane = false,
                        v if (v - 20.0).abs() < 1e-6 => inches = true,
                        v if (v - 21.0).abs() < 1e-6 => inches = false,
                        v if (v - 90.0).abs() < 1e-6 => incremental = false,
                        v if (v - 91.0).abs() < 1e-6 => incremental = true,
                        v if (v - 92.0).abs() < 1e-6 => set_offset = true,
                        v if [28.0, 30.0, 53.0].iter().any(|c| (v - c).abs() < 1e-6) => {
                            non_modal_skip = true;
                            skipped.push("G28/G30/G53 (machine-frame moves)");
                        }
                        v if (54.0..=59.0).contains(&v) => skipped.push("G54-G59 (work coordinate systems)"),
                        _ => {}
                    }
                }
                b'M' => {
                    codes_seen += 1;
                    if is_code(w, b'M', 3.0) || is_code(w, b'M', 4.0) {
                        spindle_on = true;
                    } else if is_code(w, b'M', 5.0) || is_code(w, b'M', 2.0) || is_code(w, b'M', 30.0) {
                        spindle_on = false;
                    }
                }
                b'S' => s = w.value,
                b'X' => x = Some(w.value),
                b'Y' => y = Some(w.value),
                b'I' => i = Some(w.value),
                b'J' => j = Some(w.value),
                b'R' => r = Some(w.value),
                _ => {}
            }
        }
        let k = if inches { 25.4 } else { 1.0 };
        if set_offset {
            // G92: the current position is renamed to the given coordinates.
            let want = Point::new(x.map_or(pos.x - offset.x, |v| v * k), y.map_or(pos.y - offset.y, |v| v * k));
            offset = pos - want;
            skipped.push("G92 (coordinate offset, applied)");
            continue;
        }
        if non_modal_skip || (x.is_none() && y.is_none() && i.is_none() && j.is_none()) {
            continue;
        }
        let target = if incremental {
            pos + Vec2::new(x.unwrap_or(0.0) * k, y.unwrap_or(0.0) * k)
        } else {
            Point::new(x.map_or(pos.x, |v| v * k + offset.x), y.map_or(pos.y, |v| v * k + offset.y))
        };
        let cutting = spindle_on && s > 0.0 || !uses_spindle;
        let key = if uses_spindle { (s * 1000.0).round() as i64 } else { i64::MIN };
        match motion {
            Some(1) if cutting => cuts.line(key, pos, target)?,
            Some(2 | 3) if cutting && xy_plane => {
                let cw = motion == Some(2);
                let center = match r {
                    Some(r) if i.is_none() && j.is_none() => arc_center_r(pos, target, r * k, cw),
                    _ => Some(pos + Vec2::new(i.unwrap_or(0.0) * k, j.unwrap_or(0.0) * k)),
                };
                match center {
                    Some(c) => {
                        let mut from = pos;
                        for p in arc(pos, target, c, cw) {
                            cuts.line(key, from, p)?;
                            from = p;
                        }
                    }
                    None => {
                        skipped.push("arcs whose radius cannot reach their end (drawn as lines)");
                        cuts.line(key, pos, target)?;
                    }
                }
            }
            Some(2 | 3) if cutting => {
                skipped.push("arcs outside the XY plane (drawn as lines)");
                cuts.line(key, pos, target)?;
            }
            _ => cuts.break_chain(),
        }
        pos = target;
    }
    if codes_seen == 0 && cuts.by_power.is_empty() {
        return invalid("no G-code found");
    }
    if bad_lines > 0 {
        art.warn(format!("skipped {bad_lines} line(s) that are not G-code"));
    }
    skipped.sort_unstable();
    skipped.dedup();
    if !skipped.is_empty() {
        art.warn(format!("approximated or skipped: {}", skipped.join(", ")));
    }
    for (key, path) in cuts.by_power {
        let label = if key == i64::MIN { "cut".to_owned() } else { format!("S {}", number(key as f64 / 1000.0)) };
        art.push(path, label, None);
    }
    if art.paths.is_empty() {
        return invalid("the G-code program cuts nothing (no feed moves with the laser on)");
    }
    Ok(art)
}

fn number(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn bbox(art: &Art, i: usize) -> kurbo::Rect {
        art.paths[i].path.bounding_box()
    }

    #[test]
    fn laser_moves_by_power_travel_skipped() {
        let g = "; OpenEtch\nG90 G21\nM4 S0\nG0 X10 Y10\nG1 X20 Y10 S500 F1200\nG1 Y20\nG0 X50 Y50\nG1 X60 S250\nM5\nG1 X0 Y0\n";
        let art = parse(g).unwrap();
        assert_eq!(art.paths.len(), 2, "{:?}", art.paths);
        assert_eq!(art.paths[0].label, "S 250");
        assert_eq!(bbox(&art, 0), kurbo::Rect::new(50.0, 50.0, 60.0, 50.0));
        assert_eq!(art.paths[1].label, "S 500");
        assert_eq!(bbox(&art, 1), kurbo::Rect::new(10.0, 10.0, 20.0, 20.0));
        // A move with S0 or after M5 does not cut.
        let off = parse("M3 S0\nG1 X10 Y0 F100\n").unwrap_err();
        assert!(off.to_string().contains("cuts nothing"), "{off}");
    }

    #[test]
    fn plotter_programs_inches_incremental_and_comments() {
        // No M3/M4: every feed move cuts.
        let art = parse("G20 (inches)\nG91\nG1 X1 Y0\ng1 y1 ; up\nN10 G0 X1\nG1 X1\n%\n").unwrap();
        assert_eq!(art.paths.len(), 1);
        assert_eq!(art.paths[0].label, "cut");
        let b = bbox(&art, 0);
        assert!((b.x1 - 76.2).abs() < 1e-9 && (b.y1 - 25.4).abs() < 1e-9, "{b:?}");
        assert_eq!(art.paths[0].path.elements().iter().filter(|e| matches!(e, kurbo::PathEl::MoveTo(_))).count(), 2);
        assert!(parse("hello world").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn arcs_by_offset_and_radius() {
        // A full circle of radius 10 around (10, 0) by I/J.
        let art = parse("M3 S100\nG0 X0 Y0\nG2 X0 Y0 I10 J0 F100\n").unwrap();
        let b = bbox(&art, 0);
        assert!((b.width() - 20.0).abs() < 0.02 && (b.height() - 20.0).abs() < 0.02, "{b:?}");
        // A quarter arc counter-clockwise from (10, 0) to (0, 10) around the origin, by R.
        let art = parse("M3 S100\nG0 X10 Y0\nG3 X0 Y10 R10\n").unwrap();
        let b = bbox(&art, 0);
        assert!((b.x0).abs() < 1e-6 && (b.y0).abs() < 1e-6 && (b.x1 - 10.0).abs() < 1e-6 && (b.y1 - 10.0).abs() < 1e-6, "{b:?}");
        // Every flattened point is on the circle.
        for el in art.paths[0].path.elements() {
            if let kurbo::PathEl::LineTo(p) = el {
                assert!((p.to_vec2().hypot() - 10.0).abs() < 1e-6);
            }
        }
        // Clockwise the same way goes the long way round (three quarters).
        let art = parse("M3 S100\nG0 X10 Y0\nG2 X0 Y10 I-10 J0\n").unwrap();
        let b = bbox(&art, 0);
        assert!((b.x0 + 10.0).abs() < 0.02 && (b.y0 + 10.0).abs() < 0.02, "{b:?}");
    }

    #[test]
    fn openetch_output_reads_back() {
        let g = "G90 G21\nM4 S0\nG0 X190 Y140\nG1 X210 Y140 S500 F1200\nG1 X210 Y160\nG1 X200 Y170\nG0 X200 Y150\nG1 X205 Y150 S255 F600\nM5\n";
        let art = parse(g).unwrap();
        assert_eq!(art.paths.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["S 255", "S 500"]);
        assert_eq!(bbox(&art, 1), kurbo::Rect::new(190.0, 140.0, 210.0, 170.0));
    }

    #[test]
    fn work_offsets_are_reported() {
        let art = parse("G0 X10 Y10\nG92 X0 Y0\nG1 X5 Y0\nG28\n").unwrap();
        assert_eq!(bbox(&art, 0), kurbo::Rect::new(10.0, 10.0, 15.0, 10.0));
        assert!(art.warnings.iter().any(|w| w.contains("G92") && w.contains("G28")), "{:?}", art.warnings);
    }
}
