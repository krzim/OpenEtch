//! Workspace frames: a galvo's centred field and a gantry bed measured from its origin corner (`MirrorX` /
//! `MirrorY`). Synthetic projects only.

use doc::Document;
use kurbo::Point;
use lbrn::{Frame, Shape, from_document_in, parse, to_document, to_document_in, write};

/// A 400 x 300 mm bed project: one 20 x 10 rectangle centred at (100, 50) in LightBurn coordinates.
fn bed_project(mirror_x: &str, mirror_y: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="1.7.08" FormatVersion="1" MaterialHeight="0" MirrorX="{mirror_x}" MirrorY="{mirror_y}">
    <CutSetting type="Cut">
        <index Value="0"/>
        <name Value="C00"/>
        <priority Value="0"/>
    </CutSetting>
    <Shape Type="Rect" CutIndex="0" W="20" H="10" Cr="0">
        <XForm>1 0 0 1 100 50</XForm>
    </Shape>
</LightBurnProject>
"#
    )
}

fn centre(d: &Document) -> Point {
    d.objects[0].bounds().center()
}

fn imported(text: &str, frame: Frame) -> Document {
    let p = parse(text).unwrap();
    let i = to_document_in(&p, frame).unwrap();
    assert!(i.warnings.iter().all(|w| !w.contains("outside")), "{:?}", i.warnings);
    Document { objects: i.objects, layers: i.layers, groups: i.groups, extras: i.extras, ..Document::default() }
}

fn bed(text: &str) -> Frame {
    let (mirror_x, mirror_y) = parse(text).unwrap().mirror();
    Frame::Bed { width_mm: 400.0, height_mm: 300.0, mirror_x, mirror_y }
}

fn exported_centre(d: &Document, frame: Frame) -> (Point, (bool, bool), String) {
    let (p, warnings) = from_document_in(d, frame).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let text = write(&p);
    let back = parse(&text).unwrap();
    let rect: &Shape = &back.shapes[0];
    (rect.xform * Point::ORIGIN, back.mirror(), text)
}

#[test]
fn front_left_bed_moves_by_half_the_bed() {
    let text = bed_project("False", "False");
    let d = imported(&text, bed(&text));
    assert_eq!(centre(&d), Point::new(-100.0, -100.0), "field = LightBurn - [200, 150]");
    let (c, mirror, out) = exported_centre(&d, bed(&text));
    assert_eq!((c, mirror), (Point::new(100.0, 50.0), (false, false)));
    assert_eq!(out, text, "an unchanged import exports byte for byte");
}

#[test]
fn rear_left_origin_counts_y_toward_the_front() {
    // MirrorY: the origin is the rear-left corner and y grows toward the operator, so y = 50 is 50 mm from the rear.
    let text = bed_project("False", "True");
    let d = imported(&text, bed(&text));
    assert_eq!(centre(&d), Point::new(-100.0, 100.0), "50 mm from the rear edge (+150)");
    let (c, mirror, out) = exported_centre(&d, bed(&text));
    assert_eq!((c, mirror), (Point::new(100.0, 50.0), (false, true)));
    assert_eq!(out, text);
    // Exported for a front-left device the same place is 250 mm from the front, with the flags cleared.
    let (c, mirror, _) =
        exported_centre(&d, Frame::Bed { width_mm: 400.0, height_mm: 300.0, mirror_x: false, mirror_y: false });
    assert_eq!((c, mirror), (Point::new(100.0, 250.0), (false, false)));
}

#[test]
fn rear_right_and_front_right_mirror_x_too() {
    let text = bed_project("True", "True");
    assert_eq!(centre(&imported(&text, bed(&text))), Point::new(100.0, 100.0));
    let text = bed_project("True", "False");
    let d = imported(&text, bed(&text));
    assert_eq!(centre(&d), Point::new(100.0, -100.0));
    let (c, mirror, out) = exported_centre(&d, bed(&text));
    assert_eq!((c, mirror), (Point::new(100.0, 50.0), (true, false)));
    assert_eq!(out, text);
}

#[test]
fn galvo_frame_is_the_centred_field_and_ignores_mirror_flags() {
    let text = bed_project("False", "True");
    let p = parse(&text).unwrap();
    let galvo = to_document(&p, 150.0).unwrap();
    assert_eq!(galvo.objects[0].bounds().center(), Point::new(25.0, -25.0));
    assert!(galvo.warnings.iter().any(|w| w.contains("MirrorY=True was not applied")), "{:?}", galvo.warnings);
    let same = to_document_in(&p, Frame::Galvo { field_mm: 150.0 }).unwrap();
    assert_eq!(same, galvo);
    // A galvo export keeps the root attributes the import kept.
    let d = Document { objects: galvo.objects, layers: galvo.layers, extras: galvo.extras, ..Document::default() };
    let (c, mirror, out) = exported_centre(&d, Frame::Galvo { field_mm: 150.0 });
    assert_eq!((c, mirror), (Point::new(100.0, 50.0), (false, true)));
    assert_eq!(out, text);
}

#[test]
fn bed_sizes_are_checked_and_outside_art_is_reported() {
    let text = bed_project("False", "False");
    let p = parse(&text).unwrap();
    for (w, h) in [(0.0, 300.0), (400.0, f64::NAN), (6000.0, 300.0)] {
        let e = to_document_in(&p, Frame::Bed { width_mm: w, height_mm: h, mirror_x: false, mirror_y: false });
        assert!(e.is_err(), "{w} x {h}");
    }
    let small = to_document_in(&p, Frame::Bed { width_mm: 100.0, height_mm: 60.0, mirror_x: false, mirror_y: false });
    let w = small.unwrap().warnings;
    assert!(w.iter().any(|w| w.contains("outside the 100 x 60 mm workspace")), "{w:?}");
    // A document that never came from LightBurn gets the target device's flags written.
    let d = imported(&text, bed(&text));
    let fresh = Document { objects: d.objects, layers: d.layers, ..Document::default() };
    let (_, mirror, _) =
        exported_centre(&fresh, Frame::Bed { width_mm: 400.0, height_mm: 300.0, mirror_x: true, mirror_y: true });
    assert_eq!(mirror, (true, true));
}
