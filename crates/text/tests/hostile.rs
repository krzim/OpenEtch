//! Hostile input for the font parsers, the layout and variable text: nothing may panic or run for long.

use std::time::Duration;

use doc::{FontKind, FontSpec, Text};

/// Run `f` on a thread and fail when it takes longer than `limit`.
fn within<T: Send + 'static>(limit: Duration, what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(limit) {
        Ok(v) => v,
        Err(_) => panic!("{what} took longer than {limit:?}"),
    }
}

/// An `AutoCAD-86 shapes 1.0` file of `(code, spec)` shapes (shape 0 is the font's description).
fn shapes_file(shapes: &[(u16, &[u8])]) -> Vec<u8> {
    let mut d = b"AutoCAD-86 shapes 1.0\r\n\x1A".to_vec();
    let defs: Vec<Vec<u8>> = shapes
        .iter()
        .map(|(_, spec)| {
            let mut v = b"N\0".to_vec();
            v.extend_from_slice(spec);
            v
        })
        .collect();
    d.extend(0u16.to_le_bytes());
    d.extend(shapes.iter().map(|s| s.0).max().unwrap().to_le_bytes());
    d.extend((shapes.len() as u16).to_le_bytes());
    for ((code, _), def) in shapes.iter().zip(&defs) {
        d.extend(code.to_le_bytes());
        d.extend((def.len() as u16).to_le_bytes());
    }
    for def in defs {
        d.extend(def);
    }
    d
}

/// A tiny xorshift generator, so the fuzz runs are repeatable.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn shx_scale_codes_cannot_blow_up_arcs() {
    // Scale up by 255 forty times, then draw an octant arc and a bulge: the radius would be infinite (or ~1e96), and
    // flattening such an arc into cubics never ends.
    let mut spec = vec![1u8];
    for _ in 0..40 {
        spec.extend([4, 255]);
    }
    spec.extend([10, 255, 0x00, 12, 10, 10, 60, 11, 0, 0, 0, 255, 0x00, 0x40, 0]);
    let mut tiny = vec![1u8];
    for _ in 0..200 {
        tiny.extend([3, 255]);
    }
    tiny.extend([10, 255, 0x00, 0x18, 0]);
    let f = shapes_file(&[(0, &[21, 7, 0, 0]), (u16::from(b'A'), &spec), (u16::from(b'B'), &tiny), (u16::from(b'C'), &[1, 0x18, 0])]);
    let font = within(Duration::from_secs(10), "parsing an SHX font with huge scale factors", move || text::shx::parse("t", &f));
    let font = font.expect("the font still parses");
    for g in font.glyphs.values() {
        assert!(geom::is_finite(&g.path) && g.advance.is_finite(), "glyph outlines stay finite");
    }
    assert!(font.glyph('C').is_some());
}

#[test]
fn shx_and_jsf_parsers_survive_random_bytes() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let base_shx = shapes_file(&[
        (0, &[21, 7, 0, 0]),
        (u16::from(b'A'), &[1, 0x84, 10, 3, 0x12, 12, 4, 4, 100, 13, 2, 2, 50, 0, 0, 7, b'B', 0]),
        (u16::from(b'B'), &[1, 9, 1, 1, 2, 2, 0, 0, 11, 10, 20, 0, 5, 0x21, 14, 8, 1, 1, 0]),
    ]);
    let base_jsf = jsf_sample();
    within(Duration::from_secs(120), "fuzzing the SHX and JSF parsers", move || {
        for round in 0..3000u32 {
            let base = if round.is_multiple_of(2) { &base_shx } else { &base_jsf };
            let mut d = base.clone();
            let flips = 1 + (rng.next() % 16) as usize;
            for _ in 0..flips {
                let at = (rng.next() as usize) % d.len();
                d[at] = rng.next() as u8;
            }
            if rng.next().is_multiple_of(4) {
                let keep = (rng.next() as usize) % d.len();
                d.truncate(keep);
            }
            let _ = text::shx::parse("f", &d);
            let _ = text::jsf::parse("f", &d);
            let random: Vec<u8> = (0..(rng.next() % 512)).map(|_| rng.next() as u8).collect();
            let mut r = b"AutoCAD-86 unifont 1.0\r\n\x1A".to_vec();
            r.extend(&random);
            let _ = text::shx::parse("f", &r);
            let mut j = b"JCZ SINGLE FONT\0".to_vec();
            j.extend(&random);
            let _ = text::jsf::parse("f", &j);
        }
    });
}

/// A JSF in the documented layout with glyphs `A` and `B`.
fn jsf_sample() -> Vec<u8> {
    let mut d = vec![0xCCu8; 0x11A];
    d[..16].copy_from_slice(b"JCZ SINGLE FONT\0");
    let put = |d: &mut Vec<u8>, o: usize, v: [u8; 4]| d[o..o + 4].copy_from_slice(&v);
    for o in [0x9E, 0xA2, 0xA6, 0xAA] {
        put(&mut d, o, 32f32.to_le_bytes());
    }
    let entries = 0x43u32;
    put(&mut d, 0xAE, entries.to_le_bytes());
    put(&mut d, 0xB2, 0x11Au32.to_le_bytes());
    put(&mut d, 0xB6, 80f32.to_le_bytes());
    put(&mut d, 0xBA, 20f32.to_le_bytes());
    put(&mut d, 0xBE, 3.2f32.to_le_bytes());
    d.resize(0x11A + 4 * entries as usize, 0);
    for code in [0x41usize, 0x42] {
        let at = d.len() as u32;
        d[0x11A + 4 * code..0x11A + 4 * code + 4].copy_from_slice(&at.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.push(2);
        d.extend(3u16.to_le_bytes());
        for (x, y) in [(0f32, 0f32), (10.0, 26.0), (20.0, 0.0)] {
            d.extend(x.to_le_bytes());
            d.extend(y.to_le_bytes());
        }
        d.extend([0u8; 34]);
    }
    d
}

#[test]
fn serial_formats_with_huge_widths_are_quick() {
    let format = format!("0{}", "d".repeat(200_000));
    let out = within(Duration::from_secs(5), "a 200 000 digit serial", move || text::vars::format_serial(&format, 42));
    assert_eq!(out.len(), 200_000);
    assert!(out.ends_with("042"));
}

#[test]
fn degenerate_layouts_do_not_panic() {
    let hershey = |s: &str| {
        let mut t = Text::new(s, FontSpec { family: "Hershey Sans".into(), kind: FontKind::Stroke }, 5.0);
        t.welded = false;
        t
    };
    // Missing glyphs, RTL and combining marks in a font without them: warnings, no panic.
    for s in ["\u{2603}\u{2603}", "\u{5e9}\u{5dc}\u{5d5}\u{5dd}", "e\u{301}", "\u{200b}", "\t", "\n\n\n", "A\u{FFFF}B"] {
        let r = text::render_text(&hershey(s), s, None).unwrap();
        assert!(geom::is_finite(&r.outline), "{s:?}");
    }
    // VSpace -100 % stacks every line on the first; bend 360 with one glyph; a path shorter than the text.
    let mut t = hershey("AB\nCD\nEF");
    t.v_spacing = -100.0;
    t.bend_deg = 360.0;
    t.distort = true;
    assert!(geom::is_finite(&text::render_text(&t, &t.text, None).unwrap().outline));
    t.bend_deg = 0.0;
    t.path = Some(doc::TextPath { object: "p".into(), offset_mm: -1e6 });
    let mut p = kurbo::BezPath::new();
    p.move_to((0.0, 0.0));
    p.line_to((1.0, 0.0));
    let r = text::render_text(&t, &t.text, Some(&p)).unwrap();
    assert!(geom::is_finite(&r.outline));
    // A path of a single point (zero length).
    let mut dot = kurbo::BezPath::new();
    dot.move_to((3.0, 3.0));
    dot.line_to((3.0, 3.0));
    let _ = text::render_text(&t, &t.text, Some(&dot));
    // Smallest and largest heights.
    for h in [1e-9, 10_000.0] {
        let mut t = hershey("Wide");
        t.height_mm = h;
        assert!(geom::is_finite(&text::render_text(&t, &t.text, None).unwrap().outline), "{h}");
    }
}

/// Welding a long text whose glyphs all overlap (HSpace that cancels the advance) must not take minutes: every pair
/// of glyphs would be tested for contact.
#[test]
fn welding_many_overlapping_glyphs_is_bounded() {
    let spec = FontSpec::system("Arial");
    if text::resolve(&spec, false, false).is_err() {
        return;
    }
    let mut t = Text::new("i".repeat(1000), spec, 5.0);
    t.welded = true;
    t.h_spacing = -22.2;
    let r = within(Duration::from_secs(60), "welding 1000 stacked glyphs", move || text::render_text(&t, &t.text, None));
    let r = r.unwrap();
    assert!(geom::is_finite(&r.outline));
}
