//! Real AutoCAD-compiled SHX unifonts (laserfont2 and laserfont3, SIL OFL 1.1, see `fixtures/shx/README.md`). Their
//! glyphs were checked point for point against an independent decode of the `.shp` sources they were compiled from.

use kurbo::{PathEl, Point, Shape};

fn font(name: &str) -> text::stroke::StrokeFont {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shx").join(name);
    text::shx::parse(name, &std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn laserfont_unifonts_parse() {
    for name in ["laserfont2.shx", "laserfont3.shx"] {
        let f = font(name);
        assert_eq!(f.glyphs.len(), 95, "{name}");
        assert_eq!(f.cap_height, 20.0, "{name}");
        let a = f.glyph('A').unwrap();
        assert!((a.advance - 18.0).abs() < 1e-9, "{name}: {}", a.advance);
        // `*00041`: scale 1/100, then `8,43,115`: the first stroke leaves the origin by (0.43, 1.15).
        let pts: Vec<Point> = a
            .path
            .elements()
            .iter()
            .take(2)
            .map(|e| match e {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Point::new(p.x, p.y),
                e => panic!("{name}: unexpected {e:?}"),
            })
            .collect();
        assert!(pts[0].distance(Point::ORIGIN) < 1e-9 && pts[1].distance(Point::new(0.43, 1.15)) < 1e-9, "{name}: {pts:?}");
        for c in 'A'..='Z' {
            let b = f.glyph(c).unwrap().path.bounding_box();
            assert!(b.y0 >= -1e-6 && b.y1 <= f.cap_height + 1e-6, "{name} {c}: {b:?}");
        }
    }
}
