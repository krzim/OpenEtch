//! Text auto-fit ([`doc::TextFit`], like xTool Studio's Auto Fit): after layout and bend, the text's ink is scaled
//! into a box or a circle. The box sits on the text's origin by its alignment (Align X left: the box's left edge on
//! the origin, middle: its centre, right: its right edge; Align Y top / middle / bottom likewise) and the scaled ink
//! is placed in it the same way, so a left-aligned name grows to the right, as typed. A circle is centred on the
//! origin and the ink's bounding-box centre goes to its centre; the scale is the largest at which every point of the
//! outline lies inside the circle (a coin's face), not the box's corners.
//!
//! - `contain` scales up or down, keeping the proportions, until the ink touches the area;
//! - `stretch` (boxes) scales width and height separately so the ink fills the box exactly;
//! - `shrink` is `contain` that never enlarges: text that already fits keeps its size (and its place).

use doc::{AlignH, AlignV, FitMode, FitShape, Text, TextFit};
use kurbo::{Affine, BezPath, Point, Rect};

/// The fit area in the text's own frame: the box, or the circle's bounding square.
pub fn area(t: &Text, f: &TextFit) -> Rect {
    let (w, h) = match f.shape {
        FitShape::Box => (f.width_mm, f.height_mm),
        FitShape::Circle => (f.width_mm, f.width_mm),
    };
    if f.shape == FitShape::Circle {
        return Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0);
    }
    let x0 = match t.align_h {
        AlignH::Left => 0.0,
        AlignH::Middle => -w / 2.0,
        AlignH::Right => -w,
    };
    let y0 = match t.align_v {
        AlignV::Top => -h,
        AlignV::Middle => -h / 2.0,
        AlignV::Bottom => 0.0,
    };
    Rect::new(x0, y0, x0 + w, y0 + h)
}

/// The points of `path` (curves flattened finely enough for a fit), for the circle test.
fn points(path: &BezPath, tolerance: f64) -> Vec<Point> {
    let mut out = Vec::new();
    kurbo::flatten(path.elements().iter().copied(), tolerance, |el| match el {
        kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => out.push(p),
        _ => {}
    });
    out
}

/// `outline` (the text's own frame) scaled into `t.fit`. Returns the new outline and the scale `[x, y]` it applied
/// (`[1, 1]` without a fit or without ink).
pub fn apply(t: &Text, outline: BezPath) -> (BezPath, [f64; 2]) {
    let Some(f) = t.fit.as_ref().filter(|f| f.problem().is_none()) else { return (outline, [1.0, 1.0]) };
    let Some(ink) = geom::bounds(&outline) else { return (outline, [1.0, 1.0]) };
    let (iw, ih) = (ink.width(), ink.height());
    if iw <= 1e-9 && ih <= 1e-9 {
        return (outline, [1.0, 1.0]);
    }
    let a = area(t, f);
    let (kx, ky) = match f.shape {
        FitShape::Circle => {
            let c = ink.center();
            let tol = (iw.max(ih) * 1e-4).max(1e-5);
            let far = points(&outline, tol).iter().map(|p| (*p - c).hypot()).fold(0.0, f64::max);
            if far <= 1e-12 {
                return (outline, [1.0, 1.0]);
            }
            let k = a.width() / 2.0 / far;
            (k, k)
        }
        FitShape::Box => {
            // A line of only dashes has no height (and "|" no width): the other side decides.
            let sx = if iw > 1e-9 { a.width() / iw } else { f64::INFINITY };
            let sy = if ih > 1e-9 { a.height() / ih } else { f64::INFINITY };
            match f.mode {
                FitMode::Stretch => (if sx.is_finite() { sx } else { 1.0 }, if sy.is_finite() { sy } else { 1.0 }),
                _ => {
                    let k = sx.min(sy);
                    (k, k)
                }
            }
        }
    };
    let (kx, ky) = if f.mode == FitMode::Shrink {
        if kx >= 1.0 && ky >= 1.0 {
            return (outline, [1.0, 1.0]);
        }
        (kx.min(1.0), ky.min(1.0))
    } else {
        (kx, ky)
    };
    if !(kx.is_finite() && ky.is_finite() && kx > 0.0 && ky > 0.0) {
        return (outline, [1.0, 1.0]);
    }
    let (w, h) = (iw * kx, ih * ky);
    // Where the scaled ink's lower-left corner goes.
    let (x, y) = match f.shape {
        FitShape::Circle => (-w / 2.0, -h / 2.0),
        FitShape::Box => (
            match t.align_h {
                AlignH::Left => a.x0,
                AlignH::Middle => a.center().x - w / 2.0,
                AlignH::Right => a.x1 - w,
            },
            match t.align_v {
                AlignV::Bottom => a.y0,
                AlignV::Middle => a.center().y - h / 2.0,
                AlignV::Top => a.y1 - h,
            },
        ),
    };
    let tf = Affine::translate((x, y)) * Affine::scale_non_uniform(kx, ky) * Affine::translate((-ink.x0, -ink.y0));
    (tf * outline, [kx, ky])
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::FontSpec;

    fn rect_path(r: Rect) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((r.x0, r.y0));
        p.line_to((r.x1, r.y0));
        p.line_to((r.x1, r.y1));
        p.line_to((r.x0, r.y1));
        p.close_path();
        p
    }

    fn text(fit: TextFit, h: AlignH, v: AlignV) -> Text {
        let mut t = Text::new("x", FontSpec::system("Any"), 5.0);
        t.fit = Some(fit);
        t.align_h = h;
        t.align_v = v;
        t
    }

    fn close(a: Rect, b: Rect) -> bool {
        (a.x0 - b.x0).abs() < 1e-9 && (a.x1 - b.x1).abs() < 1e-9 && (a.y0 - b.y0).abs() < 1e-9 && (a.y1 - b.y1).abs() < 1e-9
    }

    #[test]
    fn box_contain_grows_and_aligns() {
        let ink = rect_path(Rect::new(1.0, 2.0, 11.0, 4.0)); // 10 x 2
        let fit = TextFit { shape: FitShape::Box, width_mm: 40.0, height_mm: 20.0, mode: FitMode::Contain };
        let (out, k) = apply(&text(fit, AlignH::Middle, AlignV::Middle), ink.clone());
        assert_eq!(k, [4.0, 4.0]);
        assert!(close(geom::bounds(&out).unwrap(), Rect::new(-20.0, -4.0, 20.0, 4.0)));
        // Left / top: the box hangs right of and below the origin, the ink in its top-left corner.
        let (out, _) = apply(&text(fit, AlignH::Left, AlignV::Top), ink.clone());
        assert!(close(geom::bounds(&out).unwrap(), Rect::new(0.0, -8.0, 40.0, 0.0)));
        let (out, _) = apply(&text(fit, AlignH::Right, AlignV::Bottom), ink);
        assert!(close(geom::bounds(&out).unwrap(), Rect::new(-40.0, 0.0, 0.0, 8.0)));
    }

    #[test]
    fn box_stretch_fills_and_shrink_only_shrinks() {
        let ink = rect_path(Rect::new(0.0, 0.0, 10.0, 2.0));
        let stretch = TextFit { shape: FitShape::Box, width_mm: 30.0, height_mm: 12.0, mode: FitMode::Stretch };
        let (out, k) = apply(&text(stretch, AlignH::Middle, AlignV::Middle), ink.clone());
        assert_eq!(k, [3.0, 6.0]);
        assert!(close(geom::bounds(&out).unwrap(), Rect::new(-15.0, -6.0, 15.0, 6.0)));
        let shrink = TextFit { mode: FitMode::Shrink, ..stretch };
        let (out, k) = apply(&text(shrink, AlignH::Middle, AlignV::Middle), ink.clone());
        assert_eq!(k, [1.0, 1.0], "already fits: unchanged");
        assert_eq!(out, ink);
        let small = TextFit { width_mm: 5.0, ..shrink };
        let (_, k) = apply(&text(small, AlignH::Middle, AlignV::Middle), ink);
        assert_eq!(k, [0.5, 0.5]);
    }

    #[test]
    fn circle_uses_the_outline_not_its_corners() {
        // A plus sign: its bounding box corners are empty, so it fits larger than the box would allow.
        let mut plus = rect_path(Rect::new(-5.0, -1.0, 5.0, 1.0));
        plus.extend(rect_path(Rect::new(-1.0, -5.0, 1.0, 5.0)).iter());
        let fit = TextFit { shape: FitShape::Circle, width_mm: 20.0, height_mm: 0.0, mode: FitMode::Contain };
        let (out, k) = apply(&text(fit, AlignH::Left, AlignV::Top), Affine::translate((30.0, 7.0)) * plus);
        let far = 26f64.sqrt();
        assert!((k[0] - 10.0 / far).abs() < 1e-9, "{k:?}");
        let b = geom::bounds(&out).unwrap();
        assert!(b.center().to_vec2().hypot() < 1e-9, "centred on the origin whatever the alignment");
        assert!(b.width() > 20.0 / 2f64.sqrt(), "bigger than the inscribed square");
        assert!(points(&out, 1e-4).iter().all(|p| p.to_vec2().hypot() <= 10.0 + 1e-9));
    }

    #[test]
    fn no_fit_no_ink_and_bad_values_change_nothing() {
        let ink = rect_path(Rect::new(0.0, 0.0, 10.0, 2.0));
        let mut t = Text::new("x", FontSpec::system("Any"), 5.0);
        assert_eq!(apply(&t, ink.clone()).1, [1.0, 1.0]);
        t.fit = Some(TextFit { shape: FitShape::Box, width_mm: 10.0, height_mm: 0.0, mode: FitMode::Contain });
        assert!(t.problem().is_some(), "a box needs a height");
        assert_eq!(apply(&t, ink).1, [1.0, 1.0]);
        t.fit = Some(TextFit { shape: FitShape::Circle, width_mm: 10.0, height_mm: 0.0, mode: FitMode::Stretch });
        assert!(t.problem().unwrap().contains("stretch"));
        t.fit = Some(TextFit { shape: FitShape::Box, width_mm: 10.0, height_mm: 4.0, mode: FitMode::Contain });
        assert_eq!(apply(&t, BezPath::new()).1, [1.0, 1.0]);
        // A dash has no height: the width decides.
        let mut dash = BezPath::new();
        dash.move_to((0.0, 0.0));
        dash.line_to((2.0, 0.0));
        assert_eq!(apply(&t, dash).1, [5.0, 5.0]);
    }
}
