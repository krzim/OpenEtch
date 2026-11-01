//! `VertList`/`PrimList` text and their exact conversion to and from kurbo paths.
//!
//! The compact-token scanner and the "`c0x1` without `c0y` means no control point" rule are ported from
//! openlaser-rs `crates/import/src/lbrn2.rs` (https://github.com/luizbueno3d/openlaser-rs @ 9d81a15,
//! MIT OR Apache-2.0, Copyright (c) the openlaser-rs authors), and re-checked against the owner's samples: every
//! `B` primitive in them uses full `cNx`/`cNy` pairs and the bare form only ever appears as `c0x1`/`c1x1`.

use kurbo::{BezPath, PathEl, Point};

use crate::LbrnError;
use crate::model::{PathData, Prim, Prims, Vertex, num};

fn malformed(m: impl Into<String>) -> LbrnError {
    LbrnError::Malformed(m.into())
}

/// A cursor over compact path text. Tokens are not separated: `V1 2c0x1c1x3c1y4` is one vertex.
struct Scan<'a>(&'a str);

impl Scan<'_> {
    fn skip_space(&mut self) {
        self.0 = self.0.trim_start();
    }

    fn take(&mut self, prefix: &str) -> bool {
        self.skip_space();
        match self.0.strip_prefix(prefix) {
            Some(rest) => {
                self.0 = rest;
                true
            }
            None => false,
        }
    }

    fn token(&mut self) -> Result<&str, LbrnError> {
        self.skip_space();
        let len = self
            .0
            .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')))
            .unwrap_or(self.0.len());
        if len == 0 {
            let near: String = self.0.chars().take(12).collect();
            return Err(malformed(format!("expected a number in path data near '{near}'")));
        }
        let (t, rest) = self.0.split_at(len);
        self.0 = rest;
        Ok(t)
    }

    fn num(&mut self) -> Result<f64, LbrnError> {
        let t = self.token()?;
        t.parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| malformed(format!("'{t}' is not a finite number")))
    }

    fn index(&mut self) -> Result<usize, LbrnError> {
        let t = self.token()?;
        t.parse().map_err(|_| malformed(format!("'{t}' is not a vertex index")))
    }

    fn at_end(&mut self) -> bool {
        self.skip_space();
        self.0.is_empty()
    }
}

/// Parse a `VertList`, refusing more than `max` vertices.
pub(crate) fn parse_vertices(text: &str, max: usize) -> Result<Vec<Vertex>, LbrnError> {
    let mut s = Scan(text);
    let mut out = Vec::new();
    while !s.at_end() {
        if out.len() >= max {
            return Err(LbrnError::TooLarge(format!("more than {max} path vertices")));
        }
        if !s.take("V") {
            return Err(malformed("VertList entries must start with V"));
        }
        let p = Point::new(s.num()?, s.num()?);
        let mut c = [None; 4];
        while s.take("c") {
            let slot = if s.take("0x") {
                0
            } else if s.take("0y") {
                1
            } else if s.take("1x") {
                2
            } else if s.take("1y") {
                3
            } else {
                return Err(malformed("unknown control point component in VertList"));
            };
            if c[slot].replace(s.num()?).is_some() {
                return Err(malformed("duplicate control point component in VertList"));
            }
        }
        out.push(Vertex { p, c0: control(c[0], c[1])?, c1: control(c[2], c[3])? });
    }
    Ok(out)
}

/// A full `x`/`y` pair is a control point; nothing, or the bare flag `cNx1`, means "no control point".
fn control(x: Option<f64>, y: Option<f64>) -> Result<Option<Point>, LbrnError> {
    match (x, y) {
        (Some(x), Some(y)) => Ok(Some(Point::new(x, y))),
        (None, None) => Ok(None),
        (Some(1.0), None) => Ok(None),
        _ => Err(malformed("incomplete control point in VertList (only the flag cNx1 may omit cNy)")),
    }
}

/// Parse a `PrimList` whose indices must be below `vertices`, refusing more than `max` segments. Returns the
/// primitives and their segment count.
pub(crate) fn parse_prims(text: &str, vertices: usize, max: usize) -> Result<(Prims, usize), LbrnError> {
    let too_many = || LbrnError::TooLarge(format!("more than {max} path segments (a shared list counts once per use)"));
    let whole = match text.trim() {
        "LineClosed" => Some((Prims::LineClosed, vertices)),
        "LineOpen" => Some((Prims::LineOpen, vertices.saturating_sub(1))),
        _ => None,
    };
    if let Some((prims, n)) = whole {
        return if n > max { Err(too_many()) } else { Ok((prims, n)) };
    }
    let mut s = Scan(text);
    let mut out = Vec::new();
    while !s.at_end() {
        if out.len() >= max {
            return Err(too_many());
        }
        let bezier = if s.take("L") {
            false
        } else if s.take("B") {
            true
        } else {
            let near: String = s.0.chars().take(12).collect();
            return Err(malformed(format!("unsupported PrimList primitive near '{near}'")));
        };
        let (from, to) = (s.index()?, s.index()?);
        if from >= vertices || to >= vertices {
            return Err(malformed(format!("PrimList refers to vertex {} of {vertices}", from.max(to))));
        }
        out.push(Prim { bezier, from, to });
    }
    let n = out.len();
    Ok((Prims::List(out), n))
}

pub(crate) fn write_vertices(v: &[Vertex]) -> String {
    let mut s = String::with_capacity(v.len() * 24);
    for v in v {
        s.push('V');
        s.push_str(&num(v.p.x));
        s.push(' ');
        s.push_str(&num(v.p.y));
        for (tag, c) in [("c0", v.c0), ("c1", v.c1)] {
            match c {
                Some(c) => {
                    s.push_str(&format!("{tag}x{}{tag}y{}", num(c.x), num(c.y)));
                }
                None => {
                    s.push_str(tag);
                    s.push_str("x1");
                }
            }
        }
    }
    s
}

pub(crate) fn write_prims(p: &Prims) -> String {
    match p {
        Prims::LineClosed => "LineClosed".into(),
        Prims::LineOpen => "LineOpen".into(),
        Prims::List(list) => {
            let mut s = String::with_capacity(list.len() * 8);
            for p in list {
                s.push(if p.bezier { 'B' } else { 'L' });
                s.push_str(&format!("{} {}", p.from, p.to));
            }
            s
        }
    }
}

impl PathData {
    /// The path in its own (pre-XForm) coordinates. A primitive that does not start where the previous one ended
    /// starts a new subpath; one that ends on its subpath's first vertex closes it (a closing line becomes the
    /// implicit line of `ClosePath`).
    pub fn to_bez(&self) -> BezPath {
        let v = &self.vertices;
        let mut path = BezPath::new();
        let prims = match &self.prims {
            Prims::List(l) => l,
            Prims::LineClosed | Prims::LineOpen => {
                let mut it = v.iter();
                if let Some(first) = it.next() {
                    path.move_to(first.p);
                    for x in it {
                        path.line_to(x.p);
                    }
                    if self.prims == Prims::LineClosed {
                        path.close_path();
                    }
                }
                return path;
            }
        };
        let (mut current, mut start) = (None, None);
        for p in prims {
            let (Some(a), Some(b)) = (v.get(p.from), v.get(p.to)) else { continue };
            if current != Some(p.from) {
                path.move_to(a.p);
                start = Some(p.from);
            }
            let closing = start == Some(p.to);
            if p.bezier {
                path.curve_to(a.c0.unwrap_or(a.p), b.c1.unwrap_or(b.p), b.p);
            } else if !closing {
                path.line_to(b.p);
            }
            if closing {
                path.close_path();
                (current, start) = (None, None);
            } else {
                current = Some(p.to);
            }
        }
        path
    }

    /// Encode a kurbo path. Lines and cubics are exact; quadratics are raised to cubics (also exact). Each subpath
    /// gets its own vertices, so `to_bez` reads the same subpaths back.
    pub fn from_bez(path: &BezPath) -> PathData {
        let mut v: Vec<Vertex> = Vec::new();
        let mut prims: Vec<Prim> = Vec::new();
        let vertex = |p| Vertex { p, c0: None, c1: None };
        // Subpath start index, current index, number of prims when the subpath started.
        let mut start: Option<usize> = None;
        let mut cur: Option<usize> = None;
        let mut sub_prims = 0;
        let mut last_start_point = Point::ZERO;
        for el in path.elements() {
            // Begin a subpath implicitly when a segment follows a ClosePath without a MoveTo.
            if !matches!(el, PathEl::MoveTo(_) | PathEl::ClosePath) && cur.is_none() {
                drop_empty(&mut v, start, prims.len(), sub_prims);
                v.push(vertex(last_start_point));
                (start, cur, sub_prims) = (Some(v.len() - 1), Some(v.len() - 1), prims.len());
            }
            match *el {
                PathEl::MoveTo(p) => {
                    drop_empty(&mut v, start, prims.len(), sub_prims);
                    v.push(vertex(p));
                    (start, cur, sub_prims) = (Some(v.len() - 1), Some(v.len() - 1), prims.len());
                    last_start_point = p;
                }
                PathEl::LineTo(p) => {
                    let Some(c) = cur else { continue };
                    v.push(vertex(p));
                    prims.push(Prim { bezier: false, from: c, to: v.len() - 1 });
                    cur = Some(v.len() - 1);
                }
                PathEl::QuadTo(q, p) => {
                    let Some(c) = cur else { continue };
                    let p0 = v[c].p;
                    let (a, b) = (p0 + (q - p0) * (2.0 / 3.0), p + (q - p) * (2.0 / 3.0));
                    cubic(&mut v, &mut prims, c, a, b, p);
                    cur = Some(v.len() - 1);
                }
                PathEl::CurveTo(a, b, p) => {
                    let Some(c) = cur else { continue };
                    cubic(&mut v, &mut prims, c, a, b, p);
                    cur = Some(v.len() - 1);
                }
                PathEl::ClosePath => {
                    if let (Some(s), Some(c)) = (start, cur)
                        && s != c
                    {
                        let ends_on_last = prims.last().is_some_and(|p| p.to == c) && c == v.len() - 1;
                        if ends_on_last && v[c].p == v[s].p {
                            // The last segment already returns to the start: end it on the start vertex instead.
                            v[s].c1 = v[c].c1;
                            if let Some(last) = prims.last_mut() {
                                last.to = s;
                            }
                            v.pop();
                        } else {
                            prims.push(Prim { bezier: false, from: c, to: s });
                        }
                    }
                    cur = None;
                }
            }
        }
        drop_empty(&mut v, start, prims.len(), sub_prims);
        PathData { vertices: v, prims: Prims::List(prims), encoded: None }
    }
}

fn cubic(v: &mut Vec<Vertex>, prims: &mut Vec<Prim>, from: usize, a: Point, b: Point, p: Point) {
    v[from].c0 = Some(a);
    v.push(Vertex { p, c0: None, c1: Some(b) });
    prims.push(Prim { bezier: true, from, to: v.len() - 1 });
}

/// Remove the start vertex of a subpath that produced no primitives (a bare MoveTo).
fn drop_empty(v: &mut Vec<Vertex>, start: Option<usize>, prims_now: usize, prims_at_start: usize) {
    if let Some(s) = start
        && prims_now == prims_at_start
        && s + 1 == v.len()
    {
        v.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compact_vertices_and_shorthand() {
        let v = parse_vertices("V1 2c0x1c1x1V-3.5 4e-1c0x5c0y6c1x7c1y8 V0 0", 10).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], Vertex { p: Point::new(1.0, 2.0), c0: None, c1: None });
        assert_eq!(v[1].p, Point::new(-3.5, 0.4));
        assert_eq!(v[1].c0, Some(Point::new(5.0, 6.0)));
        assert_eq!(v[1].c1, Some(Point::new(7.0, 8.0)));
        assert!(parse_vertices("V1 2c0x3", 10).is_err(), "bare cNx other than 1");
        assert!(parse_vertices("V1 2c0y3", 10).is_err());
        assert!(parse_vertices("V1", 10).is_err());
        assert!(parse_vertices("X1 2", 10).is_err());
        assert!(parse_vertices("V1 nan", 10).is_err());
        assert!(matches!(parse_vertices("V1 2V3 4", 1), Err(LbrnError::TooLarge(_))));
    }

    #[test]
    fn parses_prims_and_checks_indices() {
        assert_eq!(parse_prims(" LineClosed ", 2, 9).unwrap(), (Prims::LineClosed, 2));
        let p = parse_prims("L0 1B1 2L2 0", 3, 9).unwrap();
        assert_eq!(
            p,
            (
                Prims::List(vec![
                    Prim { bezier: false, from: 0, to: 1 },
                    Prim { bezier: true, from: 1, to: 2 },
                    Prim { bezier: false, from: 2, to: 0 },
                ]),
                3
            )
        );
        assert!(parse_prims("L0 3", 3, 9).is_err());
        assert!(parse_prims("Q0 1", 3, 9).is_err());
        assert!(parse_prims("L0", 3, 9).is_err());
        assert!(matches!(parse_prims("L0 1L1 2L2 0", 3, 2), Err(LbrnError::TooLarge(_))));
        assert!(matches!(parse_prims("LineOpen", 5, 3), Err(LbrnError::TooLarge(_))));
        assert_eq!(parse_prims("LineOpen", 5, 4).unwrap().1, 4);
    }

    #[test]
    fn subpaths_and_closure() {
        // Square closed by returning to vertex 0, then an open polyline on its own vertices.
        let d = PathData {
            vertices: parse_vertices("V0 0V10 0V10 10V0 10V20 0V30 0V30 10", 99).unwrap(),
            prims: parse_prims("L0 1L1 2L2 3L3 0L4 5L5 6", 7, 99).unwrap().0,
            encoded: None,
        };
        let b = d.to_bez();
        let els = b.elements();
        assert_eq!(els.iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count(), 2);
        assert_eq!(els.iter().filter(|e| matches!(e, PathEl::ClosePath)).count(), 1);
        assert_eq!(els[4], PathEl::ClosePath, "closing line is the implicit ClosePath line");
        let back = PathData::from_bez(&b);
        assert_eq!(back.to_bez(), b);
        assert_eq!(back, d);
    }

    #[test]
    fn cubic_and_quad_encode_exactly() {
        let mut b = BezPath::new();
        b.move_to((0.0, 0.0));
        b.curve_to((1.0, 2.0), (3.0, 2.0), (4.0, 0.0));
        b.quad_to((2.0, -3.0), (0.0, 0.0));
        b.close_path();
        let d = PathData::from_bez(&b);
        assert_eq!(d.vertices.len(), 2, "the closing curve ends on the start vertex");
        let text = (write_vertices(&d.vertices), write_prims(&d.prims));
        let parsed = PathData {
            vertices: parse_vertices(&text.0, 99).unwrap(),
            prims: parse_prims(&text.1, d.vertices.len(), 99).unwrap().0,
            encoded: None,
        };
        assert_eq!(parsed, d);
        let back = parsed.to_bez();
        let segs: Vec<_> = back.segments().collect();
        assert_eq!(segs.len(), 2);
        // The quadratic's midpoint survives the degree raise.
        let q = kurbo::QuadBez::new((4.0, 0.0), (2.0, -3.0), (0.0, 0.0));
        use kurbo::ParamCurve;
        assert!((segs[1].eval(0.5) - q.eval(0.5)).hypot() < 1e-12);
    }

    #[test]
    fn line_open_and_degenerate_input() {
        let d = PathData { vertices: parse_vertices("V0 0V1 0V1 1", 9).unwrap(), prims: Prims::LineOpen, encoded: None };
        assert_eq!(d.to_bez().elements().len(), 3);
        let mut b = BezPath::new();
        b.move_to((5.0, 5.0));
        b.move_to((0.0, 0.0));
        b.line_to((1.0, 0.0));
        b.close_path();
        b.line_to((0.0, 1.0));
        let d = PathData::from_bez(&b);
        assert_eq!(d.vertices.len(), 4, "the bare MoveTo is dropped; the segment after Close restarts at the start");
        assert_eq!(d.to_bez().segments().count(), 3);
    }
}
